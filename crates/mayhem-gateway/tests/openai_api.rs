use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderMap, Method, Request, StatusCode},
    Router,
};
use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use mayhem_attestation::ValidatedAttestationPolicy;
use mayhem_gateway::openai::{
    openai_router, validate_loopback_dashboard_bind, ArtifactGenerationOutput,
    ArtifactGenerationRequest, AudioSpeechOutput, AudioSpeechRequest, AudioTranscriptionOutput,
    AudioTranscriptionRequest, ChatCompletionRequest, ChatMessage, ChatOutput, EmbeddingOutput,
    EmbeddingRequest, GatewayArtifactGenerationFuture, GatewayArtifactGenerationResult,
    GatewayArtifactOutput, GatewayAttestationAuthority, GatewayAttestationCollateral,
    GatewayAudioSpeechFuture, GatewayAudioSpeechResult, GatewayAudioTranscriptionFuture,
    GatewayAudioTranscriptionResult, GatewayCanaryModelConfig, GatewayCanaryProbePolicy,
    GatewayCanaryPrompt, GatewayCanaryRegistry, GatewayEmbeddingFuture, GatewayEmbeddingResult,
    GatewayImageGenerationFuture, GatewayImageGenerationResult, GatewayMarketInfo, GatewayModel,
    GatewayRouteCandidate, GatewaySessionBackend, GatewaySessionError, GatewaySessionFuture,
    GatewaySessionInvocation, GatewaySessionResult, GatewaySpecialityCalibration, GatewayState,
    ImageGenerationOutput, ImageGenerationRequest, MayhemModelInfo, ModelCaps, PriceRefAu,
    ProviderKybInfo, ProviderSignedReceipt, SamplingProfile, ShapeAdapterInfo, ToolCallOutput,
    Usage,
};
use mayhem_gateway::{
    aggregate_canary_fingerprints, audio_fingerprint, image_average_hash_hex, normalize_rate_map,
    priced_usage_au, text_generation_rate_map, token_fingerprint, HeartbeatAttestation,
    HeartbeatCaps, HeartbeatModalityCapacity, HeartbeatPerf, HeartbeatQueue, HeartbeatReceiver,
    HeartbeatSlots, ProviderHeartbeat, ProviderKey, ReputationEventKind, HEARTBEAT_SCHEMA_VERSION,
};
use mayhem_proto::{
    catalog_enclave_id, endpoint_request_fingerprint, receipt_signing_bytes,
    AdminAttestationPolicy, AdminEnclaveAttestationBinding, AttestationQuoteKindPolicy,
    AttestationTrustDataKind, AttestationTrustDataRef, AttestationVerifierProfile,
    CatalogEnclaveIdentity, EndpointAttributeSpec, EndpointSpecialityMapping,
    EndpointSpecialitySelector, EndpointSpecialityTarget, EndpointValueType, HardwareQuoteKind,
    ModelSpecialityDescriptor, ModelSpecialityLevel, ReceiptBody, ReceiptUsage,
    TranscriptionResult, TranscriptionTimestamp, ATTESTATION_POLICY_SCHEMA_VERSION,
    CONTRACT_VERSION, DEFAULT_MODEL_CLASS, SESSION_RECEIPT_SCHEMA_VERSION,
    TRANSCRIPTION_RESULT_SCHEMA_VERSION, USAGE_AUDIO_SECOND, USAGE_FRAME, USAGE_IMAGE,
    USAGE_INPUT_CHARACTER, USAGE_STEP, USAGE_VIDEO_SECOND,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tower::ServiceExt;

#[derive(Debug)]
struct TestDirectSessionBackend;

impl GatewaySessionBackend for TestDirectSessionBackend {
    fn name(&self) -> &str {
        "test-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            let prompt_tokens = request.messages.len() as u64;
            let completion_tokens = 4;
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: Some(format!(
                        "direct session response from {} via {}",
                        model.id, invocation.session_id
                    )),
                    tool_calls: Vec::new(),
                    artifacts: Vec::new(),
                    finish_reason: "stop".to_owned(),
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens: prompt_tokens + completion_tokens,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                token_ids: vec![1, 2, 3, 4],
                quality: None,
            })
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
struct SpecialityTransportRecord {
    endpoint_request: Value,
    reasoning_effort: Option<String>,
    speciality_values: BTreeMap<String, Value>,
    effective_specialities: BTreeMap<String, String>,
    voucher_specialities: BTreeMap<String, String>,
    top_k: Option<i32>,
    min_p: Option<f64>,
}

#[derive(Debug)]
struct SpecialityRecordingBackend {
    records: Arc<Mutex<Vec<SpecialityTransportRecord>>>,
}

impl GatewaySessionBackend for SpecialityRecordingBackend {
    fn name(&self) -> &str {
        "speciality-recording"
    }

    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            self.records
                .lock()
                .expect("speciality transport records")
                .push(SpecialityTransportRecord {
                    endpoint_request: request.endpoint_request.clone().unwrap_or(Value::Null),
                    reasoning_effort: request.reasoning_effort.clone(),
                    speciality_values: request.speciality_values.clone(),
                    effective_specialities: request.effective_specialities.clone(),
                    voucher_specialities: invocation
                        .spend_voucher
                        .body
                        .required_specialities
                        .clone(),
                    top_k: request.top_k,
                    min_p: request.min_p,
                });
            let prompt_tokens = request.messages.len() as u64;
            let completion_tokens = 4;
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: Some(format!("speciality response from {}", model.id)),
                    tool_calls: Vec::new(),
                    artifacts: Vec::new(),
                    finish_reason: "stop".to_owned(),
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens: prompt_tokens + completion_tokens,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                token_ids: vec![1, 2, 3, 4],
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct EmbeddingDirectSessionBackend;

impl GatewaySessionBackend for EmbeddingDirectSessionBackend {
    fn name(&self) -> &str {
        "test-embedding-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async { Err(GatewaySessionError::new("chat not expected")) })
    }

    fn run_embedding<'a>(
        &'a self,
        _model: &'a GatewayModel,
        request: &'a EmbeddingRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewayEmbeddingFuture<'a> {
        Box::pin(async move {
            let input_count = request.input.as_array().map_or(1, Vec::len);
            let prompt_tokens = match &request.input {
                Value::String(value) => value.split_whitespace().count() as u64,
                Value::Array(values) => values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|value| value.split_whitespace().count() as u64)
                    .sum(),
                _ => 0,
            };
            let embeddings = (0..input_count)
                .map(|index| {
                    if index == 0 {
                        vec![0.12, 0.34, 0.56]
                    } else {
                        vec![0.11, 0.33, 0.55]
                    }
                })
                .collect();
            Ok(GatewayEmbeddingResult {
                output: EmbeddingOutput {
                    embeddings,
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens: 0,
                        total_tokens: prompt_tokens,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct ImageGenerationDirectSessionBackend;

impl GatewaySessionBackend for ImageGenerationDirectSessionBackend {
    fn name(&self) -> &str {
        "test-image-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async { Err(GatewaySessionError::new("chat not expected")) })
    }

    fn run_image_generation<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ImageGenerationRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewayImageGenerationFuture<'a> {
        Box::pin(async move {
            let image = b"\x89PNG mayhem image".to_vec();
            let usage = image_usage_for_test(request);
            let provider_receipt =
                signed_image_provider_receipt(model, request, invocation, &usage)?;
            Ok(GatewayImageGenerationResult {
                output: ImageGenerationOutput {
                    artifacts: vec![GatewayArtifactOutput {
                        id: "image-1".to_owned(),
                        content_type: "image/png".to_owned(),
                        blake3: blake3::hash(&image).to_hex().to_string(),
                        bytes: image,
                    }],
                    usage,
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: Some(provider_receipt),
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct ArtifactGenerationDirectSessionBackend;

impl GatewaySessionBackend for ArtifactGenerationDirectSessionBackend {
    fn name(&self) -> &str {
        "test-artifact-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async { Err(GatewaySessionError::new("chat not expected")) })
    }

    fn run_artifact_generation<'a>(
        &'a self,
        _model: &'a GatewayModel,
        request: &'a ArtifactGenerationRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewayArtifactGenerationFuture<'a> {
        Box::pin(async move {
            let (content_type, bytes, usage) = if request.output_modality == "video" {
                (
                    "video/mp4",
                    b"mayhem-test-mp4".to_vec(),
                    ReceiptUsage::from_units([
                        (
                            USAGE_VIDEO_SECOND,
                            request
                                .duration_seconds
                                .saturating_mul(request.artifact_count),
                        ),
                        (
                            USAGE_FRAME,
                            request.frame_count.saturating_mul(request.artifact_count),
                        ),
                    ]),
                )
            } else {
                (
                    "audio/wav",
                    wav_bytes_for_duration_seconds(request.duration_seconds),
                    ReceiptUsage::from_units([
                        (
                            USAGE_INPUT_CHARACTER,
                            mayhem_proto::artifact_generation_input_characters(
                                &request.endpoint_family,
                                &request.contract_request,
                            ),
                        ),
                        (
                            USAGE_AUDIO_SECOND,
                            request
                                .duration_seconds
                                .saturating_mul(request.artifact_count),
                        ),
                    ]),
                )
            };
            let artifacts = (0..request.artifact_count)
                .map(|index| GatewayArtifactOutput {
                    id: format!("{}-test-{index}", request.output_modality),
                    content_type: content_type.to_owned(),
                    blake3: blake3::hash(&bytes).to_hex().to_string(),
                    bytes: bytes.clone(),
                })
                .collect();
            Ok(GatewayArtifactGenerationResult {
                output: ArtifactGenerationOutput { artifacts, usage },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct ArtifactGenerationRecordingBackend {
    requests: Arc<Mutex<Vec<ArtifactGenerationRequest>>>,
}

impl GatewaySessionBackend for ArtifactGenerationRecordingBackend {
    fn name(&self) -> &str {
        "test-artifact-recording"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async { Err(GatewaySessionError::new("chat not expected")) })
    }

    fn run_artifact_generation<'a>(
        &'a self,
        _model: &'a GatewayModel,
        request: &'a ArtifactGenerationRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewayArtifactGenerationFuture<'a> {
        Box::pin(async move {
            self.requests
                .lock()
                .expect("artifact generation records")
                .push(request.clone());
            let bytes = wav_bytes_for_duration_seconds(request.duration_seconds);
            let artifacts = (0..request.artifact_count)
                .map(|index| GatewayArtifactOutput {
                    id: format!("recorded-audio-{index}"),
                    content_type: "audio/wav".to_owned(),
                    blake3: blake3::hash(&bytes).to_hex().to_string(),
                    bytes: bytes.clone(),
                })
                .collect();
            Ok(GatewayArtifactGenerationResult {
                output: ArtifactGenerationOutput {
                    artifacts,
                    usage: ReceiptUsage::from_units([
                        (
                            USAGE_INPUT_CHARACTER,
                            mayhem_proto::artifact_generation_input_characters(
                                &request.endpoint_family,
                                &request.contract_request,
                            ),
                        ),
                        (
                            USAGE_AUDIO_SECOND,
                            request
                                .duration_seconds
                                .saturating_mul(request.artifact_count),
                        ),
                    ]),
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct ExtraCanaryArtifactBackend;

impl GatewaySessionBackend for ExtraCanaryArtifactBackend {
    fn name(&self) -> &str {
        "test-extra-canary-artifact"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async { Err(GatewaySessionError::new("chat not expected")) })
    }

    fn run_artifact_generation<'a>(
        &'a self,
        _model: &'a GatewayModel,
        request: &'a ArtifactGenerationRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewayArtifactGenerationFuture<'a> {
        Box::pin(async move {
            let count = request
                .artifact_count
                .saturating_add(u64::from(request.prompt == "fixed music canary"));
            let bytes = wav_bytes_for_duration_seconds(request.duration_seconds);
            let artifacts = (0..count)
                .map(|index| GatewayArtifactOutput {
                    id: format!("recorded-audio-{index}"),
                    content_type: "audio/wav".to_owned(),
                    blake3: blake3::hash(&bytes).to_hex().to_string(),
                    bytes: bytes.clone(),
                })
                .collect();
            Ok(GatewayArtifactGenerationResult {
                output: ArtifactGenerationOutput {
                    artifacts,
                    usage: ReceiptUsage::from_units([
                        (
                            USAGE_INPUT_CHARACTER,
                            mayhem_proto::artifact_generation_input_characters(
                                &request.endpoint_family,
                                &request.contract_request,
                            ),
                        ),
                        (
                            USAGE_AUDIO_SECOND,
                            request
                                .duration_seconds
                                .saturating_mul(request.artifact_count),
                        ),
                    ]),
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct ImageCanarySessionBackend {
    user_bytes: Vec<u8>,
    canary_bytes: Vec<u8>,
    requests: Arc<Mutex<Vec<ImageGenerationRequest>>>,
}

impl GatewaySessionBackend for ImageCanarySessionBackend {
    fn name(&self) -> &str {
        "test-image-canary-session"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async { Err(GatewaySessionError::new("chat not expected")) })
    }

    fn run_image_generation<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ImageGenerationRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewayImageGenerationFuture<'a> {
        Box::pin(async move {
            self.requests
                .lock()
                .expect("image request lock")
                .push(request.clone());
            let image = if request.prompt.contains("fixed image canary") {
                self.canary_bytes.clone()
            } else {
                self.user_bytes.clone()
            };
            let usage = image_usage_for_test(request);
            let provider_receipt =
                signed_image_provider_receipt(model, request, invocation, &usage)?;
            Ok(GatewayImageGenerationResult {
                output: ImageGenerationOutput {
                    artifacts: vec![GatewayArtifactOutput {
                        id: "image-1".to_owned(),
                        content_type: "image/png".to_owned(),
                        blake3: blake3::hash(&image).to_hex().to_string(),
                        bytes: image,
                    }],
                    usage,
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: Some(provider_receipt),
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct AudioSpeechDirectSessionBackend;

impl GatewaySessionBackend for AudioSpeechDirectSessionBackend {
    fn name(&self) -> &str {
        "test-audio-speech-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async { Err(GatewaySessionError::new("chat not expected")) })
    }

    fn run_audio_speech<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a AudioSpeechRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewayAudioSpeechFuture<'a> {
        Box::pin(async move {
            let audio = tiny_wav_bytes(16_000);
            let usage = audio_speech_usage_for_test(request, &audio);
            let provider_receipt =
                signed_audio_speech_provider_receipt(model, request, invocation, &usage)?;
            Ok(GatewayAudioSpeechResult {
                output: AudioSpeechOutput {
                    artifacts: vec![GatewayArtifactOutput {
                        id: "speech-1".to_owned(),
                        content_type: "audio/wav".to_owned(),
                        blake3: blake3::hash(&audio).to_hex().to_string(),
                        bytes: audio,
                    }],
                    usage,
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: Some(provider_receipt),
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct AudioTranscriptionDirectSessionBackend;

impl GatewaySessionBackend for AudioTranscriptionDirectSessionBackend {
    fn name(&self) -> &str {
        "test-audio-transcription-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async { Err(GatewaySessionError::new("chat not expected")) })
    }

    fn run_audio_transcription<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a AudioTranscriptionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewayAudioTranscriptionFuture<'a> {
        Box::pin(async move {
            let usage = audio_transcription_usage_for_test(request);
            let provider_receipt =
                signed_audio_transcription_provider_receipt(model, request, invocation, &usage)?;
            Ok(GatewayAudioTranscriptionResult {
                output: AudioTranscriptionOutput {
                    transcription: test_transcription_result(),
                    usage,
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: Some(provider_receipt),
                quality: None,
            })
        })
    }
}

fn test_transcription_result() -> TranscriptionResult {
    TranscriptionResult {
        schema_version: TRANSCRIPTION_RESULT_SCHEMA_VERSION,
        text: "hello mayhem".to_owned(),
        detected_language: Some("en".to_owned()),
        duration_seconds: Some(2.0),
        words: vec![
            TranscriptionTimestamp {
                text: "hello".to_owned(),
                start: 0.0,
                end: 0.8,
            },
            TranscriptionTimestamp {
                text: "mayhem".to_owned(),
                start: 1.0,
                end: 2.0,
            },
        ],
        segments: vec![
            TranscriptionTimestamp {
                text: "hello".to_owned(),
                start: 0.0,
                end: 0.9,
            },
            TranscriptionTimestamp {
                text: "mayhem".to_owned(),
                start: 1.0,
                end: 2.0,
            },
        ],
    }
}

#[derive(Debug)]
struct TextOnlyAudioTranscriptionBackend;

impl GatewaySessionBackend for TextOnlyAudioTranscriptionBackend {
    fn name(&self) -> &str {
        "test-text-only-audio-transcription"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async { Err(GatewaySessionError::new("chat not expected")) })
    }

    fn run_audio_transcription<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a AudioTranscriptionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewayAudioTranscriptionFuture<'a> {
        Box::pin(async move {
            let usage = audio_transcription_usage_for_test(request);
            let provider_receipt =
                signed_audio_transcription_provider_receipt(model, request, invocation, &usage)?;
            Ok(GatewayAudioTranscriptionResult {
                output: AudioTranscriptionOutput {
                    transcription: TranscriptionResult::text("hello mayhem"),
                    usage,
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: Some(provider_receipt),
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct RealSdCliImageGenerationBackend;

impl GatewaySessionBackend for RealSdCliImageGenerationBackend {
    fn name(&self) -> &str {
        "test-real-sd-cli"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async { Err(GatewaySessionError::new("chat not expected")) })
    }

    fn run_image_generation<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ImageGenerationRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewayImageGenerationFuture<'a> {
        Box::pin(async move {
            let count = request.n.unwrap_or(1).clamp(1, 4);
            if count != 1 {
                return Err(GatewaySessionError::new(
                    "real sd-cli test backend only supports n=1",
                ));
            }
            let binary = std::env::var_os("MAYHEM_STABLE_DIFFUSION_CPP_BIN")
                .map(PathBuf::from)
                .ok_or_else(|| {
                    GatewaySessionError::new("MAYHEM_STABLE_DIFFUSION_CPP_BIN is required")
                })?;
            let model_path = std::env::var_os("MAYHEM_STABLE_DIFFUSION_MODEL")
                .map(PathBuf::from)
                .ok_or_else(|| {
                    GatewaySessionError::new("MAYHEM_STABLE_DIFFUSION_MODEL is required")
                })?;
            let tempdir = tempfile::tempdir()
                .map_err(|err| GatewaySessionError::new(format!("tempdir failed: {err}")))?;
            let output_path = tempdir.path().join("image.png");
            let (width, height) = image_size_for_test(request);
            let mut command = tokio::process::Command::new(binary);
            command
                .arg("-m")
                .arg(model_path)
                .arg("-p")
                .arg(&request.prompt)
                .arg("-o")
                .arg(&output_path)
                .arg("--steps")
                .arg(image_steps_for_test(request).to_string())
                .arg("--cfg-scale")
                .arg(image_cfg_scale_for_test(request).to_string())
                .arg("--seed")
                .arg(request.seed.unwrap_or(42).to_string())
                .arg("--width")
                .arg(width.to_string())
                .arg("--height")
                .arg(height.to_string())
                .arg("--rng")
                .arg("cpu")
                .arg("--disable-image-metadata");
            if let Some(backend) = std::env::var_os("MAYHEM_STABLE_DIFFUSION_CPP_BACKEND") {
                if !backend.is_empty() {
                    command.arg("--backend").arg(backend);
                }
            }
            let output = command.output().await.map_err(|err| {
                GatewaySessionError::new(format!("starting real sd-cli failed: {err}"))
            })?;
            if !output.status.success() {
                return Err(GatewaySessionError::new(format!(
                    "real sd-cli exited with {}; stderr={:?}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
            let image = std::fs::read(&output_path).map_err(|err| {
                GatewaySessionError::new(format!("reading real sd-cli output failed: {err}"))
            })?;
            let usage = image_usage_for_test(request);
            let provider_receipt =
                signed_image_provider_receipt(model, request, invocation, &usage)?;
            Ok(GatewayImageGenerationResult {
                output: ImageGenerationOutput {
                    artifacts: vec![GatewayArtifactOutput {
                        id: "image-1".to_owned(),
                        content_type: "image/png".to_owned(),
                        blake3: blake3::hash(&image).to_hex().to_string(),
                        bytes: image,
                    }],
                    usage,
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: Some(provider_receipt),
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct ToolCallDirectSessionBackend;

impl GatewaySessionBackend for ToolCallDirectSessionBackend {
    fn name(&self) -> &str {
        "test-tool-call-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            let prompt_tokens = request.messages.len() as u64;
            let completion_tokens = 1;
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: None,
                    tool_calls: vec![
                        ToolCallOutput {
                            id: "call-normalized-1".to_owned(),
                            name: "write".to_owned(),
                            arguments: r#"{"filePath":"one.txt"}"#.to_owned(),
                        },
                        ToolCallOutput {
                            id: "call-normalized-2".to_owned(),
                            name: "write".to_owned(),
                            arguments: r#"{"filePath":"two.txt"}"#.to_owned(),
                        },
                    ],
                    artifacts: Vec::new(),
                    finish_reason: "tool_calls".to_owned(),
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens: prompt_tokens + completion_tokens,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                token_ids: vec![1],
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct ArtifactDirectSessionBackend;

impl GatewaySessionBackend for ArtifactDirectSessionBackend {
    fn name(&self) -> &str {
        "test-artifact-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            let prompt_tokens = request.messages.len() as u64;
            let image = b"\x89PNG mayhem artifact".to_vec();
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: Some(String::new()),
                    tool_calls: Vec::new(),
                    artifacts: vec![GatewayArtifactOutput {
                        id: "image-1".to_owned(),
                        content_type: "image/png".to_owned(),
                        blake3: blake3::hash(&image).to_hex().to_string(),
                        bytes: image,
                    }],
                    finish_reason: "stop".to_owned(),
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens: 0,
                        total_tokens: prompt_tokens,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                token_ids: Vec::new(),
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct VisionInspectBackend {
    seen_content: Arc<Mutex<Vec<Value>>>,
}

impl GatewaySessionBackend for VisionInspectBackend {
    fn name(&self) -> &str {
        "test-vision-inspect"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            self.seen_content
                .lock()
                .expect("seen content lock")
                .push(request.messages[0].content.clone());
            let prompt_tokens = request.messages.len() as u64;
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: Some("vision ok".to_owned()),
                    tool_calls: Vec::new(),
                    artifacts: Vec::new(),
                    finish_reason: "stop".to_owned(),
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens: 2,
                        total_tokens: prompt_tokens + 2,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                token_ids: vec![70, 71],
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct RetryThenDirectSessionBackend {
    retry_provider: String,
    calls: Arc<Mutex<Vec<String>>>,
}

impl GatewaySessionBackend for RetryThenDirectSessionBackend {
    fn name(&self) -> &str {
        "test-retry-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            let provider = invocation
                .provider_pubkey
                .clone()
                .unwrap_or_else(|| "<none>".to_owned());
            self.calls
                .lock()
                .expect("calls lock")
                .push(provider.clone());
            if provider == self.retry_provider {
                return Err(GatewaySessionError::retryable(
                    "simulated direct open timeout before spend",
                ));
            }
            let prompt_tokens = request.messages.len() as u64;
            let completion_tokens = 3;
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: Some(format!(
                        "direct retry response from {} via {}",
                        model.id, provider
                    )),
                    tool_calls: Vec::new(),
                    artifacts: Vec::new(),
                    finish_reason: "stop".to_owned(),
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens: prompt_tokens + completion_tokens,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                token_ids: vec![2, 3, 4],
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct RetryFirstDirectSessionBackend {
    calls: Arc<Mutex<Vec<String>>>,
}

impl GatewaySessionBackend for RetryFirstDirectSessionBackend {
    fn name(&self) -> &str {
        "test-retry-first-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            let provider = invocation
                .provider_pubkey
                .clone()
                .unwrap_or_else(|| "<none>".to_owned());
            let attempt = {
                let mut calls = self.calls.lock().expect("calls lock");
                calls.push(provider.clone());
                calls.len()
            };
            if attempt == 1 {
                return Err(GatewaySessionError::retryable(
                    "simulated first direct open timeout before spend",
                ));
            }
            let prompt_tokens = request.messages.len() as u64;
            let completion_tokens = 3;
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: Some(format!(
                        "direct retry response from {} via {}",
                        model.id, provider
                    )),
                    tool_calls: Vec::new(),
                    artifacts: Vec::new(),
                    finish_reason: "stop".to_owned(),
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens: prompt_tokens + completion_tokens,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                token_ids: vec![2, 3, 4],
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct AlwaysRetryDirectSessionBackend {
    calls: Arc<Mutex<Vec<String>>>,
}

impl GatewaySessionBackend for AlwaysRetryDirectSessionBackend {
    fn name(&self) -> &str {
        "test-always-retry-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            let provider = invocation
                .provider_pubkey
                .clone()
                .unwrap_or_else(|| "<none>".to_owned());
            self.calls.lock().expect("calls lock").push(provider);
            Err(GatewaySessionError::retryable(
                "simulated direct open timeout before spend",
            ))
        })
    }
}

#[derive(Debug)]
struct HedgeInspectBackend {
    invocations: Arc<Mutex<Vec<HedgeInvocationRecord>>>,
    probes: Arc<Mutex<Vec<String>>>,
    probe_delays_ms: BTreeMap<String, u64>,
}

type HedgeInvocationRecord = (String, bool, usize, usize, Option<String>);

impl GatewaySessionBackend for HedgeInspectBackend {
    fn name(&self) -> &str {
        "test-hedge-inspect"
    }

    fn hedge_probe<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        invocation: &'a mayhem_gateway::openai::GatewayHedgeProbeInvocation,
    ) -> mayhem_gateway::openai::GatewayHedgeProbeFuture<'a> {
        Box::pin(async move {
            let provider = invocation
                .provider_pubkey
                .clone()
                .unwrap_or_else(|| "<none>".to_owned());
            self.probes
                .lock()
                .expect("probes lock")
                .push(provider.clone());
            let delay = self.probe_delays_ms.get(&provider).copied().unwrap_or(1);
            tokio::time::sleep(Duration::from_millis(delay)).await;
            Ok(mayhem_gateway::openai::GatewayHedgeProbeResult {
                provider,
                ttft_ms: delay.max(1),
            })
        })
    }

    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            let provider = invocation
                .provider_pubkey
                .clone()
                .unwrap_or_else(|| "<none>".to_owned());
            self.invocations.lock().expect("invocations lock").push((
                provider.clone(),
                invocation.hedge.requested,
                invocation.hedge.planned_probe_count,
                invocation.hedge.actual_probe_count,
                invocation.hedge.winner_provider.clone(),
            ));
            let prompt_tokens = request.messages.len() as u64;
            let completion_tokens = 2;
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: Some(format!("hedge inspected for {} via {}", model.id, provider)),
                    tool_calls: Vec::new(),
                    artifacts: Vec::new(),
                    finish_reason: "stop".to_owned(),
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens: prompt_tokens + completion_tokens,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                token_ids: vec![5, 6],
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct CanarySubstitutionBackend {
    calls: Arc<Mutex<Vec<String>>>,
}

impl GatewaySessionBackend for CanarySubstitutionBackend {
    fn name(&self) -> &str {
        "test-canary-substitution"
    }

    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            let prompt = request
                .messages
                .iter()
                .map(|message| message.content.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            self.calls.lock().expect("calls lock").push(prompt.clone());
            let is_canary = prompt.contains("fixed canary");
            let token_ids = if is_canary {
                vec![9, 9, 9]
            } else {
                vec![1, 2, 3]
            };
            let content = if is_canary {
                "substituted canary output".to_owned()
            } else {
                format!(
                    "normal direct session response from {} via {}",
                    model.id, invocation.session_id
                )
            };
            let prompt_tokens = request.messages.len() as u64;
            let completion_tokens = token_ids.len() as u64;
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: Some(content),
                    tool_calls: Vec::new(),
                    artifacts: Vec::new(),
                    finish_reason: "stop".to_owned(),
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens: prompt_tokens + completion_tokens,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                token_ids,
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct SpecialityCanaryBackend {
    calls: Arc<Mutex<Vec<(BTreeMap<String, String>, BTreeMap<String, String>)>>>,
}

impl GatewaySessionBackend for SpecialityCanaryBackend {
    fn name(&self) -> &str {
        "test-speciality-canary"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            self.calls.lock().expect("calls lock").push((
                request.effective_specialities.clone(),
                invocation.spend_voucher.body.required_specialities.clone(),
            ));
            let is_canary = request
                .messages
                .iter()
                .any(|message| message.content.to_string().contains("fixed canary"));
            let effort = request
                .effective_specialities
                .get("reasoning_effort")
                .map(String::as_str)
                .unwrap_or("low");
            let token_ids = if !is_canary {
                vec![1]
            } else if effort == "high" {
                vec![9]
            } else {
                vec![4]
            };
            let prompt_tokens = request.messages.len() as u64;
            let completion_tokens = token_ids.len() as u64;
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: Some(format!("{effort} speciality canary output")),
                    tool_calls: Vec::new(),
                    artifacts: Vec::new(),
                    finish_reason: "stop".to_owned(),
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens: prompt_tokens + completion_tokens,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                token_ids,
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct ContextNeedleBackend {
    answer_needle: bool,
}

impl GatewaySessionBackend for ContextNeedleBackend {
    fn name(&self) -> &str {
        "test-context-needle"
    }

    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            let prompt = request
                .messages
                .iter()
                .map(|message| message.content.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            let is_catalog_canary = prompt.contains("fixed canary");
            let is_context_needle = prompt.contains("CONTEXT NEEDLE CODE:");
            let content = if is_context_needle {
                let code = prompt
                    .split("CONTEXT NEEDLE CODE:")
                    .nth(1)
                    .and_then(|tail| tail.split_whitespace().next())
                    .unwrap_or("MAYHEM-CTX-MISSING");
                if self.answer_needle {
                    code.to_owned()
                } else {
                    "needle omitted".to_owned()
                }
            } else if is_catalog_canary {
                "substituted canary output".to_owned()
            } else {
                format!(
                    "normal direct session response from {} via {}",
                    model.id, invocation.session_id
                )
            };
            let token_ids = if is_catalog_canary {
                vec![9, 9, 9]
            } else if is_context_needle && self.answer_needle {
                vec![7, 7, 7]
            } else if is_context_needle {
                vec![0, 0, 0]
            } else {
                vec![1, 2, 3]
            };
            let prompt_tokens = request.messages.len() as u64;
            let completion_tokens = token_ids.len() as u64;
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: Some(content),
                    tool_calls: Vec::new(),
                    artifacts: Vec::new(),
                    finish_reason: "stop".to_owned(),
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens: prompt_tokens + completion_tokens,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                token_ids,
                quality: None,
            })
        })
    }
}

fn test_app() -> Router {
    openai_router(
        GatewayState::from_embedded_catalog()
            .with_receipt_balance_au(1_000_000_000_000_000_000)
            .with_dev_session_shim(),
    )
}

fn test_state_and_app() -> (GatewayState, Router) {
    let state = GatewayState::from_embedded_catalog()
        .with_receipt_balance_au(1_000_000_000_000_000_000)
        .with_dev_session_shim();
    let app = openai_router(state.clone());
    (state, app)
}

fn current_test_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

const AV3_APPLE_JWKS: &[u8] = br#"{"keys":[]}"#;

fn av3_verifier_profile(kind: HardwareQuoteKind) -> AttestationVerifierProfile {
    match kind {
        HardwareQuoteKind::AppleAppAttestJwt => AttestationVerifierProfile::AppleAppAttestNativeV1,
        HardwareQuoteKind::AmdSevSnpVcek => AttestationVerifierProfile::AmdSevSnpVcekV1,
        HardwareQuoteKind::IntelTdxDcap => AttestationVerifierProfile::IntelTdxDcapV1,
        HardwareQuoteKind::NvidiaGb10DeviceJwt => AttestationVerifierProfile::NvidiaGb10DeviceV1,
        HardwareQuoteKind::NvidiaNrasJwt => AttestationVerifierProfile::NvidiaNrasCompositeV1,
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt => {
            AttestationVerifierProfile::NvidiaNvtrustOfflineCompositeV1
        }
        HardwareQuoteKind::Tpm2QuoteEk => AttestationVerifierProfile::Tpm2EkActivateCredentialV1,
    }
}

fn av3_apple_authority(
    candidate: &GatewayRouteCandidate,
    policy_epoch: u64,
    expires_epoch: Option<u64>,
) -> (GatewayAttestationAuthority, String) {
    let jwks_digest = hex::encode(Sha256::digest(AV3_APPLE_JWKS));
    let policy = AdminAttestationPolicy {
        schema_version: ATTESTATION_POLICY_SCHEMA_VERSION,
        sequence: 1,
        previous_policy_digest: None,
        issued_epoch: 1,
        effective_epoch: 1,
        expires_epoch,
        min_verifier_version: 3,
        emergency_disabled_quote_kinds: BTreeSet::new(),
        origin_pins: Vec::new(),
        trust_data: vec![AttestationTrustDataRef {
            id: "apple-jwks".to_owned(),
            kind: AttestationTrustDataKind::VerificationKey,
            sha256: jwks_digest,
            media_type: "application/jwk-set+json".to_owned(),
            max_bytes: 4_096,
            valid_from_epoch: Some(1),
            valid_until_epoch: None,
            source: None,
        }],
        quote_kinds: HardwareQuoteKind::ALL
            .into_iter()
            .map(|kind| AttestationQuoteKindPolicy {
                kind,
                enabled: kind == HardwareQuoteKind::AppleAppAttestJwt,
                verifier_profile: av3_verifier_profile(kind),
                evidence_schema_version: 1,
                required_trust_data: if kind == HardwareQuoteKind::AppleAppAttestJwt {
                    BTreeSet::from(["apple-jwks".to_owned()])
                } else {
                    BTreeSet::new()
                },
                measurement_trust_data: BTreeSet::new(),
                platforms: BTreeSet::new(),
                required_measurement_layers: BTreeSet::new(),
            })
            .collect(),
    };
    let validated = ValidatedAttestationPolicy::validate(policy.clone())
        .expect("valid AV.3 Apple policy fixture");
    let digest = validated.digest().to_owned();
    let collateral_reference = policy.trust_data[0].clone();
    (
        GatewayAttestationAuthority::from_catalog_records(
            Some(vec![policy]),
            vec![AdminEnclaveAttestationBinding {
                enclave_id: candidate.enclave_id.clone(),
                kind: HardwareQuoteKind::AppleAppAttestJwt,
                platform: None,
                measurement_trust_data: BTreeMap::new(),
            }],
            [GatewayAttestationCollateral {
                reference: collateral_reference,
                bytes: AV3_APPLE_JWKS.to_vec(),
                observed_epoch: 1,
            }],
            policy_epoch,
        )
        .expect("construct AV.3 authority from signed catalog records"),
        digest,
    )
}

fn av3_route_key(candidate: &GatewayRouteCandidate) -> ProviderKey {
    ProviderKey::new(
        candidate.provider.clone(),
        candidate.enclave_id.clone(),
        candidate.room_id.clone(),
    )
}

fn av3_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn av3_provider(signing_key: &SigningKey) -> String {
    hex::encode(signing_key.verifying_key().to_bytes())
}

fn av3_signed_advertisement_heartbeat(
    model: &GatewayModel,
    candidate: &GatewayRouteCandidate,
    signing_key: &SigningKey,
    kind: HardwareQuoteKind,
    declared_platform: Option<&str>,
) -> (Value, ProviderHeartbeat, u64) {
    assert_eq!(candidate.provider, av3_provider(signing_key));
    let mut heartbeat = test_provider_heartbeat(model, candidate, 0.0, 0, 8, Some(50.0), 150);
    heartbeat.ts = current_test_millis();
    heartbeat.nonce = hex::encode(Sha256::digest(format!(
        "{}:{}:{}:{}",
        heartbeat.provider,
        heartbeat.room_id,
        heartbeat.ts,
        kind.as_str()
    )));
    heartbeat.sig.clear();
    let mut raw = serde_json::to_value(&heartbeat).expect("serialize AV.3 heartbeat");
    raw["att"]["quote_kind"] = serde_json::to_value(kind).expect("serialize quote kind");
    if let Some(platform) = declared_platform {
        raw["att"]["declared_platform"] = json!(platform);
    }
    let signature = signing_key.sign(
        &mayhem_gateway::heartbeat_signing_payload(&raw).expect("AV.3 heartbeat signing payload"),
    );
    raw["sig"] = json!(hex::encode(signature.to_bytes()));
    let mut receiver = HeartbeatReceiver::with_limits(60_000, 5_000);
    let heartbeat = receiver
        .receive(&raw, heartbeat.ts)
        .expect("signed AV.3 heartbeat");
    let received_at = heartbeat.ts;
    (raw, heartbeat, received_at)
}

fn av3_ingest_advertisement(
    state: &GatewayState,
    model: &GatewayModel,
    candidate: &GatewayRouteCandidate,
    signing_key: &SigningKey,
    kind: HardwareQuoteKind,
) {
    let (raw, heartbeat, received_at) =
        av3_signed_advertisement_heartbeat(model, candidate, signing_key, kind, None);
    state
        .ingest_authenticated_provider_heartbeat(&raw, heartbeat, received_at)
        .expect("ingest signed AV.3 heartbeat");
}

async fn json_request(app: Router, method: Method, uri: &str, body: Value) -> (StatusCode, Value) {
    json_request_with_headers(app, method, uri, body, &[]).await
}

async fn json_request_with_headers(
    app: Router,
    method: Method,
    uri: &str,
    body: Value,
    request_headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let (status, headers, bytes) =
        raw_request_with_headers(app, method, uri, Some(body), request_headers).await;
    assert!(headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .contains("application/json"));
    let json = serde_json::from_slice(&bytes).expect("response body is JSON");
    (status, json)
}

async fn raw_request(
    app: Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    raw_request_with_headers(app, method, uri, body, &[]).await
}

async fn raw_request_with_headers(
    app: Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    request_headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in request_headers {
        builder = builder.header(*name, *value);
    }
    let body = if let Some(body) = body {
        builder = builder.header("content-type", "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    let response = app
        .oneshot(builder.body(body).expect("request builds"))
        .await
        .expect("router response");
    let (parts, body) = response.into_parts();
    let bytes = to_bytes(body, usize::MAX)
        .await
        .expect("response body bytes")
        .to_vec();
    (parts.status, parts.headers, bytes)
}

async fn dashboard_request_with_headers(
    app: Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    request_headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let (status, headers, bytes) =
        raw_request_with_headers(app.clone(), method, uri, body, request_headers).await;
    if status != StatusCode::SEE_OTHER {
        return (status, headers, bytes);
    }

    let location = headers
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("dashboard bootstrap redirect location")
        .to_owned();
    let cookie = headers
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .expect("dashboard bootstrap cookie")
        .to_owned();
    let mut redirected_headers = request_headers.to_vec();
    if !redirected_headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("cookie"))
    {
        redirected_headers.push(("cookie", cookie.as_str()));
    }
    raw_request_with_headers(app, Method::GET, &location, None, &redirected_headers).await
}

#[derive(Default)]
struct DashboardTestBrowser {
    cookie: Option<String>,
}

impl DashboardTestBrowser {
    async fn request(
        &mut self,
        app: Router,
        method: Method,
        uri: &str,
        body: Option<Value>,
        request_headers: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut headers = request_headers.to_vec();
        let existing_cookie = self.cookie.clone();
        if let Some(cookie) = existing_cookie.as_deref() {
            headers.push(("cookie", cookie));
        }
        let response = dashboard_request_with_headers(app, method, uri, body, &headers).await;
        if let Some(cookie) = response
            .1
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
        {
            self.cookie = Some(cookie.to_owned());
        }
        response
    }
}

async fn raw_bytes_request_with_headers(
    app: Router,
    method: Method,
    uri: &str,
    body: Vec<u8>,
    request_headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in request_headers {
        builder = builder.header(*name, *value);
    }
    let response = app
        .oneshot(builder.body(Body::from(body)).expect("request builds"))
        .await
        .expect("router response");
    let (parts, body) = response.into_parts();
    let bytes = to_bytes(body, usize::MAX)
        .await
        .expect("response body bytes")
        .to_vec();
    (parts.status, parts.headers, bytes)
}

fn audio_transcription_multipart(fields: &[(&str, &str)]) -> (String, Vec<u8>) {
    let boundary = "mayhem-transcription-boundary";
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"clip.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&tiny_wav_bytes(32_000));
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (boundary.to_owned(), body)
}

async fn first_model_id() -> String {
    let (status, body) = json_request(test_app(), Method::GET, "/v1/models", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "list");
    body["data"][0]["id"].as_str().expect("model id").to_owned()
}

#[tokio::test]
async fn production_gateway_without_live_provider_refuses_local_chat_shim() {
    let state = GatewayState::from_embedded_catalog();
    let app = openai_router(state.clone());
    let (status, models) = json_request(app.clone(), Method::GET, "/v1/models", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let model = models["data"][0]["id"].as_str().expect("model id");
    let request = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Do not fabricate a local answer." }]
    });

    let (status, body) =
        json_request(app.clone(), Method::POST, "/v1/chat/completions", request).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("no provider available"));
    assert!(state.receipts().is_empty());

    let request = json!({ "model": model, "prompt": "No deterministic completions either." });
    let (status, body) = json_request(app.clone(), Method::POST, "/v1/completions", request).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("no provider available"));
    assert!(state.receipts().is_empty());

    let (status, body) = json_request(app, Method::GET, "/mayhem/status", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["backend"], "no-live-provider");
    assert_eq!(body["rail"], "fiat");
    assert_eq!(body["dev_session_shim"], false);
}

#[tokio::test]
async fn models_endpoint_returns_openai_list_shape_with_mayhem_extension() {
    let (status, body) = json_request(test_app(), Method::GET, "/v1/models", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "list");
    assert!(!body["data"].as_array().expect("model data").is_empty());
    assert_eq!(body["data"][0]["object"], "model");
    assert_eq!(body["data"][0]["owned_by"], "mayhem");
    assert_eq!(body["data"][0]["mayhem"]["price_ref_au"]["denom"], "au_usd");
    assert_eq!(body["data"][0]["mayhem"]["price_ref_au"]["ver"], 1);
    let rate_units = body["data"][0]["mayhem"]["price_ref_au"]["rate_map"]
        .as_array()
        .expect("rate map")
        .iter()
        .map(|entry| entry["unit"].as_str().expect("rate unit"))
        .collect::<Vec<_>>();
    assert_eq!(
        rate_units,
        vec!["input_token", "cached_input_token", "output_token"]
    );
    assert_eq!(body["data"][0]["mayhem"]["caps"]["tools"], false);
    assert_eq!(body["data"][0]["mayhem"]["caps"]["ctx"], 0);
    assert_eq!(body["data"][0]["mayhem"]["registered_caps"]["tools"], true);
    assert_eq!(
        body["data"][0]["mayhem"]["adapter"]["tool_call_strategy"],
        "qwen_function_xml"
    );
}

#[tokio::test]
async fn av3_missing_policy_filters_tier2_and_routes_tier1_fallback() {
    let tier2_signing_key = av3_signing_key(21);
    let tier2_provider = av3_provider(&tier2_signing_key);
    let tier1_provider = "22".repeat(32);
    let mut model =
        routed_test_model_with_providers(&[tier2_provider.clone(), tier1_provider.clone()]);
    model.mayhem.route_candidates[0].att_tier = 2;
    model.mayhem.route_candidates[0].device_key = Some("31".repeat(32));
    model.mayhem.attestation_tiers = BTreeMap::from([("T1".to_owned(), 1), ("T2".to_owned(), 1)]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![model.clone()]).with_session_backend(Arc::new(
        RetryThenDirectSessionBackend {
            retry_provider: "ff".repeat(32),
            calls: calls.clone(),
        },
    ));
    av3_ingest_advertisement(
        &state,
        &model,
        &model.mayhem.route_candidates[0],
        &tier2_signing_key,
        HardwareQuoteKind::AppleAppAttestJwt,
    );
    let app = openai_router(state.clone());

    let (status, body) = json_request(app.clone(), Method::GET, "/v1/models", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let mayhem = &body["data"][0]["mayhem"];
    assert_eq!(mayhem["route_count"], 1);
    let tier2 = &mayhem["registered_route_candidates"][0]["attestation_verification"];
    assert_eq!(tier2["attestation_tier"], 2);
    assert_eq!(tier2["policy_required"], true);
    assert_eq!(tier2["locally_ready"], false);
    assert!(tier2["reason"]
        .as_str()
        .expect("missing-policy reason")
        .contains("authority is not configured"));
    assert_eq!(
        mayhem["registered_route_candidates"][0]["dispatch_eligible"],
        false
    );
    let tier1 = &mayhem["route_candidates"][0]["attestation_verification"];
    assert_eq!(tier1["policy_required"], false);
    assert_eq!(tier1["locally_ready"], true);

    let (status, _) = json_request(
        app,
        Method::POST,
        "/v1/chat/completions",
        json!({
            "model": "mayhem/routed-test",
            "messages": [{"role": "user", "content": "Use the eligible route."}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        calls.lock().expect("route calls").clone(),
        vec![tier1_provider.clone()]
    );
    assert_eq!(state.receipts()[0].receipt.body.provider, tier1_provider);
}

#[tokio::test]
async fn av3_stale_substituted_and_quote_mismatched_policy_routes_are_filtered() {
    let signing_key = av3_signing_key(31);
    let mut model = routed_test_model();
    model.mayhem.route_candidates[0].provider = av3_provider(&signing_key);
    model.mayhem.route_candidates[0].att_tier = 2;
    model.mayhem.route_candidates[0].device_key = Some("32".repeat(32));
    model.mayhem.attestation_tiers = BTreeMap::from([("T2".to_owned(), 1)]);
    let candidate = model.mayhem.route_candidates[0].clone();

    let (stale_authority, _) = av3_apple_authority(&candidate, 2, Some(2));
    let stale_state = test_gateway_state_from_models(vec![model.clone()])
        .with_attestation_authority(stale_authority);
    av3_ingest_advertisement(
        &stale_state,
        &model,
        &candidate,
        &signing_key,
        HardwareQuoteKind::AppleAppAttestJwt,
    );
    let (status, stale) = json_request(
        openai_router(stale_state),
        Method::GET,
        "/v1/models",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stale["data"][0]["mayhem"]["route_count"], 0);
    assert!(stale["data"][0]["mayhem"]["registered_route_candidates"][0]
        ["attestation_verification"]["reason"]
        .as_str()
        .expect("stale-policy reason")
        .contains("PolicyExpired"));

    let substituted_signing_key = av3_signing_key(32);
    let mut substituted_candidate = candidate.clone();
    substituted_candidate.provider = av3_provider(&substituted_signing_key);
    let (substituted_authority, _) = av3_apple_authority(&candidate, 1, None);
    let substituted_state = test_gateway_state_from_models(vec![model.clone()])
        .with_attestation_authority(substituted_authority);
    av3_ingest_advertisement(
        &substituted_state,
        &model,
        &substituted_candidate,
        &substituted_signing_key,
        HardwareQuoteKind::AppleAppAttestJwt,
    );
    let (status, substituted) = json_request(
        openai_router(substituted_state),
        Method::GET,
        "/v1/models",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(substituted["data"][0]["mayhem"]["route_count"], 0);
    assert!(
        substituted["data"][0]["mayhem"]["registered_route_candidates"][0]
            ["attestation_verification"]["reason"]
            .as_str()
            .expect("substituted-route reason")
            .contains("no signed heartbeat quote-kind advertisement")
    );

    let (mismatched_authority, _) = av3_apple_authority(&candidate, 1, None);
    let mismatched_state = test_gateway_state_from_models(vec![model.clone()])
        .with_attestation_authority(mismatched_authority);
    av3_ingest_advertisement(
        &mismatched_state,
        &model,
        &candidate,
        &signing_key,
        HardwareQuoteKind::NvidiaNrasJwt,
    );
    let (status, mismatched) = json_request(
        openai_router(mismatched_state),
        Method::GET,
        "/v1/models",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mismatched["data"][0]["mayhem"]["route_count"], 0);
    assert!(
        mismatched["data"][0]["mayhem"]["registered_route_candidates"][0]
            ["attestation_verification"]["reason"]
            .as_str()
            .expect("quote-kind mismatch reason")
            .contains("proves Tier 3, not route Tier 2")
    );
}

#[tokio::test]
async fn av3_heartbeat_quote_kind_and_platform_are_signature_bound() {
    let signing_key = av3_signing_key(34);
    let provider = av3_provider(&signing_key);
    let mut model = routed_test_model_with_providers(&[provider]);
    let candidate = &mut model.mayhem.route_candidates[0];
    candidate.att_tier = 2;
    candidate.device_key = Some("34".repeat(32));
    let candidate = candidate.clone();
    let (authority, _) = av3_apple_authority(&candidate, 1, None);
    let state =
        test_gateway_state_from_models(vec![model.clone()]).with_attestation_authority(authority);
    let (raw, heartbeat, received_at) = av3_signed_advertisement_heartbeat(
        &model,
        &candidate,
        &signing_key,
        HardwareQuoteKind::AppleAppAttestJwt,
        None,
    );

    let mut kind_tamper = raw.clone();
    kind_tamper["att"]["quote_kind"] = json!("nvidia_nras_jwt");
    let error = state
        .ingest_authenticated_provider_heartbeat(&kind_tamper, heartbeat.clone(), received_at)
        .expect_err("tampered quote kind must fail");
    assert!(error.contains("signature failed"));

    let mut platform_tamper = raw.clone();
    platform_tamper["att"]["declared_platform"] = json!("windows-11-tpm2");
    let error = state
        .ingest_authenticated_provider_heartbeat(&platform_tamper, heartbeat.clone(), received_at)
        .expect_err("tampered platform must fail");
    assert!(error.contains("signature failed"));

    state
        .ingest_authenticated_provider_heartbeat(&raw, heartbeat, received_at)
        .expect("untampered signed advertisement");
    let (status, body) =
        json_request(openai_router(state), Method::GET, "/v1/models", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"][0]["mayhem"]["route_count"], 1);
    assert_eq!(
        body["data"][0]["mayhem"]["route_candidates"][0]["attestation_verification"]["quote_kind"],
        "apple_app_attest_jwt"
    );
}

#[tokio::test]
async fn av3_ready_source_built_route_and_dashboard_report_local_policy_truth() {
    let signing_key = av3_signing_key(33);
    let provider = av3_provider(&signing_key);
    let mut model = routed_test_model_with_providers(std::slice::from_ref(&provider));
    let candidate = &mut model.mayhem.route_candidates[0];
    candidate.att_tier = 2;
    candidate.device_key = Some("33".repeat(32));
    candidate.binary_hash = "de".repeat(32);
    candidate.approved_binary_hashes = BTreeSet::from(["ad".repeat(32)]);
    model.mayhem.attestation_tiers = BTreeMap::from([("T2".to_owned(), 1)]);
    let route_key = av3_route_key(candidate);
    let (authority, policy_digest) = av3_apple_authority(candidate, 1, None);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![model.clone()])
        .with_attestation_authority(authority)
        .with_session_backend(Arc::new(RetryThenDirectSessionBackend {
            retry_provider: "ff".repeat(32),
            calls: calls.clone(),
        }));
    av3_ingest_advertisement(
        &state,
        &model,
        &model.mayhem.route_candidates[0],
        &signing_key,
        HardwareQuoteKind::AppleAppAttestJwt,
    );
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let token_query = dashboard_url
        .strip_prefix("http://127.0.0.1:11435/mayhem/dashboard?")
        .expect("dashboard token query");
    let evidence_path = format!(
        "/mayhem/dashboard/evidence?{token_query}&kind=route&provider={}&enclave={}&room={}",
        route_key.provider, route_key.enclave_id, route_key.room_id
    );
    let app = openai_router(state.clone());

    let (status, body) = json_request(app.clone(), Method::GET, "/v1/models", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let mayhem = &body["data"][0]["mayhem"];
    assert_eq!(mayhem["route_count"], 1);
    assert_eq!(
        mayhem["attestation_verification"]["policy_required_routes"],
        1
    );
    assert_eq!(
        mayhem["attestation_verification"]["locally_ready_routes"],
        1
    );
    assert_eq!(
        mayhem["attestation_verification"]["runtime_binary_hash_role"],
        "evidence_only"
    );
    let readiness = &mayhem["route_candidates"][0]["attestation_verification"];
    assert_eq!(readiness["attestation_tier"], 2);
    assert_eq!(readiness["quote_kind"], "apple_app_attest_jwt");
    assert_eq!(readiness["policy_sequence"], 1);
    assert_eq!(readiness["policy_digest"], policy_digest);
    assert_eq!(readiness["policy_effective_epoch"], 1);
    assert_eq!(readiness["evaluated_epoch"], 1);
    assert_eq!(readiness["verifier_profile"], "apple_app_attest_native_v1");
    assert_eq!(readiness["evidence_schema_version"], 1);
    assert_eq!(readiness["locally_ready"], true);
    assert_eq!(readiness["runtime_binary_hash_evidence_only"], true);
    assert_eq!(
        mayhem["route_candidates"][0]["binary_hash"],
        "de".repeat(32)
    );
    assert_eq!(
        mayhem["route_candidates"][0]["approved_binary_hashes"],
        json!(["ad".repeat(32)])
    );

    let (status, _) = json_request_with_headers(
        app.clone(),
        Method::POST,
        "/v1/chat/completions",
        json!({
            "model": "mayhem/routed-test",
            "messages": [{"role": "user", "content": "Use source-built runtime evidence."}]
        }),
        &[("X-Mayhem-Min-Att-Tier", "2")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        calls.lock().expect("route calls").clone(),
        vec![provider.clone()]
    );

    let (status, _, evidence_bytes) = dashboard_request_with_headers(
        app,
        Method::GET,
        &evidence_path,
        None,
        &[("host", "127.0.0.1:11435"), ("accept", "application/json")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let evidence: Value = serde_json::from_slice(&evidence_bytes).expect("route evidence JSON");
    let local = &evidence["raw"]["local_attestation_verification"];
    assert_eq!(local["attestation_tier"], 2);
    assert_eq!(local["quote_kind"], "apple_app_attest_jwt");
    assert_eq!(local["policy_digest"], policy_digest);
    assert_eq!(local["locally_ready"], true);
    assert_eq!(local["runtime_binary_hash_evidence_only"], true);
    assert!(evidence["facts"]
        .as_array()
        .expect("evidence facts")
        .iter()
        .any(|fact| fact["label"] == "Local verification" && fact["value"] == "Ready"));
}

#[tokio::test]
async fn models_endpoint_lists_empty_canonical_market_without_routing_or_billing_it() {
    let mut model = routed_test_model();
    model.mayhem.providers_online = 0;
    model.mayhem.rooms = 1;
    model.mayhem.markets = vec![GatewayMarketInfo {
        enclave_id: "11".repeat(32),
        att_tier: 2,
        quant: "int4".to_owned(),
        ctx_bracket: Some("le8k".to_owned()),
        room_ids: vec!["22".repeat(16)],
        providers_online: 0,
        route_count: 0,
        availability: "no_eligible_provider_yet".to_owned(),
        price_ref_au: PriceRefAu {
            denom: "au_usd".to_owned(),
            ver: 2,
            rate_map: text_generation_rate_map(30, 90),
            per_req_au: 0,
            min_session_au: 0,
            derivation: None,
            history: Vec::new(),
        },
    }];
    model.mayhem.route_candidates.clear();
    let state = test_gateway_state_from_models(vec![model]);
    let app = openai_router(state.clone());

    let (status, body) = json_request(app.clone(), Method::GET, "/v1/models", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().map(Vec::len), Some(1));
    assert!(body["data"][0]["mayhem"]["markets"]
        .as_array()
        .is_some_and(Vec::is_empty));
    let registered_market = &body["data"][0]["mayhem"]["registered_markets"][0];
    assert_eq!(registered_market["att_tier"], 2);
    assert_eq!(registered_market["route_count"], 0);
    assert_eq!(
        registered_market["availability"],
        "no_eligible_provider_yet"
    );
    assert!(body["data"][0]["mayhem"]["route_candidates"]
        .as_array()
        .is_none_or(Vec::is_empty));

    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Do not fabricate a route." }]
    });
    let (status, body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("no provider available"));
    assert!(state.receipts().is_empty());
}

#[tokio::test]
async fn models_endpoint_keeps_zero_live_tier3_registration_out_of_confidential_fields() {
    let mut model = routed_test_model();
    model.mayhem.route_candidates[0].att_tier = 3;
    let candidate = model.mayhem.route_candidates[0].clone();
    model.mayhem.attestation_tiers = BTreeMap::from([("T3".to_owned(), 1)]);
    model.mayhem.attestation_tier_labels = BTreeMap::from([(
        "T3".to_owned(),
        "Tier 3 - hardware confidential compute; prompt-confidential when supported".to_owned(),
    )]);
    model.mayhem.markets = vec![GatewayMarketInfo {
        enclave_id: candidate.enclave_id.clone(),
        att_tier: 3,
        quant: candidate.quant.clone(),
        ctx_bracket: Some("le8k".to_owned()),
        room_ids: vec![candidate.room_id.clone()],
        providers_online: 1,
        route_count: 1,
        availability: "routable".to_owned(),
        price_ref_au: model.mayhem.price_ref_au.clone(),
    }];
    let now = current_test_millis();
    let mut stale_heartbeat = test_provider_heartbeat(
        &model,
        &model.mayhem.route_candidates[0],
        0.0,
        0,
        8,
        None,
        150,
    );
    stale_heartbeat.caps.ctx = 65_536;
    let heartbeat_ttl_millis = 60_000;
    let state = GatewayState::from_models(vec![model])
        .with_provider_heartbeat_ttl_millis(heartbeat_ttl_millis);
    state.ingest_provider_heartbeat(
        stale_heartbeat,
        now.saturating_sub(heartbeat_ttl_millis + 1),
    );

    let (status, body) =
        json_request(openai_router(state), Method::GET, "/v1/models", Value::Null).await;

    assert_eq!(status, StatusCode::OK);
    let mayhem = &body["data"][0]["mayhem"];
    assert_eq!(mayhem["registered_provider_count"], 1);
    assert_eq!(mayhem["registered_route_count"], 1);
    assert_eq!(mayhem["providers_online"], 0);
    assert_eq!(mayhem["route_count"], 0);
    assert!(mayhem["attestation_tiers"]
        .as_object()
        .is_some_and(|tiers| tiers.is_empty()));
    assert_eq!(mayhem["registered_attestation_tiers"]["T3"], 1);
    assert_eq!(mayhem["prompt_confidential"], false);
    assert_eq!(mayhem["registered_prompt_confidential"], true);
    assert_eq!(mayhem["caps"]["tools"], false);
    assert_eq!(mayhem["caps"]["json"], false);
    assert_eq!(mayhem["caps"]["ctx"], 0);
    assert_eq!(mayhem["registered_caps"]["tools"], true);
    assert_eq!(mayhem["registered_caps"]["json"], true);
    assert_eq!(mayhem["registered_caps"]["ctx"], 8_192);
    assert!(mayhem["route_candidates"]
        .as_array()
        .is_some_and(Vec::is_empty));
    assert_eq!(
        mayhem["registered_route_candidates"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        mayhem["registered_route_candidates"][0]["dispatch_eligible"],
        false
    );
    assert!(mayhem["markets"].as_array().is_some_and(Vec::is_empty));
    assert_eq!(
        mayhem["registered_markets"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(mayhem["registered_markets"][0]["att_tier"], 3);
    assert_eq!(mayhem["registered_markets"][0]["registered_route_count"], 1);
    assert_eq!(mayhem["registered_markets"][0]["route_count"], 0);
}

#[tokio::test]
async fn models_endpoint_derives_modality_and_speciality_only_from_fresh_live_routes() {
    let providers = ["41".repeat(32), "42".repeat(32)];
    let mut model = routed_test_model_with_specialities(&providers);
    model.mayhem.providers_online = 2;
    model.mayhem.rooms = 2;
    model.mayhem.caps.image = true;
    model.mayhem.caps.output_modalities = vec!["text".to_owned(), "image".to_owned()];
    model.mayhem.adapter.modality_set = vec!["text".to_owned(), "image".to_owned()];
    model.mayhem.adapter.endpoint_families.push(
        mayhem_proto::endpoint_family_contract_template(
            mayhem_proto::ENDPOINT_OPENAI_IMAGE_GENERATIONS,
        )
        .expect("image generation endpoint contract"),
    );
    model.mayhem.route_candidates[0].served_specialities.clear();
    model.mayhem.route_candidates[1].quant = "fp16".to_owned();
    model.mayhem.route_candidates[1].served_modalities = vec!["image".to_owned()];
    let stale_kyb = ProviderKybInfo {
        provider: model.mayhem.route_candidates[1].provider.clone(),
        legal_name: "Stale Image Provider GmbH".to_owned(),
        jurisdiction: "DE".to_owned(),
        proof_hash: "ab".repeat(32),
        kyb_ref: "kyb:stale-image".to_owned(),
    };
    model.mayhem.route_candidates[1].kyb = Some(stale_kyb.clone());
    let stale_candidate = model.mayhem.route_candidates[1].clone();
    model.mayhem.quant_buckets = BTreeMap::from([("int4".to_owned(), 1), ("fp16".to_owned(), 1)]);
    model.mayhem.kyb_identities = vec![stale_kyb];
    model.mayhem.markets = vec![GatewayMarketInfo {
        enclave_id: stale_candidate.enclave_id.clone(),
        att_tier: stale_candidate.att_tier,
        quant: stale_candidate.quant.clone(),
        ctx_bracket: Some("le8k".to_owned()),
        room_ids: vec![stale_candidate.room_id.clone()],
        providers_online: 1,
        route_count: 1,
        availability: "routable".to_owned(),
        price_ref_au: model.mayhem.price_ref_au.clone(),
    }];
    let now = current_test_millis();
    let mut fresh_heartbeat = test_provider_heartbeat(
        &model,
        &model.mayhem.route_candidates[0],
        0.0,
        0,
        8,
        None,
        150,
    );
    fresh_heartbeat.caps.tools = false;
    fresh_heartbeat.caps.json = true;
    fresh_heartbeat.caps.ctx = 4_096;
    let mut catalog_capped_heartbeat = fresh_heartbeat.clone();
    catalog_capped_heartbeat.caps.ctx = 16_384;
    catalog_capped_heartbeat.ts = catalog_capped_heartbeat.ts.saturating_add(1);
    catalog_capped_heartbeat.nonce = "catalog-capped-live-context".to_owned();
    let mut stale_heartbeat = test_provider_heartbeat(
        &model,
        &model.mayhem.route_candidates[1],
        0.0,
        0,
        8,
        None,
        150,
    );
    stale_heartbeat.caps.tools = true;
    stale_heartbeat.caps.json = true;
    stale_heartbeat.caps.ctx = 32_768;
    let heartbeat_ttl_millis = 60_000;
    let state = GatewayState::from_models(vec![model])
        .with_provider_heartbeat_ttl_millis(heartbeat_ttl_millis);
    state.ingest_provider_heartbeat(fresh_heartbeat, now);
    state.ingest_provider_heartbeat(
        stale_heartbeat,
        now.saturating_sub(heartbeat_ttl_millis + 1),
    );
    let app = openai_router(state.clone());

    let (status, body) = json_request(app.clone(), Method::GET, "/v1/models", Value::Null).await;

    assert_eq!(status, StatusCode::OK);
    let mayhem = &body["data"][0]["mayhem"];
    assert_eq!(mayhem["registered_provider_count"], 2);
    assert_eq!(mayhem["registered_route_count"], 2);
    assert_eq!(mayhem["providers_online"], 1);
    assert_eq!(mayhem["route_count"], 1);
    assert_eq!(mayhem["quant_buckets"], json!({ "int4": 1 }));
    assert_eq!(mayhem["registered_quant_buckets"]["fp16"], 1);
    assert!(mayhem["kyb_identities"]
        .as_array()
        .is_some_and(Vec::is_empty));
    assert_eq!(
        mayhem["registered_kyb_identities"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(mayhem["caps"]["tools"], false);
    assert_eq!(mayhem["caps"]["json"], true);
    assert_eq!(mayhem["caps"]["ctx"], 4_096);
    assert_eq!(mayhem["caps"]["image"], false);
    assert_eq!(mayhem["caps"]["output_modalities"], json!(["text"]));
    assert_eq!(mayhem["registered_caps"]["tools"], true);
    assert_eq!(mayhem["registered_caps"]["json"], true);
    assert_eq!(mayhem["registered_caps"]["ctx"], 8_192);
    assert_eq!(mayhem["registered_caps"]["image"], true);
    assert_eq!(
        mayhem["registered_caps"]["output_modalities"],
        json!(["text", "image"])
    );
    assert_eq!(mayhem["modality_availability"]["text"]["available"], true);
    assert_eq!(mayhem["modality_availability"]["image"]["available"], false);
    assert_eq!(
        mayhem["modality_availability"]["image"]["live_route_count"],
        0
    );
    assert_eq!(
        mayhem["modality_availability"]["image"]["registered_route_count"],
        1
    );
    assert_eq!(mayhem["route_candidates"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        mayhem["route_candidates"][0]["served_modalities"],
        json!(["text"])
    );
    assert_eq!(
        mayhem["registered_route_candidates"][1]["served_modalities"],
        json!(["image"])
    );
    assert_eq!(
        mayhem["registered_route_candidates"][1]["dispatch_eligible"],
        false
    );
    let high = &mayhem["speciality_availability"]["reasoning_effort"]["levels"]["high"];
    assert_eq!(high["available"], false);
    assert_eq!(high["live_provider_count"], 0);
    assert_eq!(high["canonical_provider_count"], 1);
    assert!(mayhem["markets"].as_array().is_some_and(Vec::is_empty));
    assert_eq!(mayhem["registered_markets"][0]["registered_route_count"], 1);
    assert_eq!(mayhem["registered_markets"][0]["route_count"], 0);

    state.ingest_provider_heartbeat(catalog_capped_heartbeat, now.saturating_add(1));
    let (status, body) = json_request(app, Method::GET, "/v1/models", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"][0]["mayhem"]["caps"]["ctx"], 8_192);
    assert_eq!(body["data"][0]["mayhem"]["registered_caps"]["ctx"], 8_192);
}

#[tokio::test]
async fn models_api_and_market_dashboard_share_selector_live_counts() {
    let providers = [
        "31".repeat(32),
        "32".repeat(32),
        "33".repeat(32),
        "34".repeat(32),
        "35".repeat(32),
        "36".repeat(32),
    ];
    let mut model = routed_test_model_with_providers(&providers);
    model.mayhem.route_candidates[1].quant = "fp16".to_owned();
    model.mayhem.route_candidates[2].att_tier = 2;
    model.mayhem.route_candidates[4].accepted_rails = vec!["tap".to_owned()];
    let enclave_id = model.mayhem.route_candidates[0].enclave_id.clone();
    let room_ids = model
        .mayhem
        .route_candidates
        .iter()
        .map(|candidate| candidate.room_id.clone())
        .collect::<Vec<_>>();
    model.mayhem.markets = vec![GatewayMarketInfo {
        enclave_id,
        att_tier: 1,
        quant: "int4".to_owned(),
        ctx_bracket: Some("le8k".to_owned()),
        room_ids,
        providers_online: 6,
        route_count: 6,
        availability: "routable".to_owned(),
        price_ref_au: model.mayhem.price_ref_au.clone(),
    }];
    let mut heartbeats = model
        .mayhem
        .route_candidates
        .iter()
        .map(|candidate| test_provider_heartbeat(&model, candidate, 0.2, 0, 4, Some(50.0), 150))
        .collect::<Vec<_>>();
    heartbeats[3].caps.ctx = 16_384;
    heartbeats[5].caps.served_modalities.clear();
    let heartbeat = heartbeats[0].clone();
    let state = GatewayState::from_models(vec![model]).with_provider_heartbeats(heartbeats);
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let market_path = dashboard_url
        .replacen(
            "/mayhem/dashboard?",
            "/mayhem/dashboard/network/markets?",
            1,
        )
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard URL is rooted at gateway")
        .to_owned();
    let app = openai_router(state.clone());

    let (status, body) = json_request(app.clone(), Method::GET, "/v1/models", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let mayhem = &body["data"][0]["mayhem"];
    assert_eq!(mayhem["registered_provider_count"], 6);
    assert_eq!(mayhem["registered_route_count"], 6);
    assert_eq!(mayhem["providers_online"], 3);
    assert_eq!(mayhem["route_count"], 3);
    let market = &mayhem["markets"][0];
    assert_eq!(market["registered_provider_count"], 6);
    assert_eq!(market["registered_route_count"], 6);
    assert_eq!(market["providers_online"], 1);
    assert_eq!(market["route_count"], 1);
    assert_eq!(market["availability"], "routable");
    let mut browser = DashboardTestBrowser::default();

    let (status, _, bytes) = browser
        .request(
            app.clone(),
            Method::GET,
            &market_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let dashboard = String::from_utf8(bytes).expect("market dashboard HTML");
    assert!(dashboard.contains(
        r#"data-live-route-count="1" data-live-provider-count="1" data-registered-route-count="6" data-registered-provider-count="6""#
    ));

    let mut draining = heartbeat;
    draining.accepting_new = false;
    draining.ts = draining.ts.saturating_add(1);
    draining.nonce = "draining-market-count".to_owned();
    state.ingest_provider_heartbeat(draining.clone(), draining.ts);

    let (status, body) = json_request(app.clone(), Method::GET, "/v1/models", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let mayhem = &body["data"][0]["mayhem"];
    assert_eq!(mayhem["registered_provider_count"], 6);
    assert_eq!(mayhem["providers_online"], 2);
    assert_eq!(mayhem["route_count"], 2);
    assert!(mayhem["markets"].as_array().is_some_and(Vec::is_empty));
    let market = &mayhem["registered_markets"][0];
    assert_eq!(market["registered_provider_count"], 6);
    assert_eq!(market["registered_route_count"], 6);
    assert_eq!(market["providers_online"], 0);
    assert_eq!(market["route_count"], 0);
    assert_eq!(market["availability"], "no_eligible_provider_yet");

    let (status, _, bytes) = browser
        .request(
            app,
            Method::GET,
            &market_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let dashboard = String::from_utf8(bytes).expect("market dashboard HTML");
    assert!(dashboard.contains(
        r#"data-live-route-count="0" data-live-provider-count="0" data-registered-route-count="6" data-registered-provider-count="6""#
    ));
    assert!(dashboard.contains("no_eligible_provider_yet"));
}

#[tokio::test]
async fn models_endpoint_and_progressive_evidence_expose_speciality_cost_and_availability() {
    let providers = ["91".repeat(32), "92".repeat(32)];
    let mut model = routed_test_model_with_providers(&providers);
    model.mayhem.adapter.specialities = vec![ModelSpecialityDescriptor {
        name: "reasoning_effort".to_owned(),
        mechanism: "enum".to_owned(),
        default_level: "low".to_owned(),
        levels: vec![
            ModelSpecialityLevel {
                name: "low".to_owned(),
                rank: 1,
                native_value: json!("low"),
                default_max_output_tokens: Some(4_096),
                max_reasoning_tokens: Some(1_024),
            },
            ModelSpecialityLevel {
                name: "high".to_owned(),
                rank: 2,
                native_value: json!("high"),
                default_max_output_tokens: Some(8_192),
                max_reasoning_tokens: Some(4_096),
            },
            ModelSpecialityLevel {
                name: "xhigh".to_owned(),
                rank: 3,
                native_value: json!("xhigh"),
                default_max_output_tokens: Some(16_384),
                max_reasoning_tokens: Some(8_192),
            },
        ],
        calibration_modalities: Vec::new(),
        research_evidence: vec!["pinned model-card fixture".to_owned()],
    }];
    model.mayhem.speciality_calibrations = BTreeMap::from([(
        "nvfp4".to_owned(),
        BTreeMap::from([(
            "reasoning_effort".to_owned(),
            BTreeMap::from([
                (
                    "low".to_owned(),
                    GatewaySpecialityCalibration {
                        fingerprint: "11".repeat(32),
                        verification_method: None,
                        token_prefixes: BTreeMap::new(),
                        output_tokens_min: 1_000,
                        output_tokens_max: 1_000,
                        reasoning_tokens_min: 250,
                        reasoning_tokens_max: 250,
                    },
                ),
                (
                    "high".to_owned(),
                    GatewaySpecialityCalibration {
                        fingerprint: "22".repeat(32),
                        verification_method: None,
                        token_prefixes: BTreeMap::new(),
                        output_tokens_min: 2_000,
                        output_tokens_max: 2_000,
                        reasoning_tokens_min: 1_500,
                        reasoning_tokens_max: 1_500,
                    },
                ),
                (
                    "xhigh".to_owned(),
                    GatewaySpecialityCalibration {
                        fingerprint: "33".repeat(32),
                        verification_method: None,
                        token_prefixes: BTreeMap::new(),
                        output_tokens_min: 3_000,
                        output_tokens_max: 3_000,
                        reasoning_tokens_min: 2_500,
                        reasoning_tokens_max: 2_500,
                    },
                ),
            ]),
        )]),
    )]);
    model.mayhem.route_candidates[0].served_specialities = BTreeMap::from([(
        "reasoning_effort".to_owned(),
        vec!["low".to_owned(), "high".to_owned()],
    )]);
    model.mayhem.route_candidates[1].served_specialities =
        BTreeMap::from([("reasoning_effort".to_owned(), vec!["low".to_owned()])]);
    let state = test_gateway_state_from_models(vec![model]);
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let token_query = dashboard_path
        .strip_prefix("/mayhem/dashboard?")
        .expect("dashboard token query");
    let user_path = format!("/mayhem/dashboard/models?{token_query}");
    let provider_path = format!("/mayhem/dashboard/earn/machines?{token_query}");
    let network_path = format!("/mayhem/dashboard/network/models?{token_query}");
    let evidence_path =
        format!("/mayhem/dashboard/evidence?{token_query}&kind=model&id=mayhem%2Frouted-test");
    let app = openai_router(state);
    let mut browser = DashboardTestBrowser::default();

    let (status, body) = json_request(app.clone(), Method::GET, "/v1/models", Value::Null).await;

    assert_eq!(status, StatusCode::OK);
    let availability = &body["data"][0]["mayhem"]["speciality_availability"]["reasoning_effort"];
    assert_eq!(availability["default_level"], "low");
    assert_eq!(availability["levels"]["low"]["live_provider_count"], 2);
    assert_eq!(availability["levels"]["high"]["live_provider_count"], 1);
    assert_eq!(availability["levels"]["xhigh"]["live_provider_count"], 0);
    assert_eq!(availability["levels"]["xhigh"]["available"], false);
    assert_eq!(
        availability["levels"]["high"]["expected_volume"]["output_tokens_min"],
        2_000
    );
    assert_eq!(
        availability["levels"]["high"]["expected_volume"]["expected_output_cost_au_min"],
        "120"
    );
    assert_eq!(
        availability["levels"]["high"]["expected_volume"]["expected_output_cost_usd_min"],
        "$0.00000000000000012"
    );
    assert_eq!(
        body["data"][0]["mayhem"]["route_candidates"][1]["served_specialities"]["reasoning_effort"],
        json!(["low"])
    );

    let (status, _, user_bytes) = browser
        .request(
            app.clone(),
            Method::GET,
            &user_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let user_html = String::from_utf8(user_bytes).expect("user dashboard html");
    assert!(user_html.contains("reasoning_effort:low|high|xhigh"));
    assert!(user_html.contains("data-evidence-url"));

    let (status, _, provider_bytes) = browser
        .request(
            app.clone(),
            Method::GET,
            &provider_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let provider_html = String::from_utf8(provider_bytes).expect("provider dashboard html");
    assert!(provider_html.contains("Machines and serving routes"));
    assert!(provider_html.contains("No machine routes yet"));
    assert!(!provider_html.contains("mayhem/routed-test"));

    let (status, _, network_bytes) = browser
        .request(
            app.clone(),
            Method::GET,
            &network_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let network_html = String::from_utf8(network_bytes).expect("network dashboard html");
    assert!(network_html.contains("reasoning_effort:low|high|xhigh"));
    assert!(network_html.contains("data-evidence-url"));

    let (status, _, evidence_bytes) = browser
        .request(
            app,
            Method::GET,
            &evidence_path,
            None,
            &[("host", "127.0.0.1:11435"), ("accept", "application/json")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let evidence: Value = serde_json::from_slice(&evidence_bytes).expect("model evidence json");
    assert_eq!(
        evidence["raw"]["catalog_model"]["mayhem"]["speciality_calibrations"]["nvfp4"]
            ["reasoning_effort"]["high"]["output_tokens_min"],
        2_000
    );
    assert_eq!(
        evidence["raw"]["catalog_model"]["mayhem"]["route_candidates"][0]["served_specialities"]
            ["reasoning_effort"],
        json!(["low", "high"])
    );
    assert_no_external_urls(&user_html);
    assert_no_external_urls(&provider_html);
    assert_no_external_urls(&network_html);

    if let Some(dir) = std::env::var_os("MAYHEM_DASHBOARD_VISUAL_DIR") {
        fs::create_dir_all(&dir).expect("create dashboard visual dir");
        fs::write(
            PathBuf::from(&dir).join("mayhem-user-effort.html"),
            user_html,
        )
        .expect("write user dashboard visual html");
        fs::write(
            PathBuf::from(&dir).join("mayhem-provider-effort.html"),
            provider_html,
        )
        .expect("write provider dashboard visual html");
        fs::write(
            PathBuf::from(&dir).join("mayhem-network-effort.html"),
            network_html,
        )
        .expect("write network dashboard visual html");
    }
}

#[tokio::test]
async fn models_endpoint_preserves_catalog_tier_counts_as_registered_evidence() {
    let embedding_contract =
        mayhem_proto::endpoint_family_contract_template(mayhem_proto::ENDPOINT_OPENAI_EMBEDDINGS)
            .unwrap();
    let catalog = json!({
        "models": [{
            "model_id": "mayhem/tier2-model",
            "model_class": "embedding",
            "caps": { "tools": true, "json": true, "ctx_max": 4096, "vision": false },
            "adapter": {
                "endpoint_families": [embedding_contract],
                "chat_template_id": "none",
                "tool_call_strategy": "none",
                "reasoning_passthrough": "strip",
                "modality_set": ["embedding"]
            },
            "price_ref_au": {
                "denom": "au_usd",
                "ver": 1,
                "rate_map": [
                    { "unit": "input_token", "per_unit_au": "10", "granularity": 1000 }
                ]
            },
            "attestation_tiers": { "T1": 1, "T2": 2 }
        }]
    });
    let state = GatewayState::from_catalog_json(&catalog.to_string()).expect("catalog parses");
    let app = openai_router(state);

    let (status, body) = json_request(app, Method::GET, "/v1/models", Value::Null).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"][0]["id"], "mayhem/tier2-model");
    assert_eq!(body["data"][0]["mayhem"]["model_class"], "embedding");
    assert!(body["data"][0]["mayhem"]["attestation_tiers"]
        .as_object()
        .is_some_and(|tiers| tiers.is_empty()));
    assert_eq!(
        body["data"][0]["mayhem"]["registered_attestation_tiers"]["T1"],
        1
    );
    assert_eq!(
        body["data"][0]["mayhem"]["registered_attestation_tiers"]["T2"],
        2
    );
    assert!(
        body["data"][0]["mayhem"]["registered_attestation_tier_labels"]["T2"]
            .as_str()
            .expect("tier 2 label")
            .contains("TPM EK / Apple App Attest / NVIDIA GB10")
    );
}

#[tokio::test]
async fn embeddings_endpoint_uses_routed_engine_and_records_receipt() {
    let state = test_gateway_state_from_models(vec![routed_embedding_test_model()])
        .with_session_backend(Arc::new(EmbeddingDirectSessionBackend));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "admin/embed-fixture",
        "input": ["alpha", "beta"],
        "dimensions": 3,
        "encoding_format": "float"
    });

    let (status, body) = json_request(app, Method::POST, "/v1/embeddings", request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "list");
    assert_eq!(body["model"], "admin/embed-fixture");
    assert_eq!(body["data"].as_array().expect("embedding data").len(), 2);
    assert_eq!(body["data"][0]["object"], "embedding");
    let first_value = body["data"][0]["embedding"][0]
        .as_f64()
        .expect("embedding value");
    assert!((first_value - 0.12).abs() < 0.0001);
    assert_eq!(body["usage"]["prompt_tokens"], 2);
    assert_eq!(body["usage"]["total_tokens"], 2);
    assert_eq!(body["mayhem"]["backend"], "test-embedding-direct-session");
    assert_eq!(body["mayhem"]["direct_session"], true);
    assert_eq!(body["mayhem"]["receipt"]["rail"], "fiat");

    let receipts = state.receipts();
    assert_eq!(receipts.len(), 1);
    let receipt = &receipts[0].receipt;
    assert_eq!(receipt.body.model_id, "admin/embed-fixture");
    assert_eq!(receipt.body.price_ver, 3);
    assert_eq!(receipt.body.usage.input_tokens(), 2);
    assert_eq!(receipt.body.usage.output_tokens(), 0);
    assert_eq!(receipt.body.au_owed_cum, 1);
}

#[tokio::test]
async fn automatic_embedding_cosine_probe_records_pass() {
    let expected = vec![0.12, 0.34, 0.56];
    let state = test_gateway_state_from_models(vec![routed_embedding_test_model()])
        .with_canary_registry(test_embedding_canary_registry(expected))
        .with_canary_probe_policy(GatewayCanaryProbePolicy::every_session_for_tests())
        .with_session_backend(Arc::new(EmbeddingDirectSessionBackend));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "admin/embed-fixture",
        "input": "user embedding",
        "encoding_format": "float"
    });

    let (status, body) = json_request(app, Method::POST, "/v1/embeddings", request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mayhem"]["backend"], "test-embedding-direct-session");
    assert_eq!(state.receipts().len(), 2);
    let probes = state.probes();
    assert_eq!(probes.len(), 1);
    let probe = &probes[0];
    assert_eq!(probe.verification_method, "embedding_cosine");
    assert!(probe.pass);
    assert_eq!(probe.match_bps, 10_000);
    assert_eq!(probe.reputation_event_kind, ReputationEventKind::ProbeOk);
    assert_eq!(probe.evidence["receipts"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn embeddings_endpoint_supports_base64_float32_encoding() {
    let state = test_gateway_state_from_models(vec![routed_embedding_test_model()])
        .with_session_backend(Arc::new(EmbeddingDirectSessionBackend));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "admin/embed-fixture",
        "input": ["alpha", "beta"],
        "dimensions": 3,
        "encoding_format": "base64"
    });

    let (status, body) = json_request(app, Method::POST, "/v1/embeddings", request).await;

    assert_eq!(status, StatusCode::OK);
    let encoded = body["data"][0]["embedding"]
        .as_str()
        .expect("base64 embedding payload");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("embedding is base64");
    assert_eq!(bytes.len(), 3 * 4);
    let values = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect::<Vec<_>>();
    assert!((values[0] - 0.12).abs() < 0.0001);
    assert!((values[1] - 0.34).abs() < 0.0001);
    assert!((values[2] - 0.56).abs() < 0.0001);
    assert_eq!(state.receipts().len(), 1);
}

#[tokio::test]
async fn embeddings_endpoint_rejects_non_embedding_model() {
    let model = first_model_id().await;
    let (status, body) = json_request(
        test_app(),
        Method::POST,
        "/v1/embeddings",
        json!({ "model": model, "input": "hello" }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["param"], "model");
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("does not expose endpoint family openai_embeddings"));
}

#[tokio::test]
async fn image_generation_endpoint_uses_routed_engine_and_records_receipt() {
    let state = test_gateway_state_from_models(vec![routed_image_generation_test_model()])
        .with_session_backend(Arc::new(ImageGenerationDirectSessionBackend));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "admin/image-fixture",
        "prompt": "a red cube",
        "n": 1,
        "size": "64x64",
        "steps": 3,
        "cfg_scale": 1.25,
        "seed": 9,
        "response_format": "b64_json"
    });

    let (status, body) = json_request(app, Method::POST, "/v1/images/generations", request).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["object"], "images.response");
    assert_eq!(body["model"], "admin/image-fixture");
    assert_eq!(body["data"].as_array().expect("image data").len(), 1);
    let encoded = body["data"][0]["b64_json"]
        .as_str()
        .expect("b64_json image");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("image is base64"),
        b"\x89PNG mayhem image"
    );
    assert_eq!(body["usage"][USAGE_IMAGE], 1);
    assert_eq!(body["usage"][USAGE_STEP], 3);
    assert_eq!(body["mayhem"]["backend"], "test-image-direct-session");
    assert_eq!(body["mayhem"]["direct_session"], true);
    assert_eq!(body["mayhem"]["receipt"]["rail"], "fiat");

    let receipts = state.receipts();
    assert_eq!(receipts.len(), 1);
    let receipt = &receipts[0].receipt;
    assert_eq!(receipt.body.model_id, "admin/image-fixture");
    assert_eq!(receipt.body.price_ver, 4);
    assert_eq!(receipt.body.usage.get(USAGE_IMAGE), 1);
    assert_eq!(receipt.body.usage.get(USAGE_STEP), 3);
    assert_eq!(receipt.body.au_owed_cum, 506);
}

#[tokio::test]
async fn image_job_reconnect_retrieves_once_without_rebilling_or_idempotency_conflicts() {
    let state = test_gateway_state_from_models(vec![routed_image_generation_test_model()])
        .with_session_backend(Arc::new(ImageGenerationDirectSessionBackend));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "admin/image-fixture",
        "prompt": "recover this image",
        "n": 1,
        "size": "64x64",
        "steps": 3,
        "response_format": "b64_json"
    });
    let (status, headers, _) = raw_request_with_headers(
        app.clone(),
        Method::POST,
        "/v1/images/generations",
        Some(request.clone()),
        &[("idempotency-key", "image-reconnect-1")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let job_id = headers["x-mayhem-job-id"]
        .to_str()
        .expect("job id header")
        .to_owned();
    assert_eq!(state.receipts().len(), 1);

    let job_uri = format!("/v1/jobs/{job_id}");
    let (job_status, job) = json_request(app.clone(), Method::GET, &job_uri, json!({})).await;
    assert_eq!(job_status, StatusCode::OK, "{job}");
    assert_eq!(job["status"], "completed");
    assert_eq!(job["endpoint_family"], "openai_image_generations");
    let artifact_uri = job["artifacts"][0]["content_url"]
        .as_str()
        .expect("artifact content URL")
        .to_owned();

    let result_uri = format!("{job_uri}/result");
    let (result_status, result) =
        json_request(app.clone(), Method::GET, &result_uri, json!({})).await;
    assert_eq!(result_status, StatusCode::OK, "{result}");
    assert_eq!(result["result"]["kind"], "image");
    assert_eq!(result["result"]["usage"][USAGE_IMAGE], 1);

    let (artifact_status, artifact_headers, artifact) =
        raw_request(app.clone(), Method::GET, &artifact_uri, None).await;
    assert_eq!(artifact_status, StatusCode::OK);
    assert_eq!(artifact_headers["content-type"], "image/png");
    assert_eq!(artifact, b"\x89PNG mayhem image");
    assert_eq!(state.receipts().len(), 1, "retrieval must never bill");

    let (replay_status, replay) = json_request_with_headers(
        app.clone(),
        Method::POST,
        "/v1/images/generations",
        request,
        &[("idempotency-key", "image-reconnect-1")],
    )
    .await;
    assert_eq!(replay_status, StatusCode::OK, "{replay}");
    assert_eq!(replay["id"], job_id);
    assert_eq!(replay["status"], "completed");
    assert_eq!(state.receipts().len(), 1, "idempotent replay must not bill");

    let (conflict_status, conflict) = json_request_with_headers(
        app,
        Method::POST,
        "/v1/images/generations",
        json!({
            "model": "admin/image-fixture",
            "prompt": "a different request",
            "n": 1,
            "size": "64x64",
            "steps": 3,
            "response_format": "b64_json"
        }),
        &[("idempotency-key", "image-reconnect-1")],
    )
    .await;
    assert_eq!(conflict_status, StatusCode::CONFLICT, "{conflict}");
    assert!(conflict["error"]["message"]
        .as_str()
        .expect("conflict message")
        .contains("different request"));
}

#[tokio::test]
async fn automatic_seed_perceptual_hash_probe_records_image_mismatch() {
    let expected_image = png_average_hash_fixture(false);
    let substituted_image = png_average_hash_fixture(true);
    let expected_hash = image_average_hash_hex(&expected_image).expect("expected image hash");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![routed_image_generation_test_model()])
        .with_canary_registry(test_image_canary_registry(expected_hash.clone()))
        .with_canary_probe_policy(GatewayCanaryProbePolicy::every_session_for_tests())
        .with_session_backend(Arc::new(ImageCanarySessionBackend {
            user_bytes: expected_image,
            canary_bytes: substituted_image,
            requests: requests.clone(),
        }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "admin/image-fixture",
        "prompt": "a user image",
        "n": 1,
        "size": "64x64",
        "steps": 1,
        "response_format": "b64_json"
    });

    let (status, body) = json_request(app, Method::POST, "/v1/images/generations", request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mayhem"]["backend"], "test-image-canary-session");
    assert_eq!(
        state.receipts().len(),
        2,
        "automatic image probe state: {:?}",
        state.probes()
    );
    let seen_requests = requests.lock().expect("image request lock").clone();
    assert_eq!(seen_requests.len(), 2);
    assert_eq!(seen_requests[0].prompt, "a user image");
    assert_eq!(seen_requests[1].prompt, "fixed image canary");
    assert_eq!(seen_requests[1].size.as_deref(), Some("64x64"));
    assert_eq!(seen_requests[1].seed, Some(7));

    let probes = state.probes();
    assert_eq!(probes.len(), 1);
    let probe = &probes[0];
    assert_eq!(probe.verification_method, "seed_perceptual_hash");
    assert!(!probe.pass);
    assert_eq!(probe.reputation_event_kind, ReputationEventKind::ProbeFail);
    assert_eq!(
        probe.probe_command["verification_method"],
        "seed_perceptual_hash"
    );
    assert_eq!(probe.probe_command["pass"], false);
    assert_eq!(
        probe.evidence["evidence"]["catalog_expected_perceptual_hashes"]["fixed-image"].as_str(),
        Some(expected_hash.as_str())
    );
    assert_ne!(
        probe.evidence["evidence"]["observed_perceptual_hashes"]["fixed-image"].as_str(),
        Some(expected_hash.as_str())
    );
    assert_eq!(
        probe.evidence["receipts"]
            .as_array()
            .expect("canary receipts")
            .len(),
        1
    );
}

#[tokio::test]
async fn image_generation_scales_step_usage_by_resolution_and_validates_size() {
    let state = test_gateway_state_from_models(vec![routed_image_generation_test_model()])
        .with_session_backend(Arc::new(ImageGenerationDirectSessionBackend));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "admin/image-fixture",
        "prompt": "a red cube",
        "n": 1,
        "size": "1024x1024",
        "steps": 3,
        "response_format": "b64_json"
    });

    let (status, body) =
        json_request(app.clone(), Method::POST, "/v1/images/generations", request).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["usage"][USAGE_IMAGE], 1);
    assert_eq!(body["usage"][USAGE_STEP], 12);
    let receipt = &state.receipts()[0].receipt;
    assert_eq!(receipt.body.usage.get(USAGE_STEP), 12);
    assert_eq!(receipt.body.au_owed_cum, 524);

    let (bad_status, bad_body) = json_request(
        app,
        Method::POST,
        "/v1/images/generations",
        json!({
            "model": "admin/image-fixture",
            "prompt": "too large",
            "size": "2048x2048",
            "steps": 3
        }),
    )
    .await;
    assert_eq!(bad_status, StatusCode::BAD_REQUEST);
    assert!(bad_body["error"]["message"]
        .as_str()
        .expect("bad size message")
        .contains("maximum"));
}

#[tokio::test]
async fn video_generation_supports_full_bounded_job_lifecycle_and_hf_overlap() {
    let state = test_gateway_state_from_models(vec![routed_video_generation_test_model()])
        .with_session_backend(Arc::new(ArtifactGenerationDirectSessionBackend));
    let app = openai_router(state.clone());
    let (status, created) = json_request(
        app.clone(),
        Method::POST,
        "/v1/videos",
        json!({
            "model": "admin/video-fixture",
            "prompt": "a small red square moving left",
            "seconds": "4",
            "size": "1280x720"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["object"], "video");
    assert_eq!(created["status"], "completed");
    assert_eq!(created["usage"][USAGE_VIDEO_SECOND], 4);
    assert_eq!(created["usage"][USAGE_FRAME], 96);
    assert_eq!(created["mayhem"]["receipt"]["rail"], "fiat");
    let video_id = created["id"].as_str().expect("video id").to_owned();

    let (list_status, list) = json_request(
        app.clone(),
        Method::GET,
        "/v1/videos?limit=1&order=desc",
        json!({}),
    )
    .await;
    assert_eq!(list_status, StatusCode::OK, "{list}");
    assert_eq!(list["object"], "list");
    assert_eq!(list["data"][0]["id"], video_id);
    assert_eq!(list["has_more"], false);

    let retrieve_uri = format!("/v1/videos/{video_id}");
    let (retrieve_status, retrieved) =
        json_request(app.clone(), Method::GET, &retrieve_uri, json!({})).await;
    assert_eq!(retrieve_status, StatusCode::OK, "{retrieved}");
    assert_eq!(retrieved["id"], video_id);

    let content_uri = format!("/v1/videos/{video_id}/content");
    let (content_status, content_headers, content) =
        raw_request(app.clone(), Method::GET, &content_uri, None).await;
    assert_eq!(content_status, StatusCode::OK);
    assert_eq!(content_headers["content-type"], "video/mp4");
    assert_eq!(content, b"mayhem-test-mp4");
    assert!(content_headers.contains_key("x-mayhem-artifact-blake3"));

    let (hf_status, hf_headers, hf_video) = raw_request(
        app.clone(),
        Method::POST,
        "/hf-inference/models/admin/video-fixture",
        Some(json!({
            "inputs": "a blue square moving right",
            "parameters": {
                "num_frames": 8,
                "fps": 4.0,
                "num_inference_steps": 2
            }
        })),
    )
    .await;
    assert_eq!(hf_status, StatusCode::OK);
    assert_eq!(hf_headers["content-type"], "video/mp4");
    assert_eq!(hf_video, b"mayhem-test-mp4");
    assert!(hf_headers.contains_key("x-mayhem-receipt"));

    let (delete_status, deleted) =
        json_request(app.clone(), Method::DELETE, &retrieve_uri, json!({})).await;
    assert_eq!(delete_status, StatusCode::OK, "{deleted}");
    assert_eq!(deleted["id"], video_id);
    assert_eq!(deleted["deleted"], true);

    let (missing_status, missing) = json_request(app, Method::GET, &retrieve_uri, json!({})).await;
    assert_eq!(missing_status, StatusCode::NOT_FOUND, "{missing}");
    assert_eq!(state.receipts().len(), 2);
}

#[tokio::test]
async fn audio_and_music_generation_routes_remain_distinct_and_hf_compatible() {
    let state = test_gateway_state_from_models(vec![
        routed_general_audio_generation_test_model(),
        routed_music_generation_test_model(),
    ])
    .with_session_backend(Arc::new(ArtifactGenerationDirectSessionBackend));
    let app = openai_router(state.clone());

    let (audio_status, audio) = json_request(
        app.clone(),
        Method::POST,
        "/v1/audio/generations",
        json!({
            "model": "admin/audio-generation-fixture",
            "prompt": "wind",
            "duration_seconds": 2,
            "response_format": "wav",
            "seed": 7
        }),
    )
    .await;
    assert_eq!(audio_status, StatusCode::OK, "{audio}");
    assert_eq!(audio["object"], "audio.generation");
    assert_eq!(audio["data"].as_array().unwrap().len(), 1);
    assert_eq!(audio["audio"], audio["data"][0]["audio"]);
    assert_eq!(audio["usage"][USAGE_INPUT_CHARACTER], 4);
    assert_eq!(audio["usage"][USAGE_AUDIO_SECOND], 2);
    assert_eq!(audio["mayhem"]["receipt"]["rail"], "fiat");

    let (music_status, music) = json_request(
        app.clone(),
        Method::POST,
        "/v1/music/generations",
        json!({
            "model": "admin/music-generation-fixture",
            "prompt": "piano",
            "duration_seconds": 10,
            "response_format": "wav",
            "seed": 9
        }),
    )
    .await;
    assert_eq!(music_status, StatusCode::OK, "{music}");
    assert_eq!(music["object"], "music.generation");
    assert_eq!(music["data"].as_array().unwrap().len(), 2);
    assert_eq!(music["music"], music["data"][0]["music"]);
    assert_eq!(music["usage"][USAGE_INPUT_CHARACTER], 77);
    assert_eq!(music["usage"][USAGE_AUDIO_SECOND], 20);
    assert_eq!(music["mayhem"]["receipt"]["rail"], "fiat");

    let (hf_status, hf_headers, hf_audio) = raw_request(
        app,
        Method::POST,
        "/hf-inference/models/admin/audio-generation-fixture",
        Some(json!({
            "inputs": "rain",
            "parameters": {"duration_seconds": 2}
        })),
    )
    .await;
    assert_eq!(hf_status, StatusCode::OK);
    assert_eq!(hf_headers["content-type"], "audio/wav");
    assert_eq!(hf_headers["x-mayhem-sampling-rate"], "16000");
    assert_eq!(hf_audio, wav_bytes_for_duration_seconds(2));
    assert!(hf_headers.contains_key("x-mayhem-receipt"));
    assert_eq!(state.receipts().len(), 3);
}

#[tokio::test]
async fn music_generation_normalizes_repaint_surface_and_returns_every_artifact() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut model = routed_music_generation_test_model();
    model.mayhem.model_class = "general-audio-foundation-model".to_owned();
    let state = test_gateway_state_from_models(vec![model])
        .with_dev_session_shim()
        .with_session_backend(Arc::new(ArtifactGenerationRecordingBackend {
            requests: requests.clone(),
        }));
    let app = openai_router(state);
    let source_bytes = tiny_wav_bytes(16_000);
    let reference_bytes = tiny_wav_bytes(8_000);
    let mut request_body = json!({
        "model": "admin/music-generation-fixture",
        "caption": "a bright synth-pop chorus",
        "lyrics": "[Chorus]\nMayhem in the moonlight",
        "instrumental": false,
        "style": "glossy",
        "genre": "synth-pop",
        "tags": [" bright ", "anthemic", "night drive"],
        "vocal_language": "en",
        "bpm": 124,
        "keyscale": "F# minor",
        "timesignature": "4/4",
        "duration": 12.25,
        "inference_steps": 8,
        "guidance_scale": 7.0,
        "seeds": [42, 43],
    });
    for controls in [
        json!({
            "task_type": "repaint",
            "inference_method": "ode",
            "shift": 1.5,
            "use_adg": true,
            "cfg_interval_start": 0.1,
            "cfg_interval_end": 0.9,
        }),
        json!({
            "repainting_start": 0.25,
            "repainting_end": 1.75,
            "repaint_strength": 0.6,
            "chunk_mask_mode": "explicit",
            "repaint_mode": "balanced",
            "audio_cover_strength": 0.7,
            "coverNoiseStrength": 0.2,
            "sampler_mode": "heun",
            "velocity_norm_threshold": 1.0,
            "velocity_ema_factor": 0.1,
            "dcw_enabled": true,
            "dcw_mode": "double",
            "dcw_scaler": 0.04,
            "dcw_high_scaler": 0.01,
            "dcw_wavelet": "db2",
        }),
        json!({
            "enable_normalization": true,
            "normalization_db": -2.0,
            "fade_in_duration": 0.2,
            "fade_out_duration": 0.3,
            "latent_shift": 0.1,
            "latent_rescale": 1.1,
            "retake_seed": 99,
            "retake_variance": 0.2,
            "audio_format": "mp3",
            "mp3_bitrate": "320k",
            "mp3_sample_rate": 44100,
            "batch_size": 2,
        }),
        json!({
            "src_audio": format!(
                "data:audio/wav;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(&source_bytes)
            ),
            "melody": format!(
                "data:audio/wav;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(&reference_bytes)
            ),
        }),
    ] {
        request_body
            .as_object_mut()
            .expect("music request fixture object")
            .extend(
                controls
                    .as_object()
                    .expect("music controls fixture object")
                    .clone(),
            );
    }

    let (status, response) =
        json_request(app, Method::POST, "/v1/music/generations", request_body).await;

    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["object"], "music.generation");
    assert_eq!(response["data"].as_array().unwrap().len(), 2);
    assert_eq!(response["mayhem"]["artifacts"].as_array().unwrap().len(), 2);
    assert_eq!(response["music"], response["data"][0]["music"]);
    assert_eq!(response["usage"][USAGE_AUDIO_SECOND], 26);

    let requests = requests.lock().expect("artifact generation records");
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.model, "admin/music-generation-fixture");
    assert_eq!(request.prompt, "a bright synth-pop chorus");
    assert_eq!(request.duration_seconds, 13);
    assert_eq!(request.step_count, 8);
    assert_eq!(request.artifact_count, 2);
    assert_eq!(request.response_format, "mp3");
    let signed = &request.contract_request;
    assert_eq!(signed["prompt"], "a bright synth-pop chorus");
    assert_eq!(signed["style"], "glossy");
    assert_eq!(signed["genre"], "synth-pop");
    assert_eq!(signed["tags"], "bright, anthemic, night drive");
    assert_eq!(signed["language"], "en");
    assert_eq!(signed["key"], "F# minor");
    assert_eq!(signed["time_signature"], "4/4");
    assert_eq!(signed["duration_seconds"], 12.25);
    assert_eq!(signed["steps"], 8);
    assert_eq!(signed["n"], 2);
    assert_eq!(signed["seeds"], json!([42, 43]));
    assert!(signed.get("custom_timesteps").is_none());
    assert_eq!(signed["infer_method"], "ode");
    assert_eq!(signed["repaint_start"], 0.25);
    assert_eq!(signed["repaint_end"], 1.75);
    assert_eq!(signed["repaint_strength"], 0.6);
    assert_eq!(signed["chunk_mask_mode"], "explicit");
    assert_eq!(signed["repaint_mode"], "balanced");
    assert_eq!(signed["cover_strength"], 0.7);
    assert_eq!(signed["cover_noise_strength"], 0.2);
    assert_eq!(signed["sampler"], "heun");
    assert_eq!(signed["velocity_norm_threshold"], 1.0);
    assert_eq!(signed["velocity_ema_factor"], 0.1);
    assert_eq!(signed["dcw_enabled"], true);
    assert_eq!(signed["dcw_mode"], "double");
    assert_eq!(signed["dcw_scaler"], 0.04);
    assert_eq!(signed["dcw_high_scaler"], 0.01);
    assert_eq!(signed["dcw_wavelet"], "db2");
    assert_eq!(signed["enable_normalization"], true);
    assert_eq!(signed["normalization_db"], -2.0);
    assert_eq!(signed["fade_in_duration"], 0.2);
    assert_eq!(signed["fade_out_duration"], 0.3);
    assert_eq!(signed["latent_shift"], 0.1);
    assert_eq!(signed["latent_rescale"], 1.1);
    assert_eq!(signed["retake_seed"], 99);
    assert_eq!(signed["retake_variance"], 0.2);
    assert_eq!(signed["response_format"], "mp3");
    assert_eq!(signed["mp3_bitrate"], "320k");
    assert_eq!(signed["mp3_sample_rate"], 44100);
    assert_eq!(signed["source_audio"]["encoding"], "base64");
    assert_eq!(signed["source_audio"]["content_type"], "audio/wav");
    assert_eq!(
        signed["source_audio"]["data"],
        base64::engine::general_purpose::STANDARD.encode(&source_bytes)
    );
    assert_eq!(signed["reference_audio"]["encoding"], "base64");
    assert_eq!(signed["reference_audio"]["content_type"], "audio/wav");
    assert_eq!(
        signed["reference_audio"]["data"],
        base64::engine::general_purpose::STANDARD.encode(&reference_bytes)
    );
    for alias in [
        "caption",
        "vocal_language",
        "keyscale",
        "timesignature",
        "duration",
        "inference_steps",
        "coverNoiseStrength",
        "sampler_mode",
        "batch_size",
        "src_audio",
        "melody",
    ] {
        assert!(
            signed.get(alias).is_none(),
            "alias {alias} was not normalized"
        );
    }
}

#[tokio::test]
async fn music_generation_normalizes_lm_and_audio_code_aliases_in_valid_requests() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![routed_music_generation_test_model()])
        .with_dev_session_shim()
        .with_session_backend(Arc::new(ArtifactGenerationRecordingBackend {
            requests: requests.clone(),
        }));
    let app = openai_router(state);

    let (sample_status, sample_response) = json_request(
        app.clone(),
        Method::POST,
        "/v1/music/generations",
        json!({
            "model": "admin/music-generation-fixture",
            "caption": "an upbeat night-drive chorus",
            "sampleMode": true,
            "description": "bright synth-pop with a large chorus",
            "useFormat": true,
            "thinking": true,
            "temperature": 0.85,
            "lm_cfg": 2.0,
            "top_k": 40,
            "top_p": 0.9,
            "negative_prompt": "noise",
            "cot_metas": true,
            "cot_caption": true,
            "cot_language": true,
            "use_constrained_decoding": true
        }),
    )
    .await;
    assert_eq!(sample_status, StatusCode::OK, "{sample_response}");

    let (codes_status, codes_response) = json_request(
        app,
        Method::POST,
        "/v1/music/generations",
        json!({
            "model": "admin/music-generation-fixture",
            "prompt": "continue these semantic audio codes",
            "task_type": "cover",
            "audioCodeString": "<|audio_code_1|><|audio_code_2|>"
        }),
    )
    .await;
    assert_eq!(codes_status, StatusCode::OK, "{codes_response}");

    let requests = requests.lock().expect("artifact generation records");
    assert_eq!(requests.len(), 2);
    let sample = &requests[0].contract_request;
    assert_eq!(sample["sample_mode"], true);
    assert_eq!(
        sample["sample_query"],
        "bright synth-pop with a large chorus"
    );
    assert_eq!(sample["use_format"], true);
    assert_eq!(sample["thinking"], true);
    assert_eq!(sample["lm_temperature"], 0.85);
    assert_eq!(sample["lm_cfg_scale"], 2.0);
    assert_eq!(sample["lm_top_k"], 40);
    assert_eq!(sample["lm_top_p"], 0.9);
    assert_eq!(sample["lm_negative_prompt"], "noise");
    assert_eq!(sample["use_cot_metas"], true);
    assert_eq!(sample["use_cot_caption"], true);
    assert_eq!(sample["use_cot_language"], true);
    assert_eq!(sample["constrained_decoding"], true);
    let codes = &requests[1].contract_request;
    assert_eq!(codes["audio_codes"], "<|audio_code_1|><|audio_code_2|>");
    for alias in [
        "sampleMode",
        "description",
        "useFormat",
        "temperature",
        "lm_cfg",
        "top_k",
        "top_p",
        "negative_prompt",
        "cot_metas",
        "cot_caption",
        "cot_language",
        "use_constrained_decoding",
    ] {
        assert!(
            sample.get(alias).is_none(),
            "alias {alias} was not normalized"
        );
    }
    assert!(codes.get("audioCodeString").is_none());
}

#[tokio::test]
async fn music_generation_normalizes_creator_source_and_cover_aliases() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![routed_music_generation_test_model()])
        .with_dev_session_shim()
        .with_session_backend(Arc::new(ArtifactGenerationRecordingBackend {
            requests: requests.clone(),
        }));
    let app = openai_router(state);
    let source = format!(
        "data:audio/wav;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(tiny_wav_bytes(16_000))
    );
    let reference = format!(
        "data:audio/wav;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(tiny_wav_bytes(8_000))
    );

    let (status, response) = json_request(
        app,
        Method::POST,
        "/v1/music/generations",
        json!({
            "model": "admin/music-generation-fixture",
            "no_fsq": true,
            "ctx_audio": source,
            "ref_audio": reference,
            "flow_edit": true,
            "flow_edit_source_caption": "the original arrangement"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{response}");
    let requests = requests.lock().expect("artifact generation records");
    assert_eq!(requests.len(), 1);
    let signed = &requests[0].contract_request;
    assert_eq!(signed["task_type"], "cover-nofsq");
    assert_eq!(signed["flow_edit_morph"], true);
    assert_eq!(signed["source_audio"]["content_type"], "audio/wav");
    assert_eq!(signed["reference_audio"]["content_type"], "audio/wav");
    for alias in ["no_fsq", "ctx_audio", "ref_audio", "flow_edit"] {
        assert!(
            signed.get(alias).is_none(),
            "alias {alias} was not normalized"
        );
    }
}

#[tokio::test]
async fn music_generation_preserves_signed_flow_edit_controls() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![routed_music_generation_test_model()])
        .with_dev_session_shim()
        .with_session_backend(Arc::new(ArtifactGenerationRecordingBackend {
            requests: requests.clone(),
        }));
    let app = openai_router(state);
    let source_audio = format!(
        "data:audio/wav;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(tiny_wav_bytes(16_000))
    );

    let (status, response) = json_request(
        app,
        Method::POST,
        "/v1/music/generations",
        json!({
            "model": "admin/music-generation-fixture",
            "caption": "reshape this arrangement",
            "task_type": "cover",
            "source_audio": source_audio,
            "flow_edit_morph": true,
            "flow_edit_source_caption": "a sparse original arrangement",
            "flow_edit_source_lyrics": "original lyrics",
            "flow_edit_n_min": 0.1,
            "flow_edit_n_max": 0.9,
            "flow_edit_n_avg": 1
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{response}");
    let requests = requests.lock().expect("artifact generation records");
    assert_eq!(requests.len(), 1);
    let signed = &requests[0].contract_request;
    assert_eq!(signed["flow_edit_morph"], true);
    assert_eq!(
        signed["flow_edit_source_caption"],
        "a sparse original arrangement"
    );
    assert_eq!(signed["flow_edit_source_lyrics"], "original lyrics");
    assert_eq!(signed["flow_edit_n_min"], 0.1);
    assert_eq!(signed["flow_edit_n_max"], 0.9);
    assert_eq!(signed["flow_edit_n_avg"], 1);
}

#[tokio::test]
async fn music_generation_uses_signed_defaults_and_ranges() {
    let model = routed_music_generation_test_model();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![model])
        .with_dev_session_shim()
        .with_session_backend(Arc::new(ArtifactGenerationRecordingBackend {
            requests: requests.clone(),
        }));
    let app = openai_router(state);

    let (default_status, defaulted) = json_request(
        app.clone(),
        Method::POST,
        "/v1/music/generations",
        json!({
            "model": "admin/music-generation-fixture",
            "prompt": "signed defaults"
        }),
    )
    .await;
    assert_eq!(default_status, StatusCode::OK, "{defaulted}");
    assert_eq!(defaulted["data"].as_array().unwrap().len(), 2);
    assert_eq!(defaulted["usage"][USAGE_INPUT_CHARACTER], 87);
    assert_eq!(defaulted["usage"][USAGE_AUDIO_SECOND], 2);

    let (range_status, rejected) = json_request(
        app,
        Method::POST,
        "/v1/music/generations",
        json!({
            "model": "admin/music-generation-fixture",
            "caption": "outside signed batch range",
            "batch": 9
        }),
    )
    .await;
    assert_eq!(range_status, StatusCode::BAD_REQUEST, "{rejected}");
    assert!(rejected.to_string().contains("n"));

    let requests = requests.lock().expect("artifact generation records");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].duration_seconds, 1);
    assert_eq!(requests[0].step_count, 50);
    assert_eq!(requests[0].artifact_count, 2);
    assert_eq!(requests[0].response_format, "flac");
    assert_eq!(requests[0].contract_request["guidance_scale"], 7.0);
    assert_eq!(requests[0].contract_request["thinking"], false);
    assert_eq!(requests[0].contract_request["use_cot_caption"], true);
}

#[tokio::test]
async fn music_generation_accepts_task_specific_promptless_requests() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![routed_music_generation_test_model()])
        .with_dev_session_shim()
        .with_session_backend(Arc::new(ArtifactGenerationRecordingBackend {
            requests: requests.clone(),
        }));
    let app = openai_router(state);

    let (lyrics_status, lyrics_response) = json_request(
        app.clone(),
        Method::POST,
        "/v1/music/generations",
        json!({
            "model": "admin/music-generation-fixture",
            "task_type": "text2music",
            "lyrics": "[Verse]\nLyrics carry this request"
        }),
    )
    .await;
    assert_eq!(lyrics_status, StatusCode::OK, "{lyrics_response}");

    let source_audio = format!(
        "data:audio/wav;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(tiny_wav_bytes(16_000))
    );
    for task_type in ["cover", "cover-nofsq", "repaint"] {
        let (status, response) = json_request(
            app.clone(),
            Method::POST,
            "/v1/music/generations",
            json!({
                "model": "admin/music-generation-fixture",
                "task_type": task_type,
                "source_audio": source_audio
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{task_type}: {response}");
    }

    let requests = requests.lock().expect("artifact generation records");
    assert_eq!(requests.len(), 4);
    for request in requests.iter() {
        assert_eq!(request.prompt, "");
        assert_eq!(request.contract_request["prompt"], "");
    }
    assert_eq!(
        requests[0].contract_request["lyrics"],
        "[Verse]\nLyrics carry this request"
    );
    for request in &requests[1..] {
        assert_eq!(
            request.contract_request["source_audio"]["content_type"],
            "audio/wav"
        );
    }
}

#[tokio::test]
async fn music_generation_rejects_task_requests_missing_required_inputs() {
    let model = routed_music_generation_test_model();
    let contract = model
        .mayhem
        .adapter
        .endpoint_families
        .iter()
        .find(|contract| contract.family == mayhem_proto::ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
        .expect("music endpoint contract");
    assert!(!contract
        .required_request_attributes
        .iter()
        .any(|attribute| matches!(attribute.as_str(), "prompt" | "caption")));

    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![model])
        .with_dev_session_shim()
        .with_session_backend(Arc::new(ArtifactGenerationRecordingBackend {
            requests: requests.clone(),
        }));
    let app = openai_router(state);

    let (text_status, text_error) = json_request(
        app.clone(),
        Method::POST,
        "/v1/music/generations",
        json!({
            "model": "admin/music-generation-fixture",
            "task_type": "text2music"
        }),
    )
    .await;
    assert_eq!(text_status, StatusCode::BAD_REQUEST, "{text_error}");
    assert!(text_error.to_string().contains("lyrics"), "{text_error}");

    for task_type in ["cover", "cover-nofsq", "repaint"] {
        let (status, error) = json_request(
            app.clone(),
            Method::POST,
            "/v1/music/generations",
            json!({
                "model": "admin/music-generation-fixture",
                "task_type": task_type,
                "caption": "text cannot replace the required source"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{task_type}: {error}");
        assert!(
            error.to_string().contains("source_audio"),
            "{task_type}: {error}"
        );
    }
    assert!(requests
        .lock()
        .expect("artifact generation records")
        .is_empty());
}

#[tokio::test]
async fn music_generation_rejects_unsupported_controls_and_server_paths() {
    let mut model = routed_music_generation_test_model();
    let contract = model
        .mayhem
        .adapter
        .endpoint_families
        .iter_mut()
        .find(|contract| contract.family == mayhem_proto::ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
        .expect("music endpoint contract");
    contract
        .request_attributes
        .retain(|path| path != "cover_noise_strength");
    contract
        .request_attribute_specs
        .remove("cover_noise_strength");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![model])
        .with_dev_session_shim()
        .with_session_backend(Arc::new(ArtifactGenerationRecordingBackend {
            requests: requests.clone(),
        }));
    let app = openai_router(state);

    let (unsupported_status, unsupported) = json_request(
        app.clone(),
        Method::POST,
        "/v1/music/generations",
        json!({
            "model": "admin/music-generation-fixture",
            "prompt": "unsupported capability",
            "cover_noise_strength": 0.2
        }),
    )
    .await;
    assert_eq!(unsupported_status, StatusCode::BAD_REQUEST, "{unsupported}");
    assert!(unsupported.to_string().contains("cover_noise_strength"));

    let (alias_status, alias_error) = json_request(
        app.clone(),
        Method::POST,
        "/v1/music/generations",
        json!({
            "model": "admin/music-generation-fixture",
            "prompt": "ambiguous sampling query",
            "sample_query": "first query",
            "description": "second query"
        }),
    )
    .await;
    assert_eq!(alias_status, StatusCode::BAD_REQUEST, "{alias_error}");
    assert!(alias_error.to_string().contains("conflicting aliases"));

    let (negative_status, negative_error) = json_request(
        app.clone(),
        Method::POST,
        "/v1/music/generations",
        json!({
            "model": "admin/music-generation-fixture",
            "prompt": "ambiguous negative prompt",
            "negative_prompt": "first exclusion",
            "lm_negative_prompt": "second exclusion"
        }),
    )
    .await;
    assert_eq!(negative_status, StatusCode::BAD_REQUEST, "{negative_error}");
    assert!(negative_error.to_string().contains("conflicting aliases"));

    let (sampling_status, sampling_error) = json_request(
        app.clone(),
        Method::POST,
        "/v1/music/generations",
        json!({
            "model": "admin/music-generation-fixture",
            "prompt": "ambiguous temperature",
            "temperature": 0.8,
            "lm_temperature": 0.9
        }),
    )
    .await;
    assert_eq!(sampling_status, StatusCode::BAD_REQUEST, "{sampling_error}");
    assert!(sampling_error.to_string().contains("conflicting aliases"));

    let (path_status, path_error) = json_request(
        app,
        Method::POST,
        "/v1/music/generations",
        json!({
            "model": "admin/music-generation-fixture",
            "prompt": "forbidden path",
            "source_audio_path": "/srv/private/reference.wav"
        }),
    )
    .await;
    assert_eq!(path_status, StatusCode::BAD_REQUEST, "{path_error}");
    assert!(path_error.to_string().contains("forbidden"));
    assert!(requests
        .lock()
        .expect("artifact generation records")
        .is_empty());
}

#[tokio::test]
async fn selected_music_sft_does_not_expose_or_accept_unsupported_controls() {
    let model = routed_music_generation_test_model();
    let unsupported = [
        ("global_caption", json!("not supported by the selected SFT")),
        ("use_cot_lyrics", json!(true)),
        ("lm_repetition_penalty", json!(1.1)),
        ("repaint_latent_crossfade_frames", json!(12)),
        ("repaint_wav_crossfade_sec", json!(0.1)),
        ("typical_p", json!(0.9)),
        ("do_sample", json!(true)),
        ("max_new_tokens", json!(128)),
    ];
    let contract = model
        .mayhem
        .adapter
        .endpoint_families
        .iter()
        .find(|contract| contract.family == mayhem_proto::ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
        .expect("music endpoint contract");
    for (field, _) in &unsupported {
        assert!(
            !contract
                .request_attributes
                .iter()
                .any(|attribute| attribute == field),
            "selected SFT contract unexpectedly exposes {field}"
        );
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![model])
        .with_dev_session_shim()
        .with_session_backend(Arc::new(ArtifactGenerationRecordingBackend {
            requests: requests.clone(),
        }));
    let app = openai_router(state);

    for (field, value) in unsupported {
        let mut body = json!({
            "model": "admin/music-generation-fixture",
            "prompt": "unsupported selected-SFT control"
        });
        body.as_object_mut()
            .expect("music request fixture object")
            .insert(field.to_owned(), value);
        let (status, error) =
            json_request(app.clone(), Method::POST, "/v1/music/generations", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {error}");
        assert!(error.to_string().contains(field), "{field}: {error}");
    }
    assert!(requests
        .lock()
        .expect("artifact generation records")
        .is_empty());
}

#[tokio::test]
async fn hf_text_to_audio_maps_caption_and_arbitrary_inline_audio_generically() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![routed_general_audio_generation_test_model()])
        .with_dev_session_shim()
        .with_session_backend(Arc::new(ArtifactGenerationRecordingBackend {
            requests: requests.clone(),
        }));
    let app = openai_router(state);
    let source = format!(
        "data:audio/x-custom;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(b"not a wav or flac")
    );

    let (status, headers, bytes) = raw_request(
        app,
        Method::POST,
        "/hf-inference/models/admin/audio-generation-fixture",
        Some(json!({
            "caption": "generic HF audio",
            "source_audio": source,
            "duration": 2,
            "temperature": 0.7
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "audio/wav");
    assert!(bytes.starts_with(b"RIFF"));
    let requests = requests.lock().expect("artifact generation records");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt, "generic HF audio");
    assert_eq!(requests[0].duration_seconds, 2);
    assert_eq!(requests[0].contract_request["inputs"], "generic HF audio");
    assert_eq!(
        requests[0].contract_request["parameters"]["generation_parameters"]["temperature"],
        0.7
    );
    assert_eq!(
        requests[0].contract_request["parameters"]["audio"]["content_type"],
        "audio/x-custom"
    );
    assert_eq!(
        requests[0].contract_request["parameters"]["audio"]["encoding"],
        "base64"
    );
    assert_eq!(
        requests[0].contract_request["parameters"]["audio"]["data"],
        base64::engine::general_purpose::STANDARD.encode(b"not a wav or flac")
    );
}

#[tokio::test]
async fn general_audio_controls_are_not_rewritten_as_music_lm_controls() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![routed_general_audio_generation_test_model()])
        .with_dev_session_shim()
        .with_session_backend(Arc::new(ArtifactGenerationRecordingBackend {
            requests: requests.clone(),
        }));
    let app = openai_router(state);

    let (status, response) = json_request(
        app,
        Method::POST,
        "/v1/audio/generations",
        json!({
            "model": "admin/audio-generation-fixture",
            "caption": "clean rain ambience",
            "negative_prompt": "speech",
            "temperature": 0.7,
            "top_k": 20,
            "top_p": 0.8,
            "duration_seconds": 2,
            "response_format": "wav"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{response}");
    let requests = requests.lock().expect("artifact generation records");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt, "clean rain ambience");
    assert_eq!(requests[0].contract_request["negative_prompt"], "speech");
    assert_eq!(requests[0].contract_request["temperature"], 0.7);
    assert_eq!(requests[0].contract_request["top_k"], 20);
    assert_eq!(requests[0].contract_request["top_p"], 0.8);
    for field in [
        "lm_negative_prompt",
        "lm_temperature",
        "lm_top_k",
        "lm_top_p",
    ] {
        assert!(requests[0].contract_request.get(field).is_none(), "{field}");
    }
}

#[tokio::test]
async fn image_generation_endpoint_real_sd_cli_records_receipt_when_enabled() {
    if std::env::var_os("MAYHEM_RUN_STABLE_DIFFUSION_CPP_REAL").is_none() {
        return;
    }
    let state = test_gateway_state_from_models(vec![routed_image_generation_test_model()])
        .with_session_backend(Arc::new(RealSdCliImageGenerationBackend));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "admin/image-fixture",
        "prompt": "a red cube on a white table, simple studio photo",
        "n": 1,
        "size": "512x512",
        "steps": 1,
        "cfg_scale": 1.0,
        "seed": 7,
        "response_format": "b64_json"
    });

    let (status, body) = json_request(app, Method::POST, "/v1/images/generations", request).await;

    assert_eq!(status, StatusCode::OK);
    let encoded = body["data"][0]["b64_json"]
        .as_str()
        .expect("b64_json image");
    let image = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("image is base64");
    assert!(image.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(image.len() > 1024);
    assert_eq!(body["usage"][USAGE_IMAGE], 1);
    assert_eq!(body["usage"][USAGE_STEP], 1);
    assert_eq!(body["mayhem"]["backend"], "test-real-sd-cli");
    assert_eq!(body["mayhem"]["receipt"]["rail"], "fiat");

    let receipts = state.receipts();
    assert_eq!(receipts.len(), 1);
    let receipt = &receipts[0].receipt;
    assert_eq!(receipt.body.usage.get(USAGE_IMAGE), 1);
    assert_eq!(receipt.body.usage.get(USAGE_STEP), 1);
    assert_eq!(receipt.body.au_owed_cum, 502);
}

#[tokio::test]
async fn audio_speech_endpoint_uses_routed_engine_and_records_receipt() {
    let state = test_gateway_state_from_models(vec![routed_audio_speech_test_model()])
        .with_session_backend(Arc::new(AudioSpeechDirectSessionBackend));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "admin/tts-fixture",
        "input": "hello speech",
        "voice": "launch",
        "response_format": "wav"
    });

    let (status, headers, bytes) =
        raw_request(app, Method::POST, "/v1/audio/speech", Some(request)).await;

    assert_eq!(status, StatusCode::OK);
    assert!(headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .contains("audio/wav"));
    assert_eq!(
        headers
            .get("x-mayhem-backend")
            .and_then(|value| value.to_str().ok()),
        Some("test-audio-speech-direct-session")
    );
    assert!(bytes.starts_with(b"RIFF"));
    let receipts = state.receipts();
    assert_eq!(receipts.len(), 1);
    let receipt = &receipts[0].receipt;
    assert_eq!(receipt.body.model_id, "admin/tts-fixture");
    assert_eq!(receipt.body.price_ver, 5);
    assert_eq!(receipt.body.usage.get(USAGE_INPUT_CHARACTER), 12);
    assert_eq!(receipt.body.usage.get(USAGE_AUDIO_SECOND), 1);
    assert_eq!(receipt.body.au_owed_cum, 112);
}

#[tokio::test]
async fn audio_speech_voice_clone_controls_are_signed_without_receipt_audio_leakage() {
    let state = test_gateway_state_from_models(vec![routed_audio_speech_test_model()])
        .with_session_backend(Arc::new(AudioSpeechDirectSessionBackend));
    let app = openai_router(state.clone());
    let reference_audio = base64::engine::general_purpose::STANDARD.encode(tiny_wav_bytes(16_000));
    let request = json!({
        "model": "admin/tts-fixture",
        "input": "clone this signed reference",
        "voice": "default",
        "response_format": "wav",
        "reference_audio": {
            "data": reference_audio,
            "encoding": "base64",
            "content_type": "audio/wav"
        },
        "exaggeration": 0.7,
        "cfg_weight": 0.3,
        "temperature": 0.8,
        "min_p": 0.05,
        "top_p": 1.0,
        "repetition_penalty": 1.2,
        "seed": 7
    });
    let contract =
        mayhem_proto::endpoint_family_contract_template(mayhem_proto::ENDPOINT_OPENAI_AUDIO_SPEECH)
            .unwrap();
    let normalized_request =
        mayhem_proto::materialize_endpoint_request_defaults(&contract, &request).unwrap();
    let expected_prompt_hash = endpoint_request_fingerprint(&normalized_request);

    let (status, _, bytes) =
        raw_request(app, Method::POST, "/v1/audio/speech", Some(request)).await;

    assert_eq!(status, StatusCode::OK);
    assert!(bytes.starts_with(b"RIFF"));
    let receipts = state.receipts();
    assert_eq!(receipts.len(), 1);
    let receipt = &receipts[0].receipt;
    assert_eq!(receipt.body.prompt_hash, expected_prompt_hash);
    assert!(!serde_json::to_string(receipt)
        .unwrap()
        .contains(&reference_audio));
}

#[tokio::test]
async fn automatic_audio_fingerprint_probe_records_pass() {
    let expected_audio = tiny_wav_bytes(16_000);
    let state = test_gateway_state_from_models(vec![routed_audio_speech_test_model()])
        .with_canary_registry(test_audio_fingerprint_canary_registry(audio_fingerprint(
            &expected_audio,
        )))
        .with_canary_probe_policy(GatewayCanaryProbePolicy::every_session_for_tests())
        .with_session_backend(Arc::new(AudioSpeechDirectSessionBackend));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "admin/tts-fixture",
        "input": "hello speech",
        "voice": "launch",
        "response_format": "wav"
    });

    let (status, _, bytes) =
        raw_request(app, Method::POST, "/v1/audio/speech", Some(request)).await;

    assert_eq!(status, StatusCode::OK);
    assert!(bytes.starts_with(b"RIFF"));
    assert_eq!(
        state.receipts().len(),
        2,
        "automatic audio probe state: {:?}",
        state.probes()
    );
    let probes = state.probes();
    assert_eq!(probes.len(), 1);
    let probe = &probes[0];
    assert_eq!(probe.verification_method, "audio_fingerprint");
    assert!(probe.pass);
    assert_eq!(probe.match_bps, 10_000);
    assert_eq!(probe.evidence["receipts"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn automatic_music_fingerprint_probe_uses_signed_music_surface() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let expected_audio = wav_bytes_for_duration_seconds(10);
    let state = test_gateway_state_from_models(vec![routed_music_generation_test_model()])
        .with_canary_registry(test_music_audio_fingerprint_canary_registry(
            audio_fingerprint(&expected_audio),
        ))
        .with_canary_probe_policy(GatewayCanaryProbePolicy::every_session_for_tests())
        .with_session_backend(Arc::new(ArtifactGenerationRecordingBackend {
            requests: requests.clone(),
        }));
    let app = openai_router(state.clone());

    let (status, body) = json_request(
        app,
        Method::POST,
        "/v1/music/generations",
        json!({
            "model": "admin/music-generation-fixture",
            "prompt": "user music request",
            "duration_seconds": 10,
            "response_format": "wav",
            "n": 1,
            "seed": 5
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let requests = requests.lock().expect("artifact generation records");
    assert_eq!(requests.len(), 2);
    let canary = &requests[1];
    assert_eq!(
        canary.endpoint_family,
        mayhem_proto::ENDPOINT_MAYHEM_MUSIC_GENERATIONS
    );
    assert_eq!(canary.output_modality, "audio");
    assert_eq!(canary.prompt, "fixed music canary");
    assert_eq!(canary.duration_seconds, 10);
    assert_eq!(canary.step_count, 50);
    assert_eq!(canary.artifact_count, 1);
    assert_eq!(canary.response_format, "wav");
    assert_eq!(
        canary.contract_request["lyrics"],
        "[Verse]\nCanonical proof"
    );
    assert_eq!(canary.contract_request["instrumental"], false);
    assert_eq!(canary.contract_request["bpm"], 120);
    assert_eq!(canary.contract_request["key"], "C major");
    assert_eq!(canary.contract_request["time_signature"], "4/4");
    assert_eq!(canary.contract_request["task_type"], "text2music");
    assert_eq!(canary.contract_request["thinking"], false);
    assert_eq!(canary.contract_request["guidance_scale"], 7);
    assert_eq!(canary.contract_request["seed"], 7);
    assert!(canary.contract_request.get("audio_b64").is_none());
    drop(requests);

    let probes = state.probes();
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0].verification_method, "audio_fingerprint");
    assert!(probes[0].pass, "music canary probe: {:?}", probes[0]);
    assert_eq!(probes[0].match_bps, 10_000);
    assert_eq!(state.receipts().len(), 2);
}

#[tokio::test]
async fn automatic_music_fingerprint_probe_rejects_extra_artifacts() {
    let expected_audio = wav_bytes_for_duration_seconds(10);
    let state = test_gateway_state_from_models(vec![routed_music_generation_test_model()])
        .with_canary_registry(test_music_audio_fingerprint_canary_registry(
            audio_fingerprint(&expected_audio),
        ))
        .with_canary_probe_policy(GatewayCanaryProbePolicy::every_session_for_tests())
        .with_session_backend(Arc::new(ExtraCanaryArtifactBackend));
    let app = openai_router(state.clone());

    let (status, body) = json_request(
        app,
        Method::POST,
        "/v1/music/generations",
        json!({
            "model": "admin/music-generation-fixture",
            "prompt": "user music request",
            "duration_seconds": 10,
            "response_format": "wav",
            "n": 1,
            "seed": 5
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let probes = state.probes();
    assert_eq!(probes.len(), 1);
    assert!(!probes[0].pass);
    assert_eq!(probes[0].match_bps, 0);
    assert_eq!(
        probes[0].evidence["reason"],
        "provider returned 2 audio canary artifact(s), expected 1"
    );
    assert_eq!(
        state.receipts().len(),
        1,
        "the rejected canary must not be metered"
    );
}

#[tokio::test]
async fn audio_transcription_endpoint_uses_routed_engine_and_records_receipt() {
    let state = test_gateway_state_from_models(vec![routed_audio_transcription_test_model()])
        .with_session_backend(Arc::new(AudioTranscriptionDirectSessionBackend));
    let app = openai_router(state.clone());
    let boundary = "mayhem-test-boundary";
    let audio = tiny_wav_bytes(32_000);
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nadmin/stt-fixture\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\njson\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"clip.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let (status, headers, bytes) = raw_bytes_request_with_headers(
        app,
        Method::POST,
        "/v1/audio/transcriptions",
        body,
        &[(
            "content-type",
            &format!("multipart/form-data; boundary={boundary}"),
        )],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .contains("application/json"));
    let body: Value = serde_json::from_slice(&bytes).expect("transcription JSON");
    assert_eq!(body["text"], "hello mayhem");
    assert_eq!(body["usage"][USAGE_AUDIO_SECOND], 2);
    assert_eq!(
        body["mayhem"]["backend"],
        "test-audio-transcription-direct-session"
    );
    assert_eq!(body["mayhem"]["receipt"]["rail"], "fiat");
    let receipts = state.receipts();
    assert_eq!(receipts.len(), 1);
    let receipt = &receipts[0].receipt;
    assert_eq!(receipt.body.model_id, "admin/stt-fixture");
    assert_eq!(receipt.body.price_ver, 6);
    assert_eq!(receipt.body.usage.get(USAGE_AUDIO_SECOND), 2);
    assert_eq!(receipt.body.au_owed_cum, 500);
}

#[tokio::test]
async fn audio_transcription_verbose_json_returns_requested_source_timestamps() {
    let state = test_gateway_state_from_models(vec![routed_audio_transcription_test_model()])
        .with_session_backend(Arc::new(AudioTranscriptionDirectSessionBackend));
    let app = openai_router(state);
    let (boundary, body) = audio_transcription_multipart(&[
        ("model", "admin/stt-fixture"),
        ("response_format", "verbose_json"),
        ("timestamp_granularities[]", "word"),
        ("timestamp_granularities[]", "segment"),
    ]);
    let content_type = format!("multipart/form-data; boundary={boundary}");

    let (status, headers, bytes) = raw_bytes_request_with_headers(
        app,
        Method::POST,
        "/v1/audio/transcriptions",
        body,
        &[("content-type", &content_type)],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(headers["content-type"]
        .to_str()
        .unwrap()
        .contains("application/json"));
    let body: Value = serde_json::from_slice(&bytes).expect("verbose transcription JSON");
    assert_eq!(body["task"], "transcribe");
    assert_eq!(body["text"], "hello mayhem");
    assert_eq!(body["language"], "en");
    assert_eq!(body["duration"], 2.0);
    assert_eq!(
        body["words"],
        json!([
            {"word": "hello", "start": 0.0, "end": 0.8},
            {"word": "mayhem", "start": 1.0, "end": 2.0}
        ])
    );
    assert_eq!(
        body["segments"],
        json!([
            {"id": 0, "text": "hello", "start": 0.0, "end": 0.9},
            {"id": 1, "text": "mayhem", "start": 1.0, "end": 2.0}
        ])
    );
}

#[tokio::test]
async fn audio_transcription_returns_native_text_srt_and_vtt_formats() {
    let state = test_gateway_state_from_models(vec![routed_audio_transcription_test_model()])
        .with_session_backend(Arc::new(AudioTranscriptionDirectSessionBackend));
    let app = openai_router(state);

    for (format, expected_content_type, expected_body) in [
        ("text", "text/plain", "hello mayhem".to_owned()),
        (
            "srt",
            "application/x-subrip",
            concat!(
                "1\n00:00:00,000 --> 00:00:00,900\nhello\n\n",
                "2\n00:00:01,000 --> 00:00:02,000\nmayhem\n\n"
            )
            .to_owned(),
        ),
        (
            "vtt",
            "text/vtt",
            concat!(
                "WEBVTT\n\n",
                "00:00:00.000 --> 00:00:00.900\nhello\n\n",
                "00:00:01.000 --> 00:00:02.000\nmayhem\n\n"
            )
            .to_owned(),
        ),
    ] {
        let (boundary, body) = audio_transcription_multipart(&[
            ("model", "admin/stt-fixture"),
            ("response_format", format),
        ]);
        let content_type = format!("multipart/form-data; boundary={boundary}");

        let (status, headers, bytes) = raw_bytes_request_with_headers(
            app.clone(),
            Method::POST,
            "/v1/audio/transcriptions",
            body,
            &[("content-type", &content_type)],
        )
        .await;

        assert_eq!(status, StatusCode::OK, "response_format={format}");
        assert!(
            headers["content-type"]
                .to_str()
                .unwrap()
                .contains(expected_content_type),
            "response_format={format}"
        );
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            expected_body,
            "response_format={format}"
        );
    }
}

#[tokio::test]
async fn audio_transcription_rejects_unavailable_or_inapplicable_timestamps() {
    let model = routed_audio_transcription_test_model();
    let state = test_gateway_state_from_models(vec![model.clone()])
        .with_session_backend(Arc::new(TextOnlyAudioTranscriptionBackend));
    let app = openai_router(state);
    let (boundary, body) = audio_transcription_multipart(&[
        ("model", "admin/stt-fixture"),
        ("response_format", "verbose_json"),
        ("timestamp_granularities[]", "word"),
    ]);
    let content_type = format!("multipart/form-data; boundary={boundary}");

    let (status, _, bytes) = raw_bytes_request_with_headers(
        app,
        Method::POST,
        "/v1/audio/transcriptions",
        body,
        &[("content-type", &content_type)],
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let body: Value = serde_json::from_slice(&bytes).expect("timestamp error JSON");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("did not return requested word timestamps"));

    let state = test_gateway_state_from_models(vec![model])
        .with_session_backend(Arc::new(AudioTranscriptionDirectSessionBackend));
    let app = openai_router(state.clone());
    let (boundary, body) = audio_transcription_multipart(&[
        ("model", "admin/stt-fixture"),
        ("response_format", "json"),
        ("timestamp_granularities[]", "word"),
    ]);
    let content_type = format!("multipart/form-data; boundary={boundary}");

    let (status, _, bytes) = raw_bytes_request_with_headers(
        app,
        Method::POST,
        "/v1/audio/transcriptions",
        body,
        &[("content-type", &content_type)],
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&bytes).expect("timestamp format error JSON");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("supported only with response_format=verbose_json"));
    assert!(state.receipts().is_empty());
}

#[tokio::test]
async fn hf_audio_transcription_returns_requested_timestamp_chunks() {
    let state = test_gateway_state_from_models(vec![routed_hf_audio_transcription_test_model()])
        .with_session_backend(Arc::new(AudioTranscriptionDirectSessionBackend));
    let app = openai_router(state);
    let encoded = base64::engine::general_purpose::STANDARD.encode(tiny_wav_bytes(32_000));

    for (return_timestamps, expected_chunks) in [
        (
            json!(true),
            json!([
                {"text": "hello", "timestamp": [0.0, 0.9]},
                {"text": "mayhem", "timestamp": [1.0, 2.0]}
            ]),
        ),
        (
            json!("word"),
            json!([
                {"text": "hello", "timestamp": [0.0, 0.8]},
                {"text": "mayhem", "timestamp": [1.0, 2.0]}
            ]),
        ),
    ] {
        let request = json!({
            "inputs": encoded,
            "parameters": {"return_timestamps": return_timestamps}
        });
        let (status, body) = json_request(
            app.clone(),
            Method::POST,
            "/hf-inference/models/admin/stt-fixture",
            request,
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["text"], "hello mayhem");
        assert_eq!(body["chunks"], expected_chunks);
    }
}

#[tokio::test]
async fn hf_audio_transcription_accepts_raw_wav_and_rejects_mismatched_mime() {
    let state = test_gateway_state_from_models(vec![routed_hf_audio_transcription_test_model()])
        .with_session_backend(Arc::new(AudioTranscriptionDirectSessionBackend));
    let app = openai_router(state.clone());
    let wav = tiny_wav_bytes(32_000);

    let (status, _, bytes) = raw_bytes_request_with_headers(
        app.clone(),
        Method::POST,
        "/hf-inference/models/admin/stt-fixture",
        wav.clone(),
        &[("content-type", "audio/wav")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body: Value = serde_json::from_slice(&bytes).expect("raw HF ASR response JSON");
    assert_eq!(body["text"], "hello mayhem");
    assert_eq!(
        state.receipts()[0]
            .receipt
            .body
            .usage
            .get(USAGE_AUDIO_SECOND),
        2
    );

    let (status, body) = json_request(
        app.clone(),
        Method::POST,
        "/hf-inference/models/admin/stt-fixture",
        json!({
            "inputs": {
                "data": base64::engine::general_purpose::STANDARD.encode(&wav),
                "content_type": "audio/wav",
                "filename": "arbitrary-user-audio.wav",
            },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["text"], "hello mayhem");

    let (status, _, bytes) = raw_bytes_request_with_headers(
        app,
        Method::POST,
        "/hf-inference/models/admin/stt-fixture",
        wav,
        &[("content-type", "audio/flac")],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&bytes).expect("MIME mismatch error JSON");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("does not match audio/wav bytes"));
    assert_eq!(state.receipts().len(), 2);
}

#[tokio::test]
async fn hf_audio_transcription_meters_flac_streaminfo_duration() {
    let state = test_gateway_state_from_models(vec![routed_hf_audio_transcription_test_model()])
        .with_session_backend(Arc::new(AudioTranscriptionDirectSessionBackend));
    let app = openai_router(state.clone());
    let flac = tiny_flac_bytes();
    let request = json!({
        "inputs": base64::engine::general_purpose::STANDARD.encode(flac)
    });

    let (status, body) = json_request(
        app,
        Method::POST,
        "/hf-inference/models/admin/stt-fixture",
        request,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["text"], "hello mayhem");
    assert_eq!(
        state.receipts()[0]
            .receipt
            .body
            .usage
            .get(USAGE_AUDIO_SECOND),
        3
    );
    assert_eq!(state.receipts()[0].receipt.body.au_owed_cum, 750);
}

#[tokio::test]
async fn audio_duration_rejects_forged_wav_geometry_and_decodes_flac_frames() {
    let state = test_gateway_state_from_models(vec![routed_hf_audio_transcription_test_model()])
        .with_session_backend(Arc::new(AudioTranscriptionDirectSessionBackend));
    let app = openai_router(state.clone());

    let mut wav = tiny_wav_bytes(32_000);
    wav[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
    let (status, _) = json_request(
        app.clone(),
        Method::POST,
        "/hf-inference/models/admin/stt-fixture",
        json!({
            "inputs": base64::engine::general_purpose::STANDARD.encode(wav),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(state.receipts().is_empty());

    let mut flac = tiny_flac_bytes();
    let packed = u64::from_be_bytes(flac[18..26].try_into().expect("STREAMINFO packed field"));
    let forged = (packed & !0x0000_000f_ffff_ffff) | 1;
    flac[18..26].copy_from_slice(&forged.to_be_bytes());
    let (status, body) = json_request(
        app,
        Method::POST,
        "/hf-inference/models/admin/stt-fixture",
        json!({
            "inputs": base64::engine::general_purpose::STANDARD.encode(flac),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        state.receipts()[0]
            .receipt
            .body
            .usage
            .get(USAGE_AUDIO_SECOND),
        3
    );
}

#[tokio::test]
async fn automatic_transcript_match_probe_records_pass() {
    let state = test_gateway_state_from_models(vec![routed_audio_transcription_test_model()])
        .with_canary_registry(test_transcript_canary_registry(tiny_wav_bytes(16_000)))
        .with_canary_probe_policy(GatewayCanaryProbePolicy::every_session_for_tests())
        .with_session_backend(Arc::new(AudioTranscriptionDirectSessionBackend));
    let app = openai_router(state.clone());
    let boundary = "mayhem-test-boundary";
    let audio = tiny_wav_bytes(32_000);
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nadmin/stt-fixture\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"clip.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let (status, _, bytes) = raw_bytes_request_with_headers(
        app,
        Method::POST,
        "/v1/audio/transcriptions",
        body,
        &[(
            "content-type",
            &format!("multipart/form-data; boundary={boundary}"),
        )],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let body: Value = serde_json::from_slice(&bytes).expect("transcription JSON");
    assert_eq!(body["text"], "hello mayhem");
    assert_eq!(state.receipts().len(), 2);
    let probes = state.probes();
    assert_eq!(probes.len(), 1);
    let probe = &probes[0];
    assert_eq!(probe.verification_method, "transcript_match");
    assert!(probe.pass);
    assert_eq!(probe.match_bps, 10_000);
    assert_eq!(probe.evidence["receipts"].as_array().unwrap().len(), 1);
    assert_eq!(
        probe.evidence["evidence"]["prompts"][0]["word_timestamp_count"],
        2
    );
    assert_eq!(
        probe.evidence["evidence"]["prompts"][0]["segment_timestamp_count"],
        2
    );
    assert_eq!(
        probe.evidence["evidence"]["prompts"][0]["request"]["timestamp_granularities"],
        json!(["word", "segment"])
    );
}

#[tokio::test]
async fn chat_completion_returns_tool_call_and_accepts_tool_result_followup() {
    let model = first_model_id().await;
    let tool_request = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Use the weather tool." }],
        "tools": [{
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get weather",
                "parameters": { "type": "object", "properties": {} }
            }
        }]
    });
    let (status, body) = json_request(
        test_app(),
        Method::POST,
        "/v1/chat/completions",
        tool_request,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(
        body["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
        "get_weather"
    );
    assert_eq!(
        body["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        "{}"
    );

    let tool_call_id = body["choices"][0]["message"]["tool_calls"][0]["id"]
        .as_str()
        .expect("tool call id");
    let followup = json!({
        "model": first_model_id().await,
        "messages": [
            { "role": "user", "content": "Use the weather tool." },
            { "role": "assistant", "content": null, "tool_calls": body["choices"][0]["message"]["tool_calls"] },
            { "role": "tool", "tool_call_id": tool_call_id, "content": "{\"temperature_c\":21}" }
        ]
    });
    let (status, body) =
        json_request(test_app(), Method::POST, "/v1/chat/completions", followup).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .expect("assistant content")
        .contains("temperature_c"));
}

#[tokio::test]
async fn chat_completion_can_use_direct_session_backend() {
    let state = test_gateway_state_from_models(vec![routed_test_model()])
        .with_session_backend(Arc::new(TestDirectSessionBackend));
    let app = openai_router(state.clone());
    let model = "mayhem/routed-test".to_owned();
    let request = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Use a direct session." }]
    });

    let (status, body) =
        json_request(app.clone(), Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mayhem"]["backend"], "test-direct-session");
    assert_eq!(body["mayhem"]["direct_session"], true);
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .expect("assistant content")
        .contains("direct session response"));
    assert_eq!(state.receipts().len(), 1);
    assert_eq!(state.receipts()[0].receipt.body.usage.output_tokens(), 4);
    assert_eq!(state.receipts()[0].receipt.body.provider, "55".repeat(32));
    assert_eq!(
        state.receipts()[0].receipt.body.enclave_id,
        catalog_enclave_id(&routed_test_identity())
    );
    assert_eq!(state.receipts()[0].receipt.body.price_ver, 7);
    assert_eq!(
        state.receipts()[0].voucher.body.session_id,
        state.receipts()[0].receipt.body.session_id
    );

    let (status, body) = json_request(app, Method::GET, "/mayhem/status", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["backend"], "test-direct-session");
}

#[tokio::test]
async fn chat_completion_rejects_image_content_for_non_vision_model() {
    let state = test_gateway_state_from_models(vec![routed_test_model()])
        .with_session_backend(Arc::new(TestDirectSessionBackend));
    let app = openai_router(state);
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": "describe this" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,aW1hZ2U=" } }
            ]
        }]
    });

    let (status, body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["param"], "messages");
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("does not support image_url"));
}

#[tokio::test]
async fn chat_completion_preserves_image_content_for_vision_direct_session() {
    let mut model = routed_test_model();
    model.mayhem.caps.vision = true;
    model.mayhem.adapter.modality_set = vec!["text".to_owned(), "image".to_owned()];
    for candidate in &mut model.mayhem.route_candidates {
        candidate.served_modalities = vec!["text".to_owned(), "image".to_owned()];
        candidate.caps = json!({
            "tools": true,
            "json": true,
            "ctx_max": 8192,
            "vision": true,
        });
    }
    let seen_content = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![model]).with_session_backend(Arc::new(
        VisionInspectBackend {
            seen_content: seen_content.clone(),
        },
    ));
    let app = openai_router(state);
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(1, 1)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let image_url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png.into_inner())
    );
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": "describe this" },
                { "type": "image_url", "image_url": { "url": image_url } }
            ]
        }]
    });

    let (status, body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["choices"][0]["message"]["content"], "vision ok");
    assert_eq!(
        seen_content.lock().expect("seen content")[0][1]["image_url"]["url"],
        image_url
    );
}

#[tokio::test]
async fn chat_completion_exposes_direct_session_artifact_summary() {
    let state = test_gateway_state_from_models(vec![routed_test_model()])
        .with_session_backend(Arc::new(ArtifactDirectSessionBackend));
    let app = openai_router(state);
    let image = b"\x89PNG mayhem artifact".to_vec();
    let expected_hash = blake3::hash(&image).to_hex().to_string();
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Generate an image." }]
    });

    let (status, body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mayhem"]["backend"], "test-artifact-direct-session");
    assert_eq!(body["mayhem"]["artifacts"][0]["id"], "image-1");
    assert_eq!(body["mayhem"]["artifacts"][0]["content_type"], "image/png");
    assert_eq!(body["mayhem"]["artifacts"][0]["bytes"], image.len());
    assert_eq!(body["mayhem"]["artifacts"][0]["blake3"], expected_hash);
}

#[tokio::test]
async fn automatic_canary_probe_catches_substituted_served_enclave() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![routed_test_model()])
        .with_canary_registry(test_canary_registry(&[1, 2, 3]))
        .with_canary_probe_policy(GatewayCanaryProbePolicy::every_session_for_tests())
        .with_session_backend(Arc::new(CanarySubstitutionBackend {
            calls: calls.clone(),
        }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Use a direct session." }]
    });

    let (status, body) =
        json_request(app.clone(), Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mayhem"]["backend"], "test-canary-substitution");
    assert_eq!(state.receipts().len(), 2);
    assert_eq!(calls.lock().expect("calls lock").len(), 2);

    let probes = state.probes();
    assert_eq!(probes.len(), 1);
    let probe = &probes[0];
    assert!(!probe.pass);
    assert_eq!(probe.match_bps, 0);
    assert_eq!(probe.reputation_event_kind, ReputationEventKind::ProbeFail);
    assert_eq!(probe.probe_command["op"], "probe_result");
    assert_eq!(probe.probe_command["probe_kind"], "canary");
    assert_eq!(
        probe.probe_command["verification_method"],
        "token_fingerprint"
    );
    assert_eq!(probe.probe_command["pass"], false);
    assert_eq!(probe.verification_method, "token_fingerprint");
    assert_eq!(probe.probe_command["provider"], "55".repeat(32));
    assert_eq!(
        probe.probe_command["enclave_id"],
        catalog_enclave_id(&routed_test_identity())
    );
    assert!(
        probe.evidence["evidence"]["prompts"][0]["token_count"]
            .as_u64()
            .expect("token count")
            > 0
    );
    assert_eq!(
        probe.evidence["evidence"]["catalog_expected_token_prefixes"]["fixed-probe"],
        json!([1, 2, 3])
    );
    assert_eq!(
        probe.evidence["evidence"]["prompts"][0]["token_ids"],
        json!([9, 9, 9])
    );

    let (status, body) = json_request(app, Method::GET, "/mayhem/probes", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().expect("probe list").len(), 1);
    assert_eq!(body["data"][0]["pass"], false);
    assert_eq!(
        body["data"][0]["reputation_event_kind"]["kind"],
        "probe_fail"
    );
}

#[tokio::test]
async fn automatic_canary_probe_accepts_exact_catalog_token_prefix() {
    let state = test_gateway_state_from_models(vec![routed_test_model()])
        .with_canary_registry(test_canary_registry(&[9, 9, 9]))
        .with_canary_probe_policy(GatewayCanaryProbePolicy::every_session_for_tests())
        .with_session_backend(Arc::new(CanarySubstitutionBackend {
            calls: Arc::new(Mutex::new(Vec::new())),
        }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Use a direct session." }]
    });

    let (status, _body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::OK);
    let probes = state.probes();
    assert_eq!(probes.len(), 1);
    assert!(probes[0].pass);
    assert_eq!(probes[0].match_bps, 10_000);
    assert_eq!(
        probes[0].reputation_event_kind,
        ReputationEventKind::ProbeOk
    );
}

#[tokio::test]
async fn automatic_speciality_canary_binds_each_served_level_and_catches_substitution() {
    let provider = "55".repeat(32);
    let mut model = routed_test_model_with_specialities(std::slice::from_ref(&provider));
    model
        .mayhem
        .adapter
        .specialities
        .retain(|descriptor| descriptor.name == "reasoning_effort");
    let contract = model
        .mayhem
        .adapter
        .endpoint_families
        .first_mut()
        .expect("chat endpoint contract");
    contract
        .request_attributes
        .retain(|name| name != "verbosity");
    contract.request_attribute_specs.remove("verbosity");
    contract.speciality_mappings.remove("verbosity");
    model.mayhem.route_candidates[0]
        .served_specialities
        .retain(|name, _| name == "reasoning_effort");
    let expected_for = |tokens: &[i32]| {
        let prompt = token_fingerprint(tokens.iter().copied()).digest;
        aggregate_canary_fingerprints([("fixed-probe", prompt.as_str())])
    };
    let mut registry = test_canary_registry(&[9]);
    registry
        .models
        .get_mut("mayhem/routed-test")
        .expect("canary config")
        .prompts[0]
        .specialities
        .insert("reasoning_effort".to_owned(), "high".to_owned());
    registry
        .models
        .get_mut("mayhem/routed-test")
        .expect("canary config")
        .speciality_calibrations_by_artifact_root = BTreeMap::from([(
        "aa".repeat(32),
        BTreeMap::from([(
            "reasoning_effort".to_owned(),
            BTreeMap::from([
                (
                    "low".to_owned(),
                    GatewaySpecialityCalibration {
                        fingerprint: expected_for(&[4]),
                        verification_method: None,
                        token_prefixes: BTreeMap::from([("fixed-probe".to_owned(), vec![4])]),
                        output_tokens_min: 1,
                        output_tokens_max: 1,
                        reasoning_tokens_min: 0,
                        reasoning_tokens_max: 0,
                    },
                ),
                (
                    "high".to_owned(),
                    GatewaySpecialityCalibration {
                        fingerprint: expected_for(&[5, 5]),
                        verification_method: None,
                        token_prefixes: BTreeMap::from([("fixed-probe".to_owned(), vec![5, 5])]),
                        output_tokens_min: 2,
                        output_tokens_max: 2,
                        reasoning_tokens_min: 0,
                        reasoning_tokens_max: 0,
                    },
                ),
            ]),
        )]),
    )]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![model])
        .with_canary_registry(registry)
        .with_canary_probe_policy(GatewayCanaryProbePolicy::every_session_for_tests())
        .with_session_backend(Arc::new(SpecialityCanaryBackend {
            calls: calls.clone(),
        }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Use high effort." }],
        "reasoning_effort": "high"
    });

    let (status, _) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(state.receipts().len(), 4);
    let probes = state.probes();
    assert_eq!(probes.len(), 2);
    assert!(probes[0].pass, "baseline canary must still run and pass");
    assert!(!probes[1].pass);
    assert_eq!(probes[1].match_bps, 5_000);
    assert_eq!(probes[1].evidence["evidence"]["levels"][0]["level"], "low");
    assert_eq!(probes[1].evidence["evidence"]["levels"][0]["pass"], true);
    assert_eq!(probes[1].evidence["evidence"]["levels"][1]["level"], "high");
    assert_eq!(probes[1].evidence["evidence"]["levels"][1]["pass"], false);
    let calls = calls.lock().expect("calls lock");
    assert_eq!(calls.len(), 4);
    assert!(calls.iter().all(|(request_levels, voucher_levels)| {
        request_levels == voucher_levels && request_levels.get("reasoning_effort").is_some()
    }));
    assert_eq!(
        calls
            .iter()
            .map(|(levels, _)| levels["reasoning_effort"].as_str())
            .collect::<Vec<_>>(),
        vec!["high", "high", "low", "high"]
    );
}

#[tokio::test]
async fn automatic_context_needle_probe_marks_long_context_truncation_slashable() {
    let mut model = routed_test_model();
    model.mayhem.caps.ctx = 131_072;
    model.mayhem.route_candidates[0].caps = json!({ "ctx": 131_072 });
    let state = test_gateway_state_from_models(vec![model])
        .with_receipt_balance_au(10_000_000)
        .with_canary_registry(test_canary_registry(&[9, 9, 9]))
        .with_canary_probe_policy(GatewayCanaryProbePolicy::every_session_for_tests())
        .with_session_backend(Arc::new(ContextNeedleBackend {
            answer_needle: false,
        }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Use a direct session." }]
    });

    let (status, _body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::OK);
    let probes = state.probes();
    assert_eq!(probes.len(), 2);
    assert!(probes
        .iter()
        .any(|probe| probe.verification_method == "token_fingerprint" && probe.pass));
    let needle = probes
        .iter()
        .find(|probe| probe.verification_method == "context_needle")
        .expect("context needle probe");
    assert!(!needle.pass);
    assert_eq!(needle.match_bps, 0);
    assert_eq!(needle.reputation_event_kind, ReputationEventKind::ProbeFail);
    assert_eq!(needle.probe_command["probe_kind"], "canary");
    assert_eq!(
        needle.probe_command["verification_method"],
        "context_needle"
    );
    assert_eq!(needle.probe_command["pass"], false);
    assert_eq!(needle.evidence["evidence"]["served_ctx"], 131_072);
    assert_eq!(needle.evidence["evidence"]["ctx_bracket"], "le128k");
    assert!(
        needle.evidence["evidence"]["needle_position_tokens"]
            .as_u64()
            .unwrap()
            > 100_000
    );
    assert!(
        needle.evidence["evidence"]["tail_tokens_after_needle"]
            .as_u64()
            .unwrap()
            >= 16_384
    );
}

#[tokio::test]
async fn contract_model_with_noncanonical_route_is_unavailable() {
    let mut model = routed_test_model();
    model.mayhem.route_candidates[0].room_id = "provider-local-only".to_owned();
    let state = test_gateway_state_from_models(vec![model])
        .with_session_backend(Arc::new(TestDirectSessionBackend));
    let app = openai_router(state);
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "This should not route." }]
    });

    let (status, body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("not available"));
}

#[tokio::test]
async fn chat_completion_retries_retryable_direct_session_route_before_metering() {
    let first_provider = "55".repeat(32);
    let second_provider = "66".repeat(32);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![routed_test_model_with_providers(&[
        first_provider.clone(),
        second_provider.clone(),
    ])])
    .with_session_backend(Arc::new(RetryFirstDirectSessionBackend {
        calls: calls.clone(),
    }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Retry a direct session." }]
    });

    let (status, body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mayhem"]["backend"], "test-retry-first-direct-session");
    let calls = calls.lock().expect("calls lock").clone();
    assert_eq!(calls.len(), 2);
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .expect("assistant content")
        .contains(calls[1].as_str()));
    assert_ne!(calls[0], calls[1]);
    assert!(calls.iter().all(
        |provider| [first_provider.as_str(), second_provider.as_str()].contains(&provider.as_str())
    ));
    assert_eq!(state.receipts().len(), 1);
    assert_eq!(state.receipts()[0].receipt.body.provider, calls[1]);
}

#[tokio::test]
async fn chat_completion_caps_retryable_direct_session_routes_at_four_without_receipts() {
    let providers = (1..=5)
        .map(|idx| format!("{idx:02x}").repeat(32))
        .collect::<Vec<_>>();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![routed_test_model_with_providers(&providers)])
        .with_session_backend(Arc::new(AlwaysRetryDirectSessionBackend {
            calls: calls.clone(),
        }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Every direct open times out." }]
    });

    let (status, body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("all 4 route attempt(s) failed before spend"));
    let calls = calls.lock().expect("calls lock").clone();
    assert_eq!(calls.len(), 4);
    let unique_calls = calls.iter().cloned().collect::<BTreeSet<_>>();
    assert_eq!(unique_calls.len(), 4);
    assert!(calls.iter().all(|provider| providers.contains(provider)));
    assert!(state.receipts().is_empty());
}

#[tokio::test]
async fn chat_completion_binds_x_mayhem_hedge_to_direct_session_invocation() {
    let first_provider = "55".repeat(32);
    let second_provider = "66".repeat(32);
    let invocations = Arc::new(Mutex::new(Vec::new()));
    let probes = Arc::new(Mutex::new(Vec::new()));
    let probe_delays_ms =
        BTreeMap::from([(first_provider.clone(), 25), (second_provider.clone(), 1)]);
    let state = test_gateway_state_from_models(vec![routed_test_model_with_providers(&[
        first_provider.clone(),
        second_provider.clone(),
    ])])
    .with_session_backend(Arc::new(HedgeInspectBackend {
        invocations: invocations.clone(),
        probes: probes.clone(),
        probe_delays_ms,
    }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Hedge this direct session." }]
    });

    let (status, body) = json_request_with_headers(
        app,
        Method::POST,
        "/v1/chat/completions",
        request,
        &[("X-Mayhem-Hedge", "1")],
    )
    .await;

    assert_eq!(status, StatusCode::OK, "response body: {body}");
    assert_eq!(body["mayhem"]["backend"], "test-hedge-inspect");
    assert_eq!(body["mayhem"]["hedge"]["requested"], true);
    assert_eq!(body["mayhem"]["hedge"]["planned_probe_count"], 2);
    assert_eq!(body["mayhem"]["hedge"]["actual_probe_count"], 2);
    assert_eq!(body["mayhem"]["hedge"]["winner_provider"], second_provider);
    assert_eq!(body["mayhem"]["hedge"]["winner_ttft_ms"], 1);
    let probes = probes.lock().expect("probes lock").clone();
    assert_eq!(probes.iter().cloned().collect::<BTreeSet<_>>().len(), 2);
    assert!(probes.contains(&first_provider));
    assert!(probes.contains(&second_provider));
    let invocations = invocations.lock().expect("invocations lock").clone();
    assert_eq!(invocations.len(), 1);
    assert_eq!(
        invocations[0],
        (
            second_provider.clone(),
            true,
            2,
            2,
            Some(second_provider.clone())
        )
    );
    assert_eq!(state.receipts().len(), 1);
    assert_eq!(state.receipts()[0].receipt.body.provider, invocations[0].0);
}

#[tokio::test]
async fn invalid_x_mayhem_hedge_header_is_rejected_before_session_start() {
    let invocations = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![routed_test_model_with_providers(&[
        "55".repeat(32),
        "66".repeat(32),
    ])])
    .with_session_backend(Arc::new(HedgeInspectBackend {
        invocations: invocations.clone(),
        probes: Arc::new(Mutex::new(Vec::new())),
        probe_delays_ms: BTreeMap::new(),
    }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "This header should fail." }]
    });

    let (status, body) = json_request_with_headers(
        app,
        Method::POST,
        "/v1/chat/completions",
        request,
        &[("X-Mayhem-Hedge", "true")],
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["param"], "X-Mayhem-Hedge");
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("must be 1"));
    assert!(invocations.lock().expect("invocations lock").is_empty());
    assert!(state.receipts().is_empty());
}

#[tokio::test]
async fn invalid_x_mayhem_min_att_tier_header_is_rejected_before_session_start() {
    let invocations = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![routed_test_model_with_providers(&[
        "55".repeat(32),
        "66".repeat(32),
    ])])
    .with_session_backend(Arc::new(HedgeInspectBackend {
        invocations: invocations.clone(),
        probes: Arc::new(Mutex::new(Vec::new())),
        probe_delays_ms: BTreeMap::new(),
    }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "This tier header should fail." }]
    });

    let (status, body) = json_request_with_headers(
        app,
        Method::POST,
        "/v1/chat/completions",
        request,
        &[("X-Mayhem-Min-Att-Tier", "5")],
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["param"], "X-Mayhem-Min-Att-Tier");
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("between 1 and 4"));
    assert!(invocations.lock().expect("invocations lock").is_empty());
    assert!(state.receipts().is_empty());
}

#[tokio::test]
async fn chat_completion_min_att_tier_filters_route_candidates() {
    let first_provider = "55".repeat(32);
    let second_provider = "66".repeat(32);
    let mut model =
        routed_test_model_with_providers(&[first_provider.clone(), second_provider.clone()]);
    model.mayhem.route_candidates[0].att_tier = 1;
    model.mayhem.route_candidates[1].att_tier = 4;
    model.mayhem.attestation_tiers = BTreeMap::from([("T1".to_owned(), 1), ("T4".to_owned(), 1)]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![model]).with_session_backend(Arc::new(
        RetryThenDirectSessionBackend {
            retry_provider: "ff".repeat(32),
            calls: calls.clone(),
        },
    ));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Use Tier 3 or higher." }]
    });

    let (status, body) = json_request_with_headers(
        app,
        Method::POST,
        "/v1/chat/completions",
        request,
        &[("X-Mayhem-Min-Att-Tier", "3")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        calls.lock().expect("calls lock").clone(),
        vec![second_provider.clone()]
    );
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .expect("assistant content")
        .contains(&second_provider));
    assert_eq!(state.receipts().len(), 1);
    assert_eq!(state.receipts()[0].receipt.body.provider, second_provider);
}

#[tokio::test]
async fn chat_completion_min_att_tier_two_routes_and_bills_tier2_market_price() {
    let tier1_provider = "55".repeat(32);
    let tier2_signing_key = av3_signing_key(66);
    let tier2_provider = av3_provider(&tier2_signing_key);
    let tier2_enclave = "77".repeat(32);
    let mut model =
        routed_test_model_with_providers(&[tier1_provider.clone(), tier2_provider.clone()]);
    model.mayhem.attestation_tiers = BTreeMap::from([("T1".to_owned(), 1), ("T2".to_owned(), 1)]);
    model.mayhem.route_candidates[0].att_tier = 1;
    model.mayhem.route_candidates[0].price_ver = 1;
    model.mayhem.route_candidates[0].price_ref_au = Some(PriceRefAu {
        denom: "au_usd".to_owned(),
        ver: 1,
        rate_map: text_generation_rate_map(10, 20),
        per_req_au: 0,
        min_session_au: 0,
        derivation: None,
        history: Vec::new(),
    });
    model.mayhem.route_candidates[1].att_tier = 2;
    model.mayhem.route_candidates[1].enclave_id = tier2_enclave.clone();
    model.mayhem.route_candidates[1].device_key = Some("78".repeat(32));
    model.mayhem.route_candidates[1].price_ver = 22;
    model.mayhem.route_candidates[1].price_ref_au = Some(PriceRefAu {
        denom: "au_usd".to_owned(),
        ver: 22,
        rate_map: text_generation_rate_map(90, 180),
        per_req_au: 123,
        min_session_au: 456,
        derivation: Some(json!({
            "epoch": 9u64,
            "enclave_id": tier2_enclave,
            "price_root": "aa".repeat(32)
        })),
        history: Vec::new(),
    });
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (authority, _) = av3_apple_authority(&model.mayhem.route_candidates[1], 1, None);
    let state = test_gateway_state_from_models(vec![model.clone()])
        .with_attestation_authority(authority)
        .with_session_backend(Arc::new(RetryThenDirectSessionBackend {
            retry_provider: "ff".repeat(32),
            calls: calls.clone(),
        }));
    av3_ingest_advertisement(
        &state,
        &model,
        &model.mayhem.route_candidates[1],
        &tier2_signing_key,
        HardwareQuoteKind::AppleAppAttestJwt,
    );
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Use the Tier 2 device-identity market." }]
    });

    let (status, body) = json_request_with_headers(
        app,
        Method::POST,
        "/v1/chat/completions",
        request,
        &[("X-Mayhem-Min-Att-Tier", "2")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        calls.lock().expect("calls lock").clone(),
        vec![tier2_provider.clone()]
    );
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .expect("assistant content")
        .contains(&tier2_provider));
    let receipts = state.receipts();
    assert_eq!(receipts.len(), 1);
    let receipt = &receipts[0].receipt;
    assert_eq!(receipt.body.provider, tier2_provider);
    assert_eq!(receipt.body.enclave_id, "77".repeat(32));
    assert_eq!(receipt.body.price_ver, 22);
    assert_eq!(
        receipt.body.locked_rate_map,
        normalize_rate_map(text_generation_rate_map(90, 180))
    );
    assert_eq!(receipt.body.locked_per_req_au, 123);
    assert_eq!(receipt.body.locked_min_session_au, 456);
    assert_eq!(
        receipt.body.au_owed_cum,
        priced_usage_au(
            &receipt.body.locked_rate_map,
            receipt.body.locked_per_req_au,
            receipt.body.locked_min_session_au,
            &receipt.body.usage,
        )
    );
    assert_eq!(receipt.body.au_owed_cum, 456);
}

#[tokio::test]
async fn chat_completion_min_att_tier_rejects_when_no_route_meets_pin() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![routed_test_model_with_providers(&[
        "55".repeat(32),
        "66".repeat(32),
    ])])
    .with_session_backend(Arc::new(RetryThenDirectSessionBackend {
        retry_provider: "ff".repeat(32),
        calls: calls.clone(),
    }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Need Tier 3." }]
    });

    let (status, body) = json_request_with_headers(
        app,
        Method::POST,
        "/v1/chat/completions",
        request,
        &[("X-Mayhem-Min-Att-Tier", "3")],
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["param"], "X-Mayhem-Min-Att-Tier");
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("no provider route"));
    assert!(calls.lock().expect("calls lock").is_empty());
    assert!(state.receipts().is_empty());
}

#[tokio::test]
async fn chat_completion_quant_filters_route_candidates() {
    let first_provider = "55".repeat(32);
    let second_provider = "66".repeat(32);
    let mut model =
        routed_test_model_with_providers(&[first_provider.clone(), second_provider.clone()]);
    model.mayhem.route_candidates[0].quant = "int4".to_owned();
    model.mayhem.route_candidates[1].quant = "fp16".to_owned();
    model.mayhem.quant_buckets = BTreeMap::from([("int4".to_owned(), 1), ("fp16".to_owned(), 1)]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![model]).with_session_backend(Arc::new(
        RetryThenDirectSessionBackend {
            retry_provider: "ff".repeat(32),
            calls: calls.clone(),
        },
    ));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Use the fp16 enclave." }]
    });

    let (status, body) = json_request_with_headers(
        app,
        Method::POST,
        "/v1/chat/completions",
        request,
        &[("X-Mayhem-Quant", "FP16")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        calls.lock().expect("calls lock").clone(),
        vec![second_provider.clone()]
    );
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .expect("assistant content")
        .contains(&second_provider));
    assert_eq!(state.receipts().len(), 1);
    assert_eq!(state.receipts()[0].receipt.body.provider, second_provider);
}

#[tokio::test]
async fn chat_completion_quant_rejects_when_no_route_matches() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![routed_test_model_with_providers(&[
        "55".repeat(32),
        "66".repeat(32),
    ])])
    .with_session_backend(Arc::new(RetryThenDirectSessionBackend {
        retry_provider: "ff".repeat(32),
        calls: calls.clone(),
    }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Need fp16." }]
    });

    let (status, body) = json_request_with_headers(
        app,
        Method::POST,
        "/v1/chat/completions",
        request,
        &[("X-Mayhem-Quant", "fp16")],
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["param"], "X-Mayhem-Quant");
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("no provider route"));
    assert!(calls.lock().expect("calls lock").is_empty());
    assert!(state.receipts().is_empty());
}

fn routed_test_model() -> GatewayModel {
    routed_test_model_with_providers(&["55".repeat(32)])
}

fn routed_embedding_test_model() -> GatewayModel {
    let mut model = routed_test_model_with_providers(&["55".repeat(32)]);
    model.id = "admin/embed-fixture".to_owned();
    model.mayhem.model_class = "embedding".to_owned();
    model.mayhem.price_ref_au = PriceRefAu {
        denom: "au_usd".to_owned(),
        ver: 3,
        rate_map: vec![mayhem_gateway::RateMapEntry {
            unit: "input_token".to_owned(),
            per_unit_au: 10,
            granularity: 1_000,
        }],
        per_req_au: 0,
        min_session_au: 0,
        derivation: None,
        history: Vec::new(),
    };
    model.mayhem.caps = ModelCaps {
        tools: false,
        json: false,
        ctx: 8192,
        vision: false,
        image: false,
        video: false,
        audio: false,
        max_image_width: None,
        max_image_height: None,
        max_image_steps: None,
        output_modality: Some("embedding".to_owned()),
        output_modalities: vec!["embedding".to_owned()],
    };
    model.mayhem.adapter.modality_set = vec!["embedding".to_owned()];
    model.mayhem.adapter.endpoint_families = vec![
        mayhem_proto::endpoint_family_contract_template(mayhem_proto::ENDPOINT_OPENAI_EMBEDDINGS)
            .unwrap(),
        mayhem_proto::endpoint_family_contract_template(
            mayhem_proto::ENDPOINT_HF_FEATURE_EXTRACTION,
        )
        .unwrap(),
    ];
    for candidate in &mut model.mayhem.route_candidates {
        candidate.served_modalities = vec!["embedding".to_owned()];
        candidate.price_ver = 3;
        candidate.caps = serde_json::json!({
            "ctx_max": 8192,
            "output_modality": "embedding",
            "output_modalities": ["embedding"]
        });
    }
    model
}

fn routed_image_generation_test_model() -> GatewayModel {
    let mut model = routed_test_model_with_providers(&["55".repeat(32)]);
    model.id = "admin/image-fixture".to_owned();
    model.mayhem.model_class = "image-generation".to_owned();
    model.mayhem.price_ref_au = PriceRefAu {
        denom: "au_usd".to_owned(),
        ver: 4,
        rate_map: vec![
            mayhem_gateway::RateMapEntry {
                unit: USAGE_IMAGE.to_owned(),
                per_unit_au: 500,
                granularity: 1,
            },
            mayhem_gateway::RateMapEntry {
                unit: USAGE_STEP.to_owned(),
                per_unit_au: 2,
                granularity: 1,
            },
        ],
        per_req_au: 0,
        min_session_au: 0,
        derivation: None,
        history: Vec::new(),
    };
    model.mayhem.caps = ModelCaps {
        tools: false,
        json: false,
        ctx: 4096,
        vision: false,
        image: true,
        video: false,
        audio: false,
        max_image_width: Some(1024),
        max_image_height: Some(1024),
        max_image_steps: Some(50),
        output_modality: Some("image".to_owned()),
        output_modalities: vec!["image".to_owned()],
    };
    model.mayhem.adapter.modality_set = vec!["image".to_owned()];
    let mut endpoint = mayhem_proto::endpoint_family_contract_template(
        mayhem_proto::ENDPOINT_OPENAI_IMAGE_GENERATIONS,
    )
    .unwrap();
    endpoint
        .request_attribute_specs
        .get_mut("cfg_scale")
        .expect("image template has cfg_scale")
        .default = Some(json!(1.0));
    endpoint
        .request_attribute_specs
        .get_mut("n")
        .expect("image template has n")
        .default = Some(json!(1));
    model.mayhem.adapter.endpoint_families = vec![endpoint];
    for candidate in &mut model.mayhem.route_candidates {
        candidate.served_modalities = vec!["image".to_owned()];
        candidate.price_ver = 4;
        candidate.caps = serde_json::json!({
            "ctx_max": 4096,
            "image": true,
            "max_image_width": 1024,
            "max_image_height": 1024,
            "max_image_steps": 50,
            "output_modality": "image",
            "output_modalities": ["image"]
        });
    }
    model
}

fn routed_video_generation_test_model() -> GatewayModel {
    routed_artifact_generation_test_model(
        "admin/video-fixture",
        "video-generation",
        "video",
        &"58".repeat(32),
        8,
        &[
            mayhem_proto::ENDPOINT_OPENAI_VIDEOS,
            mayhem_proto::ENDPOINT_HF_TEXT_TO_VIDEO,
        ],
        vec![
            mayhem_gateway::RateMapEntry {
                unit: USAGE_VIDEO_SECOND.to_owned(),
                per_unit_au: 100,
                granularity: 1,
            },
            mayhem_gateway::RateMapEntry {
                unit: USAGE_FRAME.to_owned(),
                per_unit_au: 2,
                granularity: 1,
            },
        ],
    )
}

fn routed_general_audio_generation_test_model() -> GatewayModel {
    routed_artifact_generation_test_model(
        "admin/audio-generation-fixture",
        "audio-generation",
        "audio",
        &"59".repeat(32),
        9,
        &[
            mayhem_proto::ENDPOINT_MAYHEM_AUDIO_GENERATIONS,
            mayhem_proto::ENDPOINT_HF_TEXT_TO_AUDIO,
        ],
        vec![
            mayhem_gateway::RateMapEntry {
                unit: USAGE_INPUT_CHARACTER.to_owned(),
                per_unit_au: 1,
                granularity: 1,
            },
            mayhem_gateway::RateMapEntry {
                unit: USAGE_AUDIO_SECOND.to_owned(),
                per_unit_au: 100,
                granularity: 1,
            },
        ],
    )
}

fn routed_music_generation_test_model() -> GatewayModel {
    routed_artifact_generation_test_model(
        "admin/music-generation-fixture",
        "music-generation",
        "audio",
        &"60".repeat(32),
        10,
        &[
            mayhem_proto::ENDPOINT_MAYHEM_MUSIC_GENERATIONS,
            mayhem_proto::ENDPOINT_HF_TEXT_TO_AUDIO,
        ],
        vec![
            mayhem_gateway::RateMapEntry {
                unit: USAGE_INPUT_CHARACTER.to_owned(),
                per_unit_au: 1,
                granularity: 1,
            },
            mayhem_gateway::RateMapEntry {
                unit: USAGE_AUDIO_SECOND.to_owned(),
                per_unit_au: 125,
                granularity: 1,
            },
        ],
    )
}

fn routed_artifact_generation_test_model(
    id: &str,
    model_class: &str,
    modality: &str,
    provider: &str,
    price_ver: u64,
    endpoint_families: &[&str],
    rate_map: Vec<mayhem_gateway::RateMapEntry>,
) -> GatewayModel {
    let mut model = routed_test_model_with_providers(&[provider.to_owned()]);
    model.id = id.to_owned();
    model.mayhem.model_class = model_class.to_owned();
    model.mayhem.price_ref_au = PriceRefAu {
        denom: "au_usd".to_owned(),
        ver: price_ver,
        rate_map,
        per_req_au: 0,
        min_session_au: 0,
        derivation: None,
        history: Vec::new(),
    };
    model.mayhem.caps = ModelCaps {
        tools: false,
        json: false,
        ctx: 4096,
        vision: false,
        image: false,
        video: modality == "video",
        audio: modality == "audio",
        max_image_width: None,
        max_image_height: None,
        max_image_steps: None,
        output_modality: Some(modality.to_owned()),
        output_modalities: vec![modality.to_owned()],
    };
    model.mayhem.adapter.modality_set = vec![modality.to_owned()];
    model.mayhem.adapter.endpoint_families = endpoint_families
        .iter()
        .map(|family| mayhem_proto::endpoint_family_contract_template(family).unwrap())
        .collect();
    for candidate in &mut model.mayhem.route_candidates {
        candidate.served_modalities = vec![modality.to_owned()];
        candidate.price_ver = price_ver;
        candidate.caps = json!({
            "ctx_max": 4096,
            "video": modality == "video",
            "audio": modality == "audio",
            "output_modality": modality,
            "output_modalities": [modality],
        });
    }
    model
}

fn routed_audio_speech_test_model() -> GatewayModel {
    let mut model = routed_test_model_with_providers(&["56".repeat(32)]);
    model.id = "admin/tts-fixture".to_owned();
    model.mayhem.model_class = "tts".to_owned();
    model.mayhem.price_ref_au = PriceRefAu {
        denom: "au_usd".to_owned(),
        ver: 5,
        rate_map: vec![
            mayhem_gateway::RateMapEntry {
                unit: USAGE_INPUT_CHARACTER.to_owned(),
                per_unit_au: 1,
                granularity: 1,
            },
            mayhem_gateway::RateMapEntry {
                unit: USAGE_AUDIO_SECOND.to_owned(),
                per_unit_au: 100,
                granularity: 1,
            },
        ],
        per_req_au: 0,
        min_session_au: 0,
        derivation: None,
        history: Vec::new(),
    };
    model.mayhem.caps = ModelCaps {
        tools: false,
        json: false,
        ctx: 4096,
        vision: false,
        image: false,
        video: false,
        audio: true,
        max_image_width: None,
        max_image_height: None,
        max_image_steps: None,
        output_modality: Some("audio".to_owned()),
        output_modalities: vec!["audio".to_owned()],
    };
    model.mayhem.adapter.modality_set = vec!["audio".to_owned()];
    model.mayhem.adapter.endpoint_families = vec![mayhem_proto::endpoint_family_contract_template(
        mayhem_proto::ENDPOINT_OPENAI_AUDIO_SPEECH,
    )
    .unwrap()];
    for candidate in &mut model.mayhem.route_candidates {
        candidate.served_modalities = vec!["audio".to_owned()];
        candidate.price_ver = 5;
        candidate.caps = serde_json::json!({
            "ctx_max": 4096,
            "audio": true,
            "output_modality": "audio",
            "output_modalities": ["audio"]
        });
    }
    model
}

fn routed_audio_transcription_test_model() -> GatewayModel {
    let mut model = routed_test_model_with_providers(&["57".repeat(32)]);
    model.id = "admin/stt-fixture".to_owned();
    model.mayhem.model_class = "stt".to_owned();
    model.mayhem.price_ref_au = PriceRefAu {
        denom: "au_usd".to_owned(),
        ver: 6,
        rate_map: vec![mayhem_gateway::RateMapEntry {
            unit: USAGE_AUDIO_SECOND.to_owned(),
            per_unit_au: 250,
            granularity: 1,
        }],
        per_req_au: 0,
        min_session_au: 0,
        derivation: None,
        history: Vec::new(),
    };
    model.mayhem.caps = ModelCaps {
        tools: false,
        json: false,
        ctx: 4096,
        vision: false,
        image: false,
        video: false,
        audio: true,
        max_image_width: None,
        max_image_height: None,
        max_image_steps: None,
        output_modality: Some("text".to_owned()),
        output_modalities: vec!["text".to_owned()],
    };
    model.mayhem.adapter.modality_set = vec!["audio".to_owned(), "text".to_owned()];
    model.mayhem.adapter.endpoint_families = vec![mayhem_proto::endpoint_family_contract_template(
        mayhem_proto::ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS,
    )
    .unwrap()];
    for candidate in &mut model.mayhem.route_candidates {
        candidate.served_modalities = vec!["audio".to_owned(), "text".to_owned()];
        candidate.price_ver = 6;
        candidate.caps = serde_json::json!({
            "ctx_max": 4096,
            "audio": true,
            "output_modality": "text",
            "output_modalities": ["text"]
        });
    }
    model
}

fn routed_hf_audio_transcription_test_model() -> GatewayModel {
    let mut model = routed_audio_transcription_test_model();
    model.mayhem.adapter.endpoint_families = vec![mayhem_proto::endpoint_family_contract_template(
        mayhem_proto::ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION,
    )
    .unwrap()];
    model
}

fn signed_image_provider_receipt(
    model: &GatewayModel,
    request: &ImageGenerationRequest,
    invocation: &GatewaySessionInvocation,
    usage: &ReceiptUsage,
) -> Result<ProviderSignedReceipt, GatewaySessionError> {
    let enclave_seed = [88_u8; 32];
    let enclave_key = SigningKey::from_bytes(&enclave_seed);
    let provider = invocation
        .provider_pubkey
        .clone()
        .unwrap_or_else(|| "55".repeat(32));
    let body = ReceiptBody {
        schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
        session_id: invocation.session_id.clone(),
        billing_id: invocation.spend_voucher.body.billing_id.clone(),
        billing_attempt: invocation.spend_voucher.body.billing_attempt,
        billing_prior_usage: invocation.spend_voucher.body.billing_prior_usage.clone(),
        billing_prior_au_owed_cum: invocation.spend_voucher.body.billing_prior_au_owed_cum,
        billing_epoch: invocation.spend_voucher.body.billing_epoch,
        reservation_id: invocation.spend_voucher.body.reservation_id.clone(),
        reservation_expires_after_epoch: invocation
            .spend_voucher
            .body
            .reservation_expires_after_epoch,
        reservation_receipt_grace_epochs: invocation
            .spend_voucher
            .body
            .reservation_receipt_grace_epochs,
        payout_revision: invocation.spend_voucher.body.payout_revision.clone(),
        seq: 1,
        final_receipt: true,
        rail: invocation.rail.clone(),
        user: invocation.user_pubkey.clone(),
        provider,
        enclave_id: invocation.enclave_id.clone(),
        model_id: model.id.clone(),
        price_ver: invocation.price_ver,
        locked_rate_map: invocation.spend_voucher.body.locked_rate_map.clone(),
        locked_per_req_au: invocation.spend_voucher.body.locked_per_req_au,
        locked_min_session_au: invocation.spend_voucher.body.locked_min_session_au,
        served_ctx: invocation.served_ctx,
        ctx_bracket: invocation.ctx_bracket.clone(),
        ctx_bracket_table_ver: invocation.ctx_bracket_table_ver,
        rules_ver: invocation.rules_ver,
        usage: usage.clone(),
        usage_attribution: BTreeMap::new(),
        au_owed_cum: priced_usage_au(
            &invocation.spend_voucher.body.locked_rate_map,
            invocation.spend_voucher.body.locked_per_req_au,
            invocation.spend_voucher.body.locked_min_session_au,
            usage,
        ),
        prompt_hash: image_prompt_hash_for_test(request),
        ts: 1_782_950_400_000,
    };
    let payload = receipt_signing_bytes(&body)
        .map_err(|err| GatewaySessionError::new(format!("receipt payload failed: {err}")))?;
    Ok(ProviderSignedReceipt {
        body,
        enclave_sig: hex::encode(enclave_key.sign(&payload).to_bytes()),
        enclave_pubkey: hex::encode(enclave_key.verifying_key().to_bytes()),
    })
}

fn signed_audio_speech_provider_receipt(
    model: &GatewayModel,
    request: &AudioSpeechRequest,
    invocation: &GatewaySessionInvocation,
    usage: &ReceiptUsage,
) -> Result<ProviderSignedReceipt, GatewaySessionError> {
    signed_provider_receipt_for_test(
        model,
        invocation,
        usage,
        audio_speech_prompt_hash_for_test(request),
    )
}

fn signed_audio_transcription_provider_receipt(
    model: &GatewayModel,
    request: &AudioTranscriptionRequest,
    invocation: &GatewaySessionInvocation,
    usage: &ReceiptUsage,
) -> Result<ProviderSignedReceipt, GatewaySessionError> {
    signed_provider_receipt_for_test(
        model,
        invocation,
        usage,
        audio_transcription_prompt_hash_for_test(request),
    )
}

fn signed_provider_receipt_for_test(
    model: &GatewayModel,
    invocation: &GatewaySessionInvocation,
    usage: &ReceiptUsage,
    prompt_hash: String,
) -> Result<ProviderSignedReceipt, GatewaySessionError> {
    let enclave_seed = [88_u8; 32];
    let enclave_key = SigningKey::from_bytes(&enclave_seed);
    let provider = invocation
        .provider_pubkey
        .clone()
        .unwrap_or_else(|| "55".repeat(32));
    let body = ReceiptBody {
        schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
        session_id: invocation.session_id.clone(),
        billing_id: invocation.spend_voucher.body.billing_id.clone(),
        billing_attempt: invocation.spend_voucher.body.billing_attempt,
        billing_prior_usage: invocation.spend_voucher.body.billing_prior_usage.clone(),
        billing_prior_au_owed_cum: invocation.spend_voucher.body.billing_prior_au_owed_cum,
        billing_epoch: invocation.spend_voucher.body.billing_epoch,
        reservation_id: invocation.spend_voucher.body.reservation_id.clone(),
        reservation_expires_after_epoch: invocation
            .spend_voucher
            .body
            .reservation_expires_after_epoch,
        reservation_receipt_grace_epochs: invocation
            .spend_voucher
            .body
            .reservation_receipt_grace_epochs,
        payout_revision: invocation.spend_voucher.body.payout_revision.clone(),
        seq: 1,
        final_receipt: true,
        rail: invocation.rail.clone(),
        user: invocation.user_pubkey.clone(),
        provider,
        enclave_id: invocation.enclave_id.clone(),
        model_id: model.id.clone(),
        price_ver: invocation.price_ver,
        locked_rate_map: invocation.spend_voucher.body.locked_rate_map.clone(),
        locked_per_req_au: invocation.spend_voucher.body.locked_per_req_au,
        locked_min_session_au: invocation.spend_voucher.body.locked_min_session_au,
        served_ctx: invocation.served_ctx,
        ctx_bracket: invocation.ctx_bracket.clone(),
        ctx_bracket_table_ver: invocation.ctx_bracket_table_ver,
        rules_ver: invocation.rules_ver,
        usage: usage.clone(),
        usage_attribution: BTreeMap::new(),
        au_owed_cum: priced_usage_au(
            &invocation.spend_voucher.body.locked_rate_map,
            invocation.spend_voucher.body.locked_per_req_au,
            invocation.spend_voucher.body.locked_min_session_au,
            usage,
        ),
        prompt_hash,
        ts: 1_782_950_400_000,
    };
    let payload = receipt_signing_bytes(&body)
        .map_err(|err| GatewaySessionError::new(format!("receipt payload failed: {err}")))?;
    Ok(ProviderSignedReceipt {
        body,
        enclave_sig: hex::encode(enclave_key.sign(&payload).to_bytes()),
        enclave_pubkey: hex::encode(enclave_key.verifying_key().to_bytes()),
    })
}

fn image_usage_for_test(request: &ImageGenerationRequest) -> ReceiptUsage {
    let images = u64::from(request.n.unwrap_or(1).clamp(1, 4));
    let steps = image_steps_for_test(request);
    let resolution_scale = image_resolution_scale_for_test(request);
    ReceiptUsage::from_units([
        (USAGE_IMAGE, images),
        (
            USAGE_STEP,
            images
                .saturating_mul(steps)
                .saturating_mul(resolution_scale),
        ),
    ])
}

fn image_resolution_scale_for_test(request: &ImageGenerationRequest) -> u64 {
    if let Some((width, height)) = request.width.zip(request.height) {
        return u64::from(width)
            .saturating_mul(u64::from(height))
            .max(1)
            .div_ceil(512 * 512)
            .max(1);
    }
    let size = request.size.as_deref().unwrap_or("512x512");
    let Some((width, height)) = size.split_once('x') else {
        return 1;
    };
    let pixels = width
        .parse::<u64>()
        .unwrap_or(512)
        .saturating_mul(height.parse::<u64>().unwrap_or(512))
        .max(1);
    pixels.div_ceil(512 * 512).max(1)
}

fn audio_speech_usage_for_test(request: &AudioSpeechRequest, audio: &[u8]) -> ReceiptUsage {
    ReceiptUsage::from_units([
        (
            USAGE_INPUT_CHARACTER,
            u64::try_from(request.input.chars().count()).unwrap(),
        ),
        (
            USAGE_AUDIO_SECOND,
            wav_duration_seconds_ceil_for_test(audio).unwrap(),
        ),
    ])
}

fn audio_transcription_usage_for_test(request: &AudioTranscriptionRequest) -> ReceiptUsage {
    ReceiptUsage::from_units([(USAGE_AUDIO_SECOND, request.audio_seconds)])
}

fn image_steps_for_test(request: &ImageGenerationRequest) -> u64 {
    request.steps.unwrap_or(1).clamp(1, 150)
}

fn image_cfg_scale_for_test(request: &ImageGenerationRequest) -> f32 {
    request.cfg_scale.unwrap_or(1.0).clamp(0.0, 50.0)
}

fn image_size_for_test(request: &ImageGenerationRequest) -> (u32, u32) {
    if let Some(dimensions) = request.width.zip(request.height) {
        return dimensions;
    }
    let size = request.size.as_deref().unwrap_or("512x512");
    let Some((width, height)) = size.split_once('x') else {
        return (512, 512);
    };
    (
        width.parse::<u32>().unwrap_or(512),
        height.parse::<u32>().unwrap_or(512),
    )
}

fn image_prompt_hash_for_test(request: &ImageGenerationRequest) -> String {
    if let Some(endpoint_request) = &request.endpoint_request {
        return endpoint_request_fingerprint(endpoint_request);
    }
    let mut body = json!({
        "kind": "image_generation",
        "model": &request.model,
        "prompt": &request.prompt,
        "n": request.n.expect("normalized image count"),
        "steps": request.steps.expect("normalized image steps"),
        "cfg_scale": request.cfg_scale.expect("normalized image guidance"),
        "response_format": request.response_format.as_deref().expect("normalized image response format"),
        "endpoint_family": request.endpoint_family.as_deref().unwrap_or(mayhem_proto::ENDPOINT_OPENAI_IMAGE_GENERATIONS),
    });
    let optional = [
        (
            "background",
            request.background.as_ref().map(|value| json!(value)),
        ),
        (
            "moderation",
            request.moderation.as_ref().map(|value| json!(value)),
        ),
        (
            "output_compression",
            request.output_compression.map(|value| json!(value)),
        ),
        (
            "output_format",
            request.output_format.as_ref().map(|value| json!(value)),
        ),
        (
            "partial_images",
            request.partial_images.map(|value| json!(value)),
        ),
        ("size", request.size.as_ref().map(|value| json!(value))),
        ("width", request.width.map(|value| json!(value))),
        ("height", request.height.map(|value| json!(value))),
        ("seed", request.seed.map(|value| json!(value))),
        (
            "quality",
            request.quality.as_ref().map(|value| json!(value)),
        ),
        ("style", request.style.as_ref().map(|value| json!(value))),
        (
            "negative_prompt",
            request.negative_prompt.as_ref().map(|value| json!(value)),
        ),
        ("shift", request.shift.map(|value| json!(value))),
        (
            "scheduler",
            request.scheduler.as_ref().map(|value| json!(value)),
        ),
        ("stream", request.stream.map(|value| json!(value))),
        ("user", request.user.as_ref().map(|value| json!(value))),
    ];
    for (name, value) in optional {
        if let Some(value) = value {
            body[name] = value;
        }
    }
    endpoint_contract_hash_for_test(body)
}

fn png_average_hash_fixture(inverted: bool) -> Vec<u8> {
    let mut image = image::ImageBuffer::<image::Luma<u8>, Vec<u8>>::new(8, 8);
    for (x, _y, pixel) in image.enumerate_pixels_mut() {
        let bright = if x < 4 { inverted } else { !inverted };
        *pixel = image::Luma([if bright { 255 } else { 0 }]);
    }
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(image)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("write png fixture");
    bytes.into_inner()
}

fn audio_speech_prompt_hash_for_test(request: &AudioSpeechRequest) -> String {
    if let Some(endpoint_request) = &request.endpoint_request {
        return endpoint_request_fingerprint(endpoint_request);
    }
    let mut body = json!({
        "kind": "audio_speech",
        "model": &request.model,
        "input": &request.input,
        "response_format": request.response_format.as_deref().unwrap_or("wav"),
        "endpoint_family": request.endpoint_family.as_deref().unwrap_or(mayhem_proto::ENDPOINT_OPENAI_AUDIO_SPEECH),
    });
    for (name, value) in [
        ("voice", request.voice.as_ref().map(|value| json!(value))),
        ("speed", request.speed.map(|value| json!(value))),
        (
            "instructions",
            request.instructions.as_ref().map(|value| json!(value)),
        ),
        (
            "stream_format",
            request.stream_format.as_ref().map(|value| json!(value)),
        ),
        ("reference_audio", request.reference_audio.as_ref().cloned()),
        (
            "exaggeration",
            request.exaggeration.map(|value| json!(value)),
        ),
        ("cfg_weight", request.cfg_weight.map(|value| json!(value))),
        ("temperature", request.temperature.map(|value| json!(value))),
        ("min_p", request.min_p.map(|value| json!(value))),
        ("top_p", request.top_p.map(|value| json!(value))),
        (
            "repetition_penalty",
            request.repetition_penalty.map(|value| json!(value)),
        ),
        ("seed", request.seed.map(|value| json!(value))),
    ] {
        if let Some(value) = value {
            body[name] = value;
        }
    }
    endpoint_contract_hash_for_test(body)
}

fn endpoint_contract_hash_for_test(mut transport_body: Value) -> String {
    if let Some(body) = transport_body.as_object_mut() {
        body.remove("kind");
        body.remove("endpoint_family");
        body.remove("mayhem_contract");
        body.remove("contract_request");
        body.remove("specialities");
    }
    endpoint_request_fingerprint(&transport_body)
}

fn audio_transcription_prompt_hash_for_test(request: &AudioTranscriptionRequest) -> String {
    endpoint_request_fingerprint(&request.contract_request)
}

fn tiny_wav_bytes(sample_count: u32) -> Vec<u8> {
    let sample_rate = 16_000u32;
    let channels = 1u16;
    let bits_per_sample = 16u16;
    let bytes_per_sample = u32::from(channels) * u32::from(bits_per_sample) / 8;
    let data_len = sample_count * bytes_per_sample;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * bytes_per_sample).to_le_bytes());
    bytes.extend_from_slice(&(bytes_per_sample as u16).to_le_bytes());
    bytes.extend_from_slice(&bits_per_sample.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for frame in 0..sample_count {
        let phase = std::f64::consts::TAU * 220.0 * f64::from(frame) / f64::from(sample_rate);
        let sample = (phase.sin() * 4_096.0).round() as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn wav_bytes_for_duration_seconds(duration_seconds: u64) -> Vec<u8> {
    let sample_count = duration_seconds
        .checked_mul(16_000)
        .and_then(|samples| u32::try_from(samples).ok())
        .expect("signed test duration fits the bounded WAV fixture");
    tiny_wav_bytes(sample_count)
}

fn tiny_flac_bytes() -> Vec<u8> {
    include_bytes!("fixtures/stt-32001-samples.flac").to_vec()
}

fn wav_duration_seconds_ceil_for_test(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let data_len = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]) as u64;
    let byte_rate = u32::from_le_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]) as u64;
    Some(data_len.div_ceil(byte_rate).max(1))
}

fn test_canary_registry(expected_tokens: &[i32]) -> GatewayCanaryRegistry {
    let prompt_fingerprint = token_fingerprint(expected_tokens.iter().copied()).digest;
    let expected_fingerprint =
        aggregate_canary_fingerprints([("fixed-probe", prompt_fingerprint.as_str())]);
    GatewayCanaryRegistry {
        models: BTreeMap::from([(
            "mayhem/routed-test".to_owned(),
            GatewayCanaryModelConfig {
                canary_set: "canary-test-v1".to_owned(),
                requires_launch_evidence: false,
                match_min_bps: 9_000,
                verification_method: "token_fingerprint".to_owned(),
                verification_tolerance_bps: None,
                prompts: vec![GatewayCanaryPrompt {
                    id: "fixed-probe".to_owned(),
                    calibration_only: false,
                    messages: vec![ChatMessage {
                        role: "user".to_owned(),
                        content: json!("fixed canary prompt"),
                        name: None,
                        extra: BTreeMap::new(),
                    }],
                    tools: None,
                    specialities: BTreeMap::new(),
                    max_tokens: 8,
                    temperature: None,
                    top_p: None,
                    top_k: None,
                    min_p: None,
                    repeat_penalty: None,
                    frequency_penalty: None,
                    presence_penalty: None,
                    prompt: None,
                    input: None,
                    audio_b64: None,
                    content_type: None,
                    filename: None,
                    language: None,
                    voice: None,
                    response_format: None,
                    require_word_timestamps: false,
                    require_segment_timestamps: false,
                    size: None,
                    steps: None,
                    cfg_scale: None,
                    shift: None,
                    seed: None,
                    endpoint_attributes: BTreeMap::new(),
                }],
                fingerprints_by_artifact_root: BTreeMap::from([(
                    "aa".repeat(32),
                    expected_fingerprint,
                )]),
                token_prefixes_by_artifact_root: BTreeMap::from([(
                    "aa".repeat(32),
                    BTreeMap::from([("fixed-probe".to_owned(), expected_tokens.to_vec())]),
                )]),
                perceptual_hashes_by_artifact_root: BTreeMap::new(),
                embedding_vectors_by_artifact_root: BTreeMap::new(),
                transcripts_by_artifact_root: BTreeMap::new(),
                audio_fingerprints_by_artifact_root: BTreeMap::new(),
                video_fingerprints_by_artifact_root: BTreeMap::new(),
                speciality_calibrations_by_artifact_root: BTreeMap::new(),
                default_fingerprint: None,
                default_token_prefixes: None,
                default_perceptual_hashes: None,
                default_embedding_vectors: None,
                default_transcripts: None,
                default_audio_fingerprints: None,
                default_video_fingerprints: None,
            },
        )]),
    }
}

fn test_image_canary_registry(expected_hash: String) -> GatewayCanaryRegistry {
    GatewayCanaryRegistry {
        models: BTreeMap::from([(
            "admin/image-fixture".to_owned(),
            GatewayCanaryModelConfig {
                canary_set: "canary-image-test-v1".to_owned(),
                requires_launch_evidence: false,
                match_min_bps: 9_000,
                verification_method: "seed_perceptual_hash".to_owned(),
                verification_tolerance_bps: Some(500),
                prompts: vec![GatewayCanaryPrompt {
                    id: "fixed-image".to_owned(),
                    calibration_only: false,
                    messages: Vec::new(),
                    tools: None,
                    specialities: BTreeMap::new(),
                    max_tokens: 1,
                    temperature: None,
                    top_p: None,
                    top_k: None,
                    min_p: None,
                    repeat_penalty: None,
                    frequency_penalty: None,
                    presence_penalty: None,
                    prompt: Some("fixed image canary".to_owned()),
                    input: None,
                    audio_b64: None,
                    content_type: None,
                    filename: None,
                    language: None,
                    voice: None,
                    response_format: None,
                    require_word_timestamps: false,
                    require_segment_timestamps: false,
                    size: Some("64x64".to_owned()),
                    steps: Some(1),
                    cfg_scale: Some(1.0),
                    shift: None,
                    seed: Some(7),
                    endpoint_attributes: BTreeMap::new(),
                }],
                fingerprints_by_artifact_root: BTreeMap::new(),
                token_prefixes_by_artifact_root: BTreeMap::new(),
                perceptual_hashes_by_artifact_root: BTreeMap::from([(
                    "aa".repeat(32),
                    BTreeMap::from([("fixed-image".to_owned(), expected_hash)]),
                )]),
                embedding_vectors_by_artifact_root: BTreeMap::new(),
                transcripts_by_artifact_root: BTreeMap::new(),
                audio_fingerprints_by_artifact_root: BTreeMap::new(),
                video_fingerprints_by_artifact_root: BTreeMap::new(),
                speciality_calibrations_by_artifact_root: BTreeMap::new(),
                default_fingerprint: None,
                default_token_prefixes: None,
                default_perceptual_hashes: None,
                default_embedding_vectors: None,
                default_transcripts: None,
                default_audio_fingerprints: None,
                default_video_fingerprints: None,
            },
        )]),
    }
}

fn test_embedding_canary_registry(expected_vector: Vec<f32>) -> GatewayCanaryRegistry {
    GatewayCanaryRegistry {
        models: BTreeMap::from([(
            "admin/embed-fixture".to_owned(),
            GatewayCanaryModelConfig {
                canary_set: "canary-embedding-test-v1".to_owned(),
                requires_launch_evidence: false,
                match_min_bps: 9_900,
                verification_method: "embedding_cosine".to_owned(),
                verification_tolerance_bps: Some(100),
                prompts: vec![GatewayCanaryPrompt {
                    id: "fixed-embedding".to_owned(),
                    calibration_only: false,
                    messages: Vec::new(),
                    tools: None,
                    specialities: BTreeMap::new(),
                    max_tokens: 1,
                    temperature: None,
                    top_p: None,
                    top_k: None,
                    min_p: None,
                    repeat_penalty: None,
                    frequency_penalty: None,
                    presence_penalty: None,
                    prompt: None,
                    input: Some("fixed embedding canary".to_owned()),
                    audio_b64: None,
                    content_type: None,
                    filename: None,
                    language: None,
                    voice: None,
                    response_format: None,
                    require_word_timestamps: false,
                    require_segment_timestamps: false,
                    size: None,
                    steps: None,
                    cfg_scale: None,
                    shift: None,
                    seed: None,
                    endpoint_attributes: BTreeMap::new(),
                }],
                fingerprints_by_artifact_root: BTreeMap::new(),
                token_prefixes_by_artifact_root: BTreeMap::new(),
                perceptual_hashes_by_artifact_root: BTreeMap::new(),
                embedding_vectors_by_artifact_root: BTreeMap::from([(
                    "aa".repeat(32),
                    BTreeMap::from([("fixed-embedding".to_owned(), expected_vector)]),
                )]),
                transcripts_by_artifact_root: BTreeMap::new(),
                audio_fingerprints_by_artifact_root: BTreeMap::new(),
                video_fingerprints_by_artifact_root: BTreeMap::new(),
                speciality_calibrations_by_artifact_root: BTreeMap::new(),
                default_fingerprint: None,
                default_token_prefixes: None,
                default_perceptual_hashes: None,
                default_embedding_vectors: None,
                default_transcripts: None,
                default_audio_fingerprints: None,
                default_video_fingerprints: None,
            },
        )]),
    }
}

fn test_transcript_canary_registry(audio: Vec<u8>) -> GatewayCanaryRegistry {
    let runtime_prompt = GatewayCanaryPrompt {
        id: "fixed-stt".to_owned(),
        calibration_only: false,
        messages: Vec::new(),
        tools: None,
        specialities: BTreeMap::new(),
        max_tokens: 1,
        temperature: None,
        top_p: None,
        top_k: None,
        min_p: None,
        repeat_penalty: None,
        frequency_penalty: None,
        presence_penalty: None,
        prompt: None,
        input: None,
        audio_b64: Some(base64::engine::general_purpose::STANDARD.encode(audio)),
        content_type: Some("audio/wav".to_owned()),
        filename: Some("fixed-stt.wav".to_owned()),
        language: Some("en".to_owned()),
        voice: None,
        response_format: Some("json".to_owned()),
        require_word_timestamps: true,
        require_segment_timestamps: true,
        size: None,
        steps: None,
        cfg_scale: None,
        shift: None,
        seed: None,
        endpoint_attributes: BTreeMap::new(),
    };
    let mut calibration_prompt = runtime_prompt.clone();
    calibration_prompt.id = "long-calibration-only".to_owned();
    calibration_prompt.calibration_only = true;
    GatewayCanaryRegistry {
        models: BTreeMap::from([(
            "admin/stt-fixture".to_owned(),
            GatewayCanaryModelConfig {
                canary_set: "canary-stt-test-v1".to_owned(),
                requires_launch_evidence: false,
                match_min_bps: 10_000,
                verification_method: "transcript_match".to_owned(),
                verification_tolerance_bps: None,
                prompts: vec![runtime_prompt, calibration_prompt],
                fingerprints_by_artifact_root: BTreeMap::new(),
                token_prefixes_by_artifact_root: BTreeMap::new(),
                perceptual_hashes_by_artifact_root: BTreeMap::new(),
                embedding_vectors_by_artifact_root: BTreeMap::new(),
                transcripts_by_artifact_root: BTreeMap::from([(
                    "aa".repeat(32),
                    BTreeMap::from([
                        ("fixed-stt".to_owned(), "hello mayhem".to_owned()),
                        (
                            "long-calibration-only".to_owned(),
                            "must never run live".to_owned(),
                        ),
                    ]),
                )]),
                audio_fingerprints_by_artifact_root: BTreeMap::new(),
                video_fingerprints_by_artifact_root: BTreeMap::new(),
                speciality_calibrations_by_artifact_root: BTreeMap::new(),
                default_fingerprint: None,
                default_token_prefixes: None,
                default_perceptual_hashes: None,
                default_embedding_vectors: None,
                default_transcripts: None,
                default_audio_fingerprints: None,
                default_video_fingerprints: None,
            },
        )]),
    }
}

fn test_audio_fingerprint_canary_registry(expected_fingerprint: String) -> GatewayCanaryRegistry {
    GatewayCanaryRegistry {
        models: BTreeMap::from([(
            "admin/tts-fixture".to_owned(),
            GatewayCanaryModelConfig {
                canary_set: "canary-tts-test-v1".to_owned(),
                requires_launch_evidence: false,
                match_min_bps: 10_000,
                verification_method: "audio_fingerprint".to_owned(),
                verification_tolerance_bps: None,
                prompts: vec![GatewayCanaryPrompt {
                    id: "fixed-tts".to_owned(),
                    calibration_only: false,
                    messages: Vec::new(),
                    tools: None,
                    specialities: BTreeMap::new(),
                    max_tokens: 1,
                    temperature: None,
                    top_p: None,
                    top_k: None,
                    min_p: None,
                    repeat_penalty: None,
                    frequency_penalty: None,
                    presence_penalty: None,
                    prompt: None,
                    input: Some("fixed speech canary".to_owned()),
                    audio_b64: None,
                    content_type: None,
                    filename: None,
                    language: None,
                    voice: Some("launch".to_owned()),
                    response_format: Some("wav".to_owned()),
                    require_word_timestamps: false,
                    require_segment_timestamps: false,
                    size: None,
                    steps: None,
                    cfg_scale: None,
                    shift: None,
                    seed: None,
                    endpoint_attributes: BTreeMap::new(),
                }],
                fingerprints_by_artifact_root: BTreeMap::new(),
                token_prefixes_by_artifact_root: BTreeMap::new(),
                perceptual_hashes_by_artifact_root: BTreeMap::new(),
                embedding_vectors_by_artifact_root: BTreeMap::new(),
                transcripts_by_artifact_root: BTreeMap::new(),
                audio_fingerprints_by_artifact_root: BTreeMap::from([(
                    "aa".repeat(32),
                    BTreeMap::from([("fixed-tts".to_owned(), expected_fingerprint)]),
                )]),
                video_fingerprints_by_artifact_root: BTreeMap::new(),
                speciality_calibrations_by_artifact_root: BTreeMap::new(),
                default_fingerprint: None,
                default_token_prefixes: None,
                default_perceptual_hashes: None,
                default_embedding_vectors: None,
                default_transcripts: None,
                default_audio_fingerprints: None,
                default_video_fingerprints: None,
            },
        )]),
    }
}

fn test_music_audio_fingerprint_canary_registry(
    expected_fingerprint: String,
) -> GatewayCanaryRegistry {
    GatewayCanaryRegistry {
        models: BTreeMap::from([(
            "admin/music-generation-fixture".to_owned(),
            GatewayCanaryModelConfig {
                canary_set: "canary-music-test-v1".to_owned(),
                requires_launch_evidence: false,
                match_min_bps: 9_000,
                verification_method: "audio_fingerprint".to_owned(),
                verification_tolerance_bps: Some(1_000),
                prompts: vec![GatewayCanaryPrompt {
                    id: "fixed-music".to_owned(),
                    calibration_only: false,
                    messages: Vec::new(),
                    tools: None,
                    specialities: BTreeMap::new(),
                    max_tokens: 1,
                    temperature: None,
                    top_p: None,
                    top_k: None,
                    min_p: None,
                    repeat_penalty: None,
                    frequency_penalty: None,
                    presence_penalty: None,
                    prompt: Some("fixed music canary".to_owned()),
                    input: None,
                    audio_b64: Some(
                        base64::engine::general_purpose::STANDARD
                            .encode(b"STT-only fixture must not enter a music request"),
                    ),
                    content_type: None,
                    filename: None,
                    language: None,
                    voice: None,
                    response_format: Some("wav".to_owned()),
                    require_word_timestamps: false,
                    require_segment_timestamps: false,
                    size: None,
                    steps: Some(50),
                    cfg_scale: None,
                    shift: None,
                    seed: Some(7),
                    endpoint_attributes: BTreeMap::from([
                        ("lyrics".to_owned(), json!("[Verse]\nCanonical proof")),
                        ("instrumental".to_owned(), json!(false)),
                        ("duration_seconds".to_owned(), json!(10)),
                        ("bpm".to_owned(), json!(120)),
                        ("keyscale".to_owned(), json!("C major")),
                        ("timesignature".to_owned(), json!("4/4")),
                        ("task_type".to_owned(), json!("text2music")),
                        ("thinking".to_owned(), json!(false)),
                        ("guidance_scale".to_owned(), json!(7)),
                        ("n".to_owned(), json!(1)),
                    ]),
                }],
                fingerprints_by_artifact_root: BTreeMap::new(),
                token_prefixes_by_artifact_root: BTreeMap::new(),
                perceptual_hashes_by_artifact_root: BTreeMap::new(),
                embedding_vectors_by_artifact_root: BTreeMap::new(),
                transcripts_by_artifact_root: BTreeMap::new(),
                audio_fingerprints_by_artifact_root: BTreeMap::from([(
                    "aa".repeat(32),
                    BTreeMap::from([("fixed-music".to_owned(), expected_fingerprint)]),
                )]),
                video_fingerprints_by_artifact_root: BTreeMap::new(),
                speciality_calibrations_by_artifact_root: BTreeMap::new(),
                default_fingerprint: None,
                default_token_prefixes: None,
                default_perceptual_hashes: None,
                default_embedding_vectors: None,
                default_transcripts: None,
                default_audio_fingerprints: None,
                default_video_fingerprints: None,
            },
        )]),
    }
}

fn routed_test_model_with_providers(providers: &[String]) -> GatewayModel {
    let mut tiers = BTreeMap::new();
    tiers.insert("T1".to_owned(), 1);
    GatewayModel {
        id: "mayhem/routed-test".to_owned(),
        created: 1_782_950_400,
        owned_by: "mayhem".to_owned(),
        mayhem: MayhemModelInfo {
            model_class: DEFAULT_MODEL_CLASS.to_owned(),
            family: "test".to_owned(),
            providers_online: 1,
            rooms: 1,
            price_ref_au: PriceRefAu {
                denom: "au_usd".to_owned(),
                ver: 7,
                rate_map: text_generation_rate_map(20, 60),
                per_req_au: 0,
                min_session_au: 0,
                derivation: None,
                history: Vec::new(),
            },
            attestation_tiers: tiers,
            attestation_tier_labels: BTreeMap::from([(
                "T1".to_owned(),
                "Tier 1 - software self-attestation; economic/trust only".to_owned(),
            )]),
            quant_buckets: BTreeMap::from([("int4".to_owned(), providers.len() as u32)]),
            min_app_version: None,
            caps: ModelCaps {
                tools: true,
                json: true,
                ctx: 8192,
                vision: false,
                image: false,
                video: false,
                audio: false,
                max_image_width: None,
                max_image_height: None,
                max_image_steps: None,
                output_modality: Some("text".to_owned()),
                output_modalities: vec!["text".to_owned()],
            },
            adapter: ShapeAdapterInfo::default(),
            speciality_calibrations: BTreeMap::new(),
            sampling: SamplingProfile::default(),
            failover: mayhem_gateway::openai::GatewayFailoverPolicyConfig::default(),
            source: "contract".to_owned(),
            kyb_identities: Vec::new(),
            markets: Vec::new(),
            route_candidates: providers
                .iter()
                .enumerate()
                .map(|(idx, provider)| routed_test_candidate(provider, idx))
                .collect(),
        },
    }
}

fn routed_test_model_with_specialities(providers: &[String]) -> GatewayModel {
    let mut model = routed_test_model_with_providers(providers);
    let specialities = vec![
        ModelSpecialityDescriptor {
            name: "reasoning_effort".to_owned(),
            mechanism: "enum".to_owned(),
            default_level: "low".to_owned(),
            levels: vec![
                ModelSpecialityLevel {
                    name: "low".to_owned(),
                    rank: 0,
                    native_value: json!("low"),
                    default_max_output_tokens: Some(8),
                    max_reasoning_tokens: Some(4),
                },
                ModelSpecialityLevel {
                    name: "high".to_owned(),
                    rank: 1,
                    native_value: json!("high"),
                    default_max_output_tokens: Some(32),
                    max_reasoning_tokens: Some(24),
                },
            ],
            calibration_modalities: Vec::new(),
            research_evidence: vec!["test family documentation".to_owned()],
        },
        ModelSpecialityDescriptor {
            name: "verbosity".to_owned(),
            mechanism: "string_enum".to_owned(),
            default_level: "concise".to_owned(),
            levels: vec![
                ModelSpecialityLevel {
                    name: "concise".to_owned(),
                    rank: 0,
                    native_value: json!("short"),
                    default_max_output_tokens: None,
                    max_reasoning_tokens: None,
                },
                ModelSpecialityLevel {
                    name: "detailed".to_owned(),
                    rank: 1,
                    native_value: json!("long"),
                    default_max_output_tokens: None,
                    max_reasoning_tokens: None,
                },
            ],
            calibration_modalities: Vec::new(),
            research_evidence: vec!["test family documentation".to_owned()],
        },
    ];
    let contract = model
        .mayhem
        .adapter
        .endpoint_families
        .iter_mut()
        .find(|contract| contract.family == mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS)
        .expect("chat endpoint contract");
    for (descriptor, target, native_path) in [
        (
            &specialities[0],
            EndpointSpecialityTarget::ChatTemplateKwarg,
            "reasoning_effort",
        ),
        (
            &specialities[1],
            EndpointSpecialityTarget::SamplingParameter,
            "verbosity",
        ),
    ] {
        contract.request_attributes.push(descriptor.name.clone());
        let levels = descriptor
            .levels
            .iter()
            .map(|level| json!(level.name))
            .collect::<Vec<_>>();
        let mut spec = EndpointAttributeSpec::new(EndpointValueType::String);
        spec.default = Some(json!(descriptor.default_level));
        spec.enum_values = levels.clone();
        spec.calibration_values = levels;
        contract
            .request_attribute_specs
            .insert(descriptor.name.clone(), spec);
        contract.speciality_mappings.insert(
            descriptor.name.clone(),
            EndpointSpecialityMapping {
                request_path: descriptor.name.clone(),
                target,
                native_path: native_path.to_owned(),
                selector: EndpointSpecialitySelector::Exact,
            },
        );
    }
    let served_specialities = specialities
        .iter()
        .map(|descriptor| {
            (
                descriptor.name.clone(),
                descriptor
                    .levels
                    .iter()
                    .map(|level| level.name.clone())
                    .collect(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    model.mayhem.adapter.specialities = specialities;
    for candidate in &mut model.mayhem.route_candidates {
        candidate.served_specialities = served_specialities.clone();
    }
    model
}

fn routed_test_candidate(provider: &str, idx: usize) -> GatewayRouteCandidate {
    let identity = routed_test_identity();
    let room_id = format!("{:02x}", idx + 160).repeat(16);
    GatewayRouteCandidate {
        provider: provider.to_owned(),
        accepted_rails: vec!["fiat".to_owned(), "tap".to_owned(), "tnk".to_owned()],
        payout_revisions: BTreeMap::from([
            ("fiat".to_owned(), "11".repeat(32)),
            ("tap".to_owned(), "22".repeat(32)),
            ("tnk".to_owned(), "33".repeat(32)),
        ]),
        served_modalities: vec!["text".to_owned()],
        served_specialities: BTreeMap::new(),
        enclave_id: catalog_enclave_id(&identity),
        room_id,
        price_ver: 7,
        price_ref_au: None,
        min_ask_au: 0,
        att_tier: 1,
        quant: "int4".to_owned(),
        served_ctx: None,
        hardware_fingerprint: None,
        device_key: None,
        admin_pubkey: identity.admin_pubkey,
        artifact_root: identity.artifact_root,
        artifact_sidecar_roots: BTreeMap::new(),
        manifest_hash: identity.manifest_hash,
        binary_hash: identity.binary_hash,
        approved_binary_hashes: BTreeSet::new(),
        launch_measurements: serde_json::Value::Null,
        kyb: None,
        reputation_bps: 10_000,
        probation: None,
        caps: serde_json::json!({}),
        local_run: None,
    }
}

fn test_provider_heartbeat(
    model: &GatewayModel,
    candidate: &GatewayRouteCandidate,
    sat: f64,
    active_slots: u32,
    max_slots: u32,
    tok_s: Option<f64>,
    ttft_ms: u64,
) -> ProviderHeartbeat {
    ProviderHeartbeat {
        t: "hb".to_owned(),
        v: HEARTBEAT_SCHEMA_VERSION,
        contract_version: CONTRACT_VERSION,
        provider: candidate.provider.clone(),
        enclave_id: candidate.enclave_id.clone(),
        model_id: model.id.clone(),
        room_id: candidate.room_id.clone(),
        sat,
        slots: HeartbeatSlots {
            active: active_slots,
            active_requests: active_slots.saturating_sub(1),
            max: max_slots,
        },
        q: HeartbeatQueue {
            free_slots: 1,
            engine_backlog: 0,
            est_wait_ms: 250,
        },
        perf: HeartbeatPerf { tok_s, ttft_ms },
        price_ver: candidate.price_ver,
        min_ask_au: 0,
        transport_peer: Some(candidate.provider.clone()),
        identity_anchor: None,
        accepting_new: true,
        caps: HeartbeatCaps {
            tools: candidate
                .caps
                .get("tools")
                .and_then(Value::as_bool)
                .unwrap_or(model.mayhem.caps.tools),
            json: candidate
                .caps
                .get("json")
                .and_then(Value::as_bool)
                .unwrap_or(model.mayhem.caps.json),
            ctx: candidate
                .caps
                .get("ctx")
                .or_else(|| candidate.caps.get("ctx_max"))
                .and_then(Value::as_u64)
                .and_then(|ctx| u32::try_from(ctx).ok())
                .unwrap_or(model.mayhem.caps.ctx),
            vision: candidate
                .caps
                .get("vision")
                .and_then(Value::as_bool)
                .unwrap_or(model.mayhem.caps.vision),
            served_modalities: candidate.served_modalities.clone(),
            served_specialities: candidate.served_specialities.clone(),
            modality_capacity: candidate
                .served_modalities
                .iter()
                .filter(|modality| modality.as_str() != "text")
                .map(|modality| {
                    let unit = match modality.as_str() {
                        "image" => "pixel",
                        "audio" => "second",
                        "video" => "frame",
                        "embedding" => "input_token",
                        _ => "unit",
                    };
                    (
                        modality.clone(),
                        HeartbeatModalityCapacity {
                            unit: unit.to_owned(),
                            max_inflight_items: 16,
                            active_items: 0,
                            max_items_per_request: 4,
                            max_item_bytes: 256 * 1024 * 1024,
                            max_item_units: 100_000_000,
                            working_set_bytes_per_item: 1024,
                        },
                    )
                })
                .collect(),
        },
        att: HeartbeatAttestation {
            epoch: 3,
            head: candidate.binary_hash.clone(),
        },
        ts: current_test_millis(),
        nonce: format!("network-dashboard-test-{}", candidate.room_id),
        sig: "11".repeat(64),
    }
}

fn test_gateway_state_from_models(models: Vec<GatewayModel>) -> GatewayState {
    let heartbeats = models
        .iter()
        .flat_map(|model| {
            model.mayhem.route_candidates.iter().map(|candidate| {
                test_provider_heartbeat(model, candidate, 0.0, 0, 8, Some(50.0), 150)
            })
        })
        .collect::<Vec<_>>();
    GatewayState::from_models(models).with_provider_heartbeats(heartbeats)
}

fn routed_test_identity() -> CatalogEnclaveIdentity {
    CatalogEnclaveIdentity {
        admin_pubkey: "44".repeat(32),
        model_id: "mayhem/routed-test".to_owned(),
        artifact_root: "aa".repeat(32),
        artifact_sidecar_roots: std::collections::BTreeMap::new(),
        manifest_hash: "bb".repeat(32),
        binary_hash: "cc".repeat(32),
    }
}

#[tokio::test]
async fn capability_body_transport_is_identical_for_curl_sdk_and_opencode() {
    let provider = "5a".repeat(32);
    let model = routed_test_model_with_specialities(std::slice::from_ref(&provider));
    let records = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![model]).with_session_backend(Arc::new(
        SpecialityRecordingBackend {
            records: records.clone(),
        },
    ));
    let app = openai_router(state);
    let base = json!({
        "model": "mayhem/routed-test",
        "messages": [{"role": "user", "content": "compare the transport"}]
    });
    let options = json!({
        "reasoning_effort": "high",
        "verbosity": "detailed",
        "top_k": 42,
        "min_p": 0.08,
        "max_completion_tokens": 24
    });
    let merge_options = |mut request: Value, options: &Value| {
        request
            .as_object_mut()
            .expect("request object")
            .extend(options.as_object().expect("options object").clone());
        request
    };
    let curl_body = merge_options(base.clone(), &options);
    let sdk_extra_body = merge_options(base.clone(), &options);
    let opencode_options = merge_options(base, &options);
    assert_eq!(curl_body, sdk_extra_body);
    assert_eq!(sdk_extra_body, opencode_options);

    for request in [curl_body, sdk_extra_body, opencode_options] {
        let (status, body) =
            json_request(app.clone(), Method::POST, "/v1/chat/completions", request).await;
        assert_eq!(status, StatusCode::OK, "response: {body}");
    }

    let records = records.lock().expect("speciality transport records");
    assert_eq!(records.len(), 3);
    let expected_specialities = BTreeMap::from([
        ("reasoning_effort".to_owned(), "high".to_owned()),
        ("verbosity".to_owned(), "detailed".to_owned()),
    ]);
    for record in records.iter() {
        assert_eq!(record.endpoint_request["reasoning_effort"], "high");
        assert_eq!(record.endpoint_request["verbosity"], "detailed");
        assert_eq!(record.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(record.speciality_values["verbosity"], "detailed");
        assert_eq!(record.effective_specialities, expected_specialities);
        assert_eq!(record.voucher_specialities, expected_specialities);
        assert_eq!(record.top_k, Some(42));
        assert_eq!(record.min_p, Some(0.08));
    }
}

#[tokio::test]
async fn capability_body_transport_rejects_unknown_fields_and_levels_before_dispatch() {
    let provider = "5b".repeat(32);
    let model = routed_test_model_with_specialities(std::slice::from_ref(&provider));
    let records = Arc::new(Mutex::new(Vec::new()));
    let state = test_gateway_state_from_models(vec![model]).with_session_backend(Arc::new(
        SpecialityRecordingBackend {
            records: records.clone(),
        },
    ));
    let app = openai_router(state);

    let (status, body) = json_request(
        app.clone(),
        Method::POST,
        "/v1/chat/completions",
        json!({
            "model": "mayhem/routed-test",
            "messages": [{"role": "user", "content": "reject unknown"}],
            "provider_native": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"]
        .as_str()
        .expect("unknown-field error")
        .contains("provider_native"));

    let (status, body) = json_request(
        app,
        Method::POST,
        "/v1/chat/completions",
        json!({
            "model": "mayhem/routed-test",
            "messages": [{"role": "user", "content": "reject unsupported"}],
            "reasoning_effort": "medium"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"]
        .as_str()
        .expect("unsupported-level error")
        .contains("reasoning_effort"));
    assert!(records
        .lock()
        .expect("speciality transport records")
        .is_empty());
}

#[tokio::test]
async fn chat_completion_streams_openai_sse_chunks_with_usage() {
    let model = first_model_id().await;
    let request = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Stream a short answer." }],
        "stream": true,
        "stream_options": { "include_usage": true }
    });
    let (status, headers, bytes) = raw_request(
        test_app(),
        Method::POST,
        "/v1/chat/completions",
        Some(request),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .starts_with("text/event-stream"));
    let body = String::from_utf8(bytes).expect("SSE body is utf8");
    assert!(body.contains("data: {"));
    assert!(body.contains("\"object\":\"chat.completion.chunk\""));
    assert!(body.contains("\"choices\":[]"));
    assert!(body.contains("\"mayhem\":{"));
    assert!(body.contains("\"backend\":\"local-openai-shape\""));
    assert!(body.contains("\"direct_session\":false"));
    assert!(body.contains("\"billable\":false"));
    assert!(body.contains("\"dev_session\":true"));
    assert!(body.contains("\"receipt\":null"));
    assert!(body.contains("data: [DONE]"));
}

#[tokio::test]
async fn chat_completion_returns_all_normalized_tool_calls() {
    let mut model = routed_test_model();
    model.mayhem.adapter = ShapeAdapterInfo {
        tool_call_strategy: "openai_tool_calls".to_owned(),
        ..ShapeAdapterInfo::default()
    };
    let state = test_gateway_state_from_models(vec![model])
        .with_session_backend(Arc::new(ToolCallDirectSessionBackend));
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Write both files." }],
        "tools": [{
            "type": "function",
            "function": { "name": "write", "parameters": { "type": "object" } }
        }]
    });
    let (status, body) = json_request(
        openai_router(state),
        Method::POST,
        "/v1/chat/completions",
        request,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let calls = body["choices"][0]["message"]["tool_calls"]
        .as_array()
        .expect("tool call array");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["id"], "call-normalized-1");
    assert_eq!(
        calls[0]["function"]["arguments"],
        r#"{"filePath":"one.txt"}"#
    );
    assert_eq!(calls[1]["id"], "call-normalized-2");
    assert_eq!(
        calls[1]["function"]["arguments"],
        r#"{"filePath":"two.txt"}"#
    );
    assert_eq!(body["choices"][0]["finish_reason"], "tool_calls");
}

#[tokio::test]
async fn chat_completion_streams_normalized_tool_call_delta() {
    let mut model = routed_test_model();
    model.mayhem.adapter = ShapeAdapterInfo {
        tool_call_strategy: "openai_tool_calls".to_owned(),
        ..ShapeAdapterInfo::default()
    };
    let state = test_gateway_state_from_models(vec![model])
        .with_session_backend(Arc::new(ToolCallDirectSessionBackend));
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Write a file." }],
        "tools": [{
            "type": "function",
            "function": { "name": "write", "parameters": { "type": "object" } }
        }],
        "stream": true
    });
    let (status, headers, bytes) = raw_request(
        openai_router(state),
        Method::POST,
        "/v1/chat/completions",
        Some(request),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .starts_with("text/event-stream"));
    let body = String::from_utf8(bytes).expect("SSE body is utf8");
    assert!(body.contains("\"tool_calls\":["));
    assert!(body.contains("\"id\":\"call-normalized-1\""));
    assert!(body.contains("\"id\":\"call-normalized-2\""));
    assert!(body.contains("\"index\":0"));
    assert!(body.contains("\"index\":1"));
    assert!(body.contains("\"type\":\"function\""));
    assert!(body.contains("\"name\":\"write\""));
    assert!(body.contains("\"arguments\":\"{\\\"filePath\\\":\\\"one.txt\\\"}\""));
    assert!(body.contains("\"arguments\":\"{\\\"filePath\\\":\\\"two.txt\\\"}\""));
    assert!(body.contains("\"finish_reason\":\"tool_calls\""));
    assert!(body.contains("data: [DONE]"));
}

#[tokio::test]
async fn streaming_dev_chat_is_unbillable_and_stores_no_receipt() {
    let (state, app) = test_state_and_app();
    let model = first_model_id().await;
    let request = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Stream without billable accounting." }],
        "stream": true,
        "stream_options": { "include_usage": true }
    });

    let (status, headers, bytes) = raw_request(
        app.clone(),
        Method::POST,
        "/v1/chat/completions",
        Some(request),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .starts_with("text/event-stream"));
    let body = String::from_utf8(bytes).expect("SSE body is utf8");
    assert!(body.contains("\"mayhem\":{"));
    assert!(body.contains("\"billable\":false"));
    assert!(body.contains("\"dev_session\":true"));
    assert!(body.contains("\"receipt\":null"));
    assert!(state.receipts().is_empty());

    let (status, body) = json_request(app, Method::GET, "/mayhem/receipts", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().expect("receipt list").len(), 0);
    assert_eq!(body["paused"].as_array().expect("paused list").len(), 0);
}

#[tokio::test]
async fn refused_receipt_cosign_pauses_session_without_storing_receipt() {
    let state = test_gateway_state_from_models(vec![routed_test_model()])
        .with_session_backend(Arc::new(TestDirectSessionBackend))
        .with_receipt_cosign_enabled(false);
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "This should pause." }],
        "stream": true,
        "stream_options": { "include_usage": true }
    });

    let (status, body) =
        json_request(app.clone(), Method::POST, "/v1/chat/completions", request).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("session paused"));
    assert!(state.receipts().is_empty());
    let paused = state.paused_sessions();
    assert_eq!(paused.len(), 1);
    assert!(paused[0].reason.contains("co-signing refused"));

    let (status, body) =
        json_request(app.clone(), Method::GET, "/mayhem/status", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sessions_paused"], 1);
    assert_eq!(body["receipts"], 0);

    let (status, body) = json_request(app, Method::GET, "/mayhem/receipts", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().expect("receipt list").len(), 0);
    assert_eq!(body["paused"].as_array().expect("paused list").len(), 1);
}

#[tokio::test]
async fn response_format_json_object_returns_parseable_json_content() {
    let model = first_model_id().await;
    let request = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Return JSON." }],
        "response_format": { "type": "json_object" }
    });
    let (status, body) =
        json_request(test_app(), Method::POST, "/v1/chat/completions", request).await;
    assert_eq!(status, StatusCode::OK);
    let content = body["choices"][0]["message"]["content"]
        .as_str()
        .expect("json content");
    let parsed: Value = serde_json::from_str(content).expect("assistant content is JSON");
    assert_eq!(parsed["ok"], true);
}

#[tokio::test]
async fn legacy_completions_return_text_completion_shape_and_stream() {
    let (state, app) = test_state_and_app();
    let model = first_model_id().await;
    let request = json!({ "model": model, "prompt": "Hello", "max_tokens": 8 });
    let (status, body) = json_request(app.clone(), Method::POST, "/v1/completions", request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "text_completion");
    assert!(body["choices"][0]["text"]
        .as_str()
        .expect("completion text")
        .contains("Mayhem response"));
    assert_eq!(body["mayhem"]["billable"], false);
    assert_eq!(body["mayhem"]["dev_session"], true);
    assert_eq!(body["mayhem"]["receipt"], Value::Null);

    let request = json!({ "model": first_model_id().await, "prompt": "Hello", "stream": true });
    let (status, headers, bytes) =
        raw_request(app, Method::POST, "/v1/completions", Some(request)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .starts_with("text/event-stream"));
    let body = String::from_utf8(bytes).expect("SSE body is utf8");
    assert!(body.contains("\"object\":\"text_completion\""));
    assert!(body.contains("\"billable\":false"));
    assert!(body.contains("\"dev_session\":true"));
    assert!(body.contains("\"receipt\":null"));
    assert!(body.contains("data: [DONE]"));
    assert!(state.receipts().is_empty());
}

#[tokio::test]
async fn mayhem_local_endpoints_report_status_receipts_and_balance() {
    let (status, body) = json_request(test_app(), Method::GET, "/mayhem/status", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert_eq!(body["backend"], "local-openai-shape");
    assert_eq!(body["dev_session_shim"], true);
    assert_eq!(body["github_update"]["state"], "disabled");
    assert!(body["github_update"]["update"].is_null());

    let (status, body) =
        json_request(test_app(), Method::GET, "/mayhem/receipts", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "list");

    let (status, body) =
        json_request(test_app(), Method::GET, "/mayhem/balance", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["denom"], "au_usd");
}

#[tokio::test]
async fn dashboard_requires_token_sets_csp_and_serves_no_external_assets() {
    let state = GatewayState::from_embedded_catalog();
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let query = dashboard_path
        .strip_prefix("/mayhem/dashboard")
        .expect("dashboard path");
    let app = openai_router(state);

    let (status, headers, bytes) =
        raw_request(app.clone(), Method::GET, "/mayhem/dashboard", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let csp = headers
        .get("content-security-policy")
        .and_then(|value| value.to_str().ok())
        .expect("dashboard CSP header");
    assert!(csp.contains("connect-src 'self' http://127.0.0.1:*"));
    assert!(!csp.contains("https:"));
    assert!(!csp.contains("http://") || csp.contains("http://127.0.0.1:*"));
    assert_eq!(
        headers
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let locked = String::from_utf8(bytes).expect("locked dashboard html");
    assert_no_external_urls(&locked);

    let (status, headers, _) = raw_request(app.clone(), Method::GET, dashboard_path, None).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        headers
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/mayhem/dashboard")
    );
    let set_cookie = headers
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .expect("dashboard session cookie");
    assert!(set_cookie.contains("Max-Age=34560000"));
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("Secure"));
    assert!(set_cookie.contains("SameSite=Strict"));
    assert!(set_cookie.starts_with("__Secure-mayhem_dashboard_"));
    let cookie = set_cookie
        .split(';')
        .next()
        .expect("dashboard cookie pair")
        .to_owned();

    let (status, _, bytes) = raw_request_with_headers(
        app.clone(),
        Method::GET,
        "/mayhem/dashboard",
        None,
        &[("cookie", &cookie)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("dashboard html after bootstrap redirect");
    assert!(body.contains(r#"class="app-shell""#));
    assert!(body.contains(r#"href="/mayhem/dashboard/models""#));
    assert!(body.contains(r#"href="/mayhem/dashboard/network""#));
    assert!(body.contains("Authenticated for this gateway run"));
    assert_no_external_urls(&body);

    let filtered_path = format!("{dashboard_path}&page=2");
    let (status, filtered_headers, _) = raw_request_with_headers(
        app.clone(),
        Method::GET,
        &filtered_path,
        None,
        &[("cookie", &cookie)],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        filtered_headers
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/mayhem/dashboard?page=2")
    );
    assert!(!filtered_headers
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .contains("token="));

    let (status, _, _) = raw_request_with_headers(
        app.clone(),
        Method::GET,
        "/mayhem/dashboard",
        None,
        &[("cookie", &cookie)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let slash_path = format!("/mayhem/dashboard/{query}");
    let (status, slash_headers, _) = raw_request_with_headers(
        app.clone(),
        Method::GET,
        &slash_path,
        None,
        &[("cookie", &cookie)],
    )
    .await;
    assert_eq!(status, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        slash_headers
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/mayhem/dashboard")
    );
    let slash_cookie = slash_headers
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .expect("trailing-slash bootstrap sets a browser session cookie")
        .to_owned();
    let (status, _, _) = raw_request_with_headers(
        app.clone(),
        Method::GET,
        "/mayhem/dashboard",
        None,
        &[("cookie", &slash_cookie)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = json_request_with_headers(
        app.clone(),
        Method::GET,
        "/mayhem/dashboard/session",
        Value::Null,
        &[("cookie", &cookie)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert_eq!(body["session_scope"], "gateway_process");
    assert!(body.get("expires_in_seconds").is_none());

    let (status, _, _) = raw_request(app.clone(), Method::GET, dashboard_path, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let restarted = openai_router(GatewayState::from_embedded_catalog());
    let (status, restarted_headers, _) = raw_request_with_headers(
        restarted,
        Method::GET,
        "/mayhem/dashboard",
        None,
        &[("cookie", &cookie)],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(restarted_headers
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("Max-Age=0")));
}

#[tokio::test]
async fn dashboard_uses_local_design_system_and_font_asset() {
    let state = GatewayState::from_embedded_catalog();
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let app = openai_router(state);

    let (status, headers, bytes) =
        dashboard_request_with_headers(app.clone(), Method::GET, dashboard_path, None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("dashboard html");
    for expected in [
        r#"href="/mayhem/dashboard/assets/app.css""#,
        r#"src="/mayhem/dashboard/assets/app.js""#,
        r#"class="app-shell""#,
        r#"class="app-brand""#,
        r#"class="state-indicator"#,
        "Authenticated for this gateway run",
    ] {
        assert!(body.contains(expected), "missing {expected}");
    }
    assert!(!body.contains("/mayhem/dashboard/components"));
    assert_no_external_urls(&body);

    let cookie = headers
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .expect("dashboard session cookie")
        .to_owned();
    let (status, css_headers, css_bytes) = raw_request_with_headers(
        app.clone(),
        Method::GET,
        "/mayhem/dashboard/assets/app.css",
        None,
        &[("cookie", &cookie)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        css_headers
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/css; charset=utf-8")
    );
    let css = String::from_utf8(css_bytes).expect("dashboard CSS");
    for expected in [
        "@font-face",
        "/mayhem/dashboard/assets/exo-latin.woff2",
        "--app-panel",
        ".app-shell",
        "@media(max-width:780px)",
    ] {
        assert!(css.contains(expected), "missing {expected}");
    }
    assert_no_external_urls(&css);

    let (status, headers, bytes) = raw_request_with_headers(
        app.clone(),
        Method::GET,
        "/mayhem/dashboard/assets/exo-latin.woff2",
        None,
        &[("cookie", &cookie)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("font/woff2")
    );
    assert!(bytes.len() > 10_000);
}

#[tokio::test]
async fn user_dashboard_renders_live_gateway_data() {
    let state = GatewayState::from_embedded_catalog()
        .with_dev_session_shim()
        .with_receipt_balance_au(1_000_000_000_000_000_000)
        .with_receipt_rail("tap")
        .with_payment_directory(json!({
            "payments": {
                "fiat": {
                    "integration_currency": "usd",
                    "adaptive_pricing": true,
                    "payout_currencies": ["eur", "gbp", "usd"]
                }
            },
            "rates": {
                "tap": {
                    "usd": "0.123456",
                    "source": "uniswap-v2",
                    "age_seconds": 12
                }
            }
        }));
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let query = dashboard_path
        .strip_prefix("/mayhem/dashboard")
        .expect("dashboard query");
    let connect_path = format!("/mayhem/dashboard/connect{query}");
    let models_path = format!("/mayhem/dashboard/models{query}");
    let app = openai_router(state);
    let model = first_model_id().await;
    let request = json!({
        "model": model,
        "messages": [{"role": "user", "content": "hello"}]
    });
    let (status, _) =
        json_request(app.clone(), Method::POST, "/v1/chat/completions", request).await;
    assert_eq!(status, StatusCode::OK);
    let mut browser = DashboardTestBrowser::default();

    let (status, _, bytes) = browser
        .request(
            app.clone(),
            Method::GET,
            dashboard_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("dashboard html");
    assert!(body.contains(r#"class="app-shell""#));
    assert!(body.contains("Ledger balance"));
    assert!(body.contains("$1.00"));
    assert!(body.contains("Final receipts"));
    assert!(body.contains("Recent activity"));
    assert!(!body.contains("1,240.00 TAP"));
    assert_no_external_urls(&body);

    let (status, _, bytes) = browser
        .request(
            app.clone(),
            Method::GET,
            &connect_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let connect = String::from_utf8(bytes).expect("connect dashboard html");
    assert!(connect.contains("Connection details"));
    assert!(connect.contains("http://127.0.0.1:11435/v1"));
    assert!(connect.contains("OPENAI_BASE_URL=http://127.0.0.1:11435/v1"));
    assert_no_external_urls(&connect);

    let (status, _, bytes) = browser
        .request(
            app,
            Method::GET,
            &models_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let models = String::from_utf8(bytes).expect("models dashboard html");
    assert!(models.contains("Model catalog"));
    assert!(models.contains(&model));
    assert_no_external_urls(&models);
}

#[tokio::test]
async fn models_dashboard_shows_canonical_route_count() {
    let providers = vec!["41".repeat(32), "42".repeat(32), "43".repeat(32)];
    let mut model = routed_test_model_with_providers(&providers);
    model.mayhem.providers_online = 2;
    let state = test_gateway_state_from_models(vec![model]);
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let models_path = dashboard_path.replacen("/mayhem/dashboard?", "/mayhem/dashboard/models?", 1);
    let app = openai_router(state);

    let (status, _, bytes) = dashboard_request_with_headers(
        app,
        Method::GET,
        &models_path,
        None,
        &[("host", "127.0.0.1:11435")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("models dashboard html");
    assert!(body.contains("Model catalog"));
    assert!(body.contains("mayhem/routed-test"));
    assert!(body.contains("3 routes accepting work; 3 fresh"));
    assert_no_external_urls(&body);
}

#[tokio::test]
async fn dashboard_page_query_reaches_later_models_and_clamps_invalid_values() {
    let provider = "ab".repeat(32);
    let seed = routed_test_model_with_providers(std::slice::from_ref(&provider));
    let models = (0..82)
        .map(|index| {
            let mut model = seed.clone();
            model.id = format!("mayhem/page-{index:03}");
            model.mayhem.markets.clear();
            model
        })
        .collect::<Vec<_>>();
    let state = test_gateway_state_from_models(models);
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let first_path = dashboard_url
        .replacen("/mayhem/dashboard?", "/mayhem/dashboard/models?", 1)
        .strip_prefix("http://127.0.0.1:11435")
        .expect("models dashboard url is rooted at gateway")
        .to_owned();
    let second_path = format!("{first_path}&page=4");
    let invalid_path = format!("{first_path}&page=not-a-page");
    let out_of_range_path = format!("{first_path}&page=999");
    let app = openai_router(state);
    let mut browser = DashboardTestBrowser::default();

    let (first_status, _, first_bytes) = browser
        .request(
            app.clone(),
            Method::GET,
            &first_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    let first = String::from_utf8(first_bytes).expect("first models page html");
    assert_eq!(first_status, StatusCode::OK, "{first}");
    assert!(first.contains("mayhem/page-000"), "{first}");
    assert!(!first.contains("mayhem/page-080"));
    assert!(first.contains("Showing rows 1&ndash;25 of 82 catalog models. Page 1 of 4."));
    assert!(first.contains(r#"rel="next" href="/mayhem/dashboard/models?page=2""#));

    let (status, _, second_bytes) = browser
        .request(
            app.clone(),
            Method::GET,
            &second_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let second = String::from_utf8(second_bytes).expect("second models page html");
    assert!(!second.contains("mayhem/page-000"));
    assert!(second.contains("mayhem/page-080"));
    assert!(second.contains("mayhem/page-081"));
    assert!(second.contains("Showing rows 76&ndash;82 of 82 catalog models. Page 4 of 4."));
    assert!(second.contains(r#"rel="prev" href="/mayhem/dashboard/models?page=3""#));

    let (_, _, invalid_bytes) = browser
        .request(
            app.clone(),
            Method::GET,
            &invalid_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    let invalid = String::from_utf8(invalid_bytes).expect("invalid models page html");
    assert!(invalid.contains("mayhem/page-000"));
    assert!(invalid.contains("Page 1 of 4."));

    let (_, _, clamped_bytes) = browser
        .request(
            app,
            Method::GET,
            &out_of_range_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    let clamped = String::from_utf8(clamped_bytes).expect("clamped models page html");
    assert!(clamped.contains("mayhem/page-081"));
    assert!(clamped.contains("Page 4 of 4."));
}

#[tokio::test]
async fn provider_dashboard_renders_routes_receipts_and_earnings() {
    let provider = "55".repeat(32);
    let state = test_gateway_state_from_models(vec![routed_test_model_with_providers(
        std::slice::from_ref(&provider),
    )])
    .with_provider_earnings(vec![json!({
        "provider": provider,
        "rail": "tap",
        "denom": "au_usd",
        "total_au": "2500000000000000000",
        "held_au": "500000000000000000",
        "paid_cum_au": "250000000000000000",
        "released_au": "1750000000000000000",
        "claimable_au": "1750000000000000000",
        "claim_model": "tap_non_custodial_claim",
        "holdbacks": [{"epoch": 7, "au": "500000000000000000"}],
        "updated_epoch": 9_u64
    })])
    .with_local_provider_id(provider.clone())
    .with_receipt_rail("tap")
    .with_session_backend(Arc::new(TestDirectSessionBackend));
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let query = dashboard_path
        .strip_prefix("/mayhem/dashboard?")
        .expect("dashboard token query");
    let earn_path = format!("/mayhem/dashboard/earn?{query}");
    let earnings_path = format!("/mayhem/dashboard/earn/earnings?{query}");
    let activity_path = format!("/mayhem/dashboard/activity?{query}");
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{"role": "user", "content": "serve this provider session"}]
    });
    let (status, _) =
        json_request(app.clone(), Method::POST, "/v1/chat/completions", request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state.receipts().len(), 1);
    assert_eq!(state.receipts()[0].receipt.body.provider, provider);
    let mut browser = DashboardTestBrowser::default();

    let (status, _, bytes) = browser
        .request(
            app.clone(),
            Method::GET,
            &earn_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("earn dashboard html");
    assert!(body.contains("Provider operations"));
    assert!(body.contains("Provider identity:"));
    assert!(!body.contains("Configured gateway identity:"));
    assert!(body.contains("Settlement snapshot"));
    assert!(body.contains("$1.75"));
    assert_no_external_urls(&body);

    let (status, _, bytes) = browser
        .request(
            app.clone(),
            Method::GET,
            &activity_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let activity = String::from_utf8(bytes).expect("activity dashboard html");
    assert!(activity.contains("Session activity"));
    assert!(activity.contains("mayhem/routed-test"));
    assert!(activity.contains("Final receipt"));
    assert_no_external_urls(&activity);

    let (status, _, bytes) = browser
        .request(
            app,
            Method::GET,
            &earnings_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let earnings = String::from_utf8(bytes).expect("earnings dashboard html");
    assert!(earnings.contains("Earnings records"));
    assert!(earnings.contains("$2.50"));
    assert!(earnings.contains("$0.50"));
    assert!(earnings.contains("$1.75"));
    assert!(earnings.contains("$0.25"));
    assert!(earnings.contains("Ledger epoch"));
    assert!(earnings.contains("<td>9</td>"));
    assert!(earnings.contains("data-evidence-url"));
    assert_no_external_urls(&earnings);
}

#[tokio::test]
async fn network_provider_dashboard_counts_multi_enclave_routes() {
    let provider = "58".repeat(32);
    let mut chat = routed_test_model_with_providers(std::slice::from_ref(&provider));
    chat.id = "mayhem/chat-small".to_owned();
    chat.mayhem.route_candidates[0].enclave_id = "11".repeat(32);
    chat.mayhem.route_candidates[0].room_id = "aa".repeat(16);
    let mut embedding = routed_test_model_with_providers(std::slice::from_ref(&provider));
    embedding.id = "mayhem/embed-small".to_owned();
    embedding.mayhem.model_class = "embedding".to_owned();
    embedding.mayhem.caps.output_modality = Some("embedding".to_owned());
    embedding.mayhem.caps.output_modalities = vec!["embedding".to_owned()];
    embedding.mayhem.route_candidates[0].enclave_id = "22".repeat(32);
    embedding.mayhem.route_candidates[0].room_id = "bb".repeat(16);
    let state = test_gateway_state_from_models(vec![chat, embedding]);
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let query = dashboard_path
        .strip_prefix("/mayhem/dashboard?")
        .expect("dashboard token query");
    let provider_path = format!("/mayhem/dashboard/network/providers?{query}");
    let app = openai_router(state);

    let (status, _, bytes) = dashboard_request_with_headers(
        app,
        Method::GET,
        &provider_path,
        None,
        &[("host", "127.0.0.1:11435")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("network providers dashboard html");
    assert!(body.contains(r#"id="provider-table""#));
    assert!(body.contains(r#"id="provider-count">2 shown rows"#));
    assert!(body.contains("mayhem/chat-small"));
    assert!(body.contains("mayhem/embed-small"));
    assert!(body.contains(&"11".repeat(32)));
    assert!(body.contains(&"22".repeat(32)));
    assert_no_external_urls(&body);
}

#[tokio::test]
async fn provider_dashboard_renders_local_load_progress() {
    let provider = "66".repeat(32);
    let model = routed_test_model_with_providers(std::slice::from_ref(&provider));
    let enclave_id = model.mayhem.route_candidates[0].enclave_id.clone();
    let progress_dir = tempfile::tempdir().expect("progress tempdir");
    fs::write(
        progress_dir.path().join("provider-progress.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": 1,
            "provider": provider,
            "model_id": "mayhem/routed-test",
            "enclave_id": enclave_id,
            "artifact": "gguf-q4_k_m",
            "label": "gguf-q4_k_m cached artifact",
            "phase": "verify",
            "status": "running",
            "position": 42_u64,
            "total": 100_u64,
            "percent": 42_u64,
            "updated_at_ms": current_test_millis()
        }))
        .expect("progress json"),
    )
    .expect("write progress");
    let state = test_gateway_state_from_models(vec![model])
        .with_local_provider_id(provider.clone())
        .with_provider_load_progress_dir(progress_dir.path().to_path_buf());
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let query = dashboard_path
        .strip_prefix("/mayhem/dashboard?")
        .expect("dashboard token query");
    let provider_path = format!("/mayhem/dashboard/earn/machines?{query}");
    let app = openai_router(state);

    let (status, _, bytes) = dashboard_request_with_headers(
        app,
        Method::GET,
        &provider_path,
        None,
        &[("host", "127.0.0.1:11435")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("provider dashboard html");
    assert!(body.contains("Preparing model"));
    assert!(body.contains("gguf-q4_k_m cached artifact"));
    assert!(body.contains("verify 42%"));
    assert!(body.contains(r#"progress max="100" value="42" aria-label="verify progress""#));
    assert_no_external_urls(&body);
}

#[tokio::test]
async fn provider_dashboard_renders_progress_before_route_exists() {
    let provider = "77".repeat(32);
    let enclave_id = "88".repeat(32);
    let progress_dir = tempfile::tempdir().expect("progress tempdir");
    let updated_at_ms = current_test_millis();
    fs::write(
        progress_dir.path().join("provider-progress.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": 1,
            "provider": provider,
            "model_id": "mayhem/loading-test",
            "enclave_id": enclave_id,
            "artifact": "gguf-q4_k_m",
            "label": "gguf-q4_k_m artifact",
            "phase": "download",
            "status": "running",
            "position": 7_u64,
            "total": 10_u64,
            "percent": 70_u64,
            "updated_at_ms": updated_at_ms
        }))
        .expect("progress json"),
    )
    .expect("write progress");
    let state = test_gateway_state_from_models(Vec::new())
        .with_local_provider_id(provider.clone())
        .with_provider_load_progress_dir(progress_dir.path().to_path_buf());
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let query = dashboard_path
        .strip_prefix("/mayhem/dashboard?")
        .expect("dashboard token query");
    let provider_path = format!("/mayhem/dashboard/earn/machines?{query}");
    let app = openai_router(state);

    let (status, _, bytes) = dashboard_request_with_headers(
        app,
        Method::GET,
        &provider_path,
        None,
        &[("host", "127.0.0.1:11435")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("provider dashboard html");
    assert!(body.contains("Preparing a model"));
    assert!(body.contains("gguf-q4_k_m artifact"));
    assert!(body.contains("download 70%"));
    assert!(body.contains(r#"progress max="100" value="70" aria-label="download progress""#));
    assert!(body.contains("No machine routes yet"));
    assert_no_external_urls(&body);
}

#[tokio::test]
async fn provider_dashboard_hides_stale_progress_before_route_exists() {
    let provider = "77".repeat(32);
    let enclave_id = "88".repeat(32);
    let progress_dir = tempfile::tempdir().expect("progress tempdir");
    let stale_at_ms = current_test_millis().saturating_sub(10 * 60 * 1000);
    fs::write(
        progress_dir.path().join("provider-progress.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": 1,
            "provider": provider,
            "model_id": "mayhem/stale-loading-test",
            "enclave_id": enclave_id,
            "artifact": "gguf-q4_k_m",
            "label": "gguf-q4_k_m artifact",
            "phase": "download",
            "status": "running",
            "position": 7_u64,
            "total": 10_u64,
            "percent": 70_u64,
            "updated_at_ms": stale_at_ms
        }))
        .expect("progress json"),
    )
    .expect("write progress");
    let state = test_gateway_state_from_models(Vec::new())
        .with_local_provider_id(provider.clone())
        .with_provider_load_progress_dir(progress_dir.path().to_path_buf());
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let query = dashboard_path
        .strip_prefix("/mayhem/dashboard?")
        .expect("dashboard token query");
    let provider_path = format!("/mayhem/dashboard/earn?{query}");
    let app = openai_router(state);

    let (status, _, bytes) = dashboard_request_with_headers(
        app,
        Method::GET,
        &provider_path,
        None,
        &[("host", "127.0.0.1:11435")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("provider dashboard html");
    assert!(!body.contains("mayhem/stale-loading-test"));
    assert!(!body.contains("download 70%"));
    assert!(!body.contains("<progress"));
    assert!(body.contains("Setup incomplete"));
    assert!(
        body.contains("No catalog route matches the provider identity configured on this gateway.")
    );
    assert!(body.contains("No serving routes yet"));
    assert_no_external_urls(&body);
}

#[tokio::test]
async fn network_dashboard_renders_live_catalog_and_provider_state() {
    let provider_a = "88".repeat(32);
    let provider_b = "99".repeat(32);
    let mut model = routed_test_model_with_providers(&[provider_a.clone(), provider_b.clone()]);
    model.mayhem.providers_online = 2;
    model.mayhem.rooms = 2;
    let mut unavailable = model.clone();
    unavailable.id = "mayhem/unavailable-test".to_owned();
    unavailable.mayhem.source = "catalog".to_owned();
    unavailable.mayhem.providers_online = 0;
    unavailable.mayhem.rooms = 0;
    unavailable.mayhem.route_candidates.clear();

    let heartbeat = test_provider_heartbeat(
        &model,
        &model.mayhem.route_candidates[0],
        0.42,
        2,
        4,
        Some(76.5),
        321,
    );
    let state = GatewayState::from_models(vec![model, unavailable])
        .with_provider_heartbeats(vec![heartbeat]);
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let network_url = dashboard_url.replacen("/mayhem/dashboard?", "/mayhem/dashboard/network?", 1);
    let network_path = network_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("network dashboard url is rooted at gateway")
        .to_owned();
    let network_models_path = network_path.replacen(
        "/mayhem/dashboard/network?",
        "/mayhem/dashboard/network/models?",
        1,
    );
    let network_providers_path = network_path.replacen(
        "/mayhem/dashboard/network?",
        "/mayhem/dashboard/network/providers?",
        1,
    );
    let app = openai_router(state);
    let mut browser = DashboardTestBrowser::default();

    let (status, _, bytes) = browser
        .request(
            app.clone(),
            Method::GET,
            &network_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("network dashboard html");
    assert!(body.contains("Network health"));
    assert!(body.contains("Catalog models"));
    assert!(body.contains("Providers"));
    assert!(body.contains("Fresh routes"));
    assert!(body.contains("Supply exceptions"));
    assert!(body.contains("mayhem/unavailable-test"));
    assert_no_external_urls(&body);

    let (status, _, bytes) = browser
        .request(
            app.clone(),
            Method::GET,
            &network_models_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let models = String::from_utf8(bytes).expect("network models dashboard html");
    assert!(models.contains("Network models"));
    assert!(models.contains("mayhem/routed-test"));
    assert!(models.contains("mayhem/unavailable-test"));
    assert!(models.contains("Capacity advertised"));
    assert!(models.contains("No provider route"));
    assert_no_external_urls(&models);

    let (status, _, bytes) = browser
        .request(
            app,
            Method::GET,
            &network_providers_path,
            None,
            &[("host", "127.0.0.1:11435")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let providers = String::from_utf8(bytes).expect("network providers dashboard html");
    assert!(providers.contains(r#"id="provider-table""#));
    assert!(providers.contains(r#"id="provider-count">2 shown rows"#));
    assert!(providers.contains(&provider_a));
    assert!(providers.contains(&provider_b));
    assert!(providers.contains("Capacity advertised"));
    assert!(providers.contains("2 / 4 active · 1 free"));
    assert!(providers.contains("321ms · 76.5 tok/s"));
    assert!(providers.contains("data-evidence-url"));
    assert_no_external_urls(&providers);
}

#[test]
fn dashboard_bind_refuses_unspecified_and_lan_addresses() {
    assert!(
        validate_loopback_dashboard_bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 11_435))).is_ok()
    );
    assert!(validate_loopback_dashboard_bind(SocketAddr::from(([0, 0, 0, 0], 11_435))).is_err());
    assert!(
        validate_loopback_dashboard_bind(SocketAddr::from(([192, 168, 1, 20], 11_435))).is_err()
    );
}

fn assert_no_external_urls(html: &str) {
    assert!(!html.contains("https://"));
    for (index, _) in html.match_indices("http://") {
        assert!(
            html[index..].starts_with("http://127.0.0.1"),
            "unexpected non-local URL in dashboard HTML: {}",
            &html[index..html.len().min(index + 80)]
        );
    }
}
