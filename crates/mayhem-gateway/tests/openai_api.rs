use axum::{
    body::{to_bytes, Body},
    http::{HeaderMap, Method, Request, StatusCode},
    Router,
};
use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use mayhem_gateway::openai::{
    openai_router, validate_loopback_dashboard_bind, AudioSpeechOutput, AudioSpeechRequest,
    AudioTranscriptionOutput, AudioTranscriptionRequest, ChatCompletionRequest, ChatMessage,
    ChatOutput, EmbeddingOutput, EmbeddingRequest, GatewayArtifactOutput, GatewayAudioSpeechFuture,
    GatewayAudioSpeechResult, GatewayAudioTranscriptionFuture, GatewayAudioTranscriptionResult,
    GatewayCanaryModelConfig, GatewayCanaryProbePolicy, GatewayCanaryPrompt, GatewayCanaryRegistry,
    GatewayEmbeddingFuture, GatewayEmbeddingResult, GatewayImageGenerationFuture,
    GatewayImageGenerationResult, GatewayLocalRunBadge, GatewayModel, GatewayRouteCandidate,
    GatewaySessionBackend, GatewaySessionError, GatewaySessionFuture, GatewaySessionInvocation,
    GatewaySessionResult, GatewayState, ImageGenerationOutput, ImageGenerationRequest,
    MayhemModelInfo, ModelCaps, PriceRefAu, ProviderSignedReceipt, ShapeAdapterInfo,
    ToolCallOutput, Usage,
};
use mayhem_gateway::{
    aggregate_canary_fingerprints, normalize_rate_map, priced_usage_au, text_generation_rate_map,
    token_fingerprint, HeartbeatAttestation, HeartbeatCaps, HeartbeatPerf, HeartbeatQueue,
    HeartbeatSlots, ProviderHeartbeat, ReputationEventKind, HEARTBEAT_SCHEMA_VERSION,
};
use mayhem_proto::{
    catalog_enclave_id, receipt_signing_bytes, CatalogEnclaveIdentity, ReceiptBody, ReceiptUsage,
    CONTRACT_VERSION, DEFAULT_MODEL_CLASS, SESSION_RECEIPT_SCHEMA_VERSION, USAGE_AUDIO_SECOND,
    USAGE_IMAGE, USAGE_INPUT_CHARACTER, USAGE_STEP,
};
use serde_json::{json, Value};
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
                    tool_call: None,
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
        _request: &'a EmbeddingRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewayEmbeddingFuture<'a> {
        Box::pin(async move {
            Ok(GatewayEmbeddingResult {
                output: EmbeddingOutput {
                    embeddings: vec![vec![0.12, 0.34, 0.56], vec![0.11, 0.33, 0.55]],
                    usage: Usage {
                        prompt_tokens: 2,
                        completion_tokens: 0,
                        total_tokens: 2,
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
                    text: "hello mayhem".to_owned(),
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
                    tool_call: Some(ToolCallOutput {
                        id: "call-normalized".to_owned(),
                        name: "write".to_owned(),
                        arguments: r#"{"filePath":"ok.txt"}"#.to_owned(),
                    }),
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
                    tool_call: None,
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
                    tool_call: None,
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
                    tool_call: None,
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
                    tool_call: None,
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
        invocation: &'a GatewaySessionInvocation,
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
                    tool_call: None,
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
                    tool_call: None,
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
                    tool_call: None,
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
    openai_router(GatewayState::from_embedded_catalog().with_dev_session_shim())
}

fn test_state_and_app() -> (GatewayState, Router) {
    let state = GatewayState::from_embedded_catalog().with_dev_session_shim();
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
        .contains("local dev shim"));
    assert!(state.receipts().is_empty());

    let (status, body) = json_request(app, Method::GET, "/mayhem/status", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["backend"], "no-live-provider");
    assert_eq!(body["dev_session_shim"], false);
}

#[tokio::test]
async fn models_endpoint_returns_openai_list_shape_with_mayhem_extension() {
    let (status, body) = json_request(test_app(), Method::GET, "/v1/models", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "list");
    assert!(body["data"].as_array().expect("model data").len() >= 2);
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
    assert_eq!(body["data"][0]["mayhem"]["caps"]["tools"], true);
    assert_eq!(
        body["data"][0]["mayhem"]["adapter"]["tool_call_strategy"],
        "mayhem_json"
    );
}

#[tokio::test]
async fn models_endpoint_surfaces_tier2_attestation_counts_from_catalog() {
    let catalog = json!({
        "models": [{
            "model_id": "mayhem/tier2-model",
            "model_class": "embedding",
            "caps": { "tools": true, "json": true, "ctx_max": 4096, "vision": false },
            "price_ref_au": {
                "denom": "au_usd",
                "ver": 1,
                "rate_map": [
                    { "unit": "input_token", "per_unit_au": "10", "granularity": 1000 },
                    { "unit": "output_token", "per_unit_au": "30", "granularity": 1000 }
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
    assert_eq!(body["data"][0]["mayhem"]["attestation_tiers"]["T1"], 1);
    assert_eq!(body["data"][0]["mayhem"]["attestation_tiers"]["T2"], 2);
    assert!(body["data"][0]["mayhem"]["attestation_tier_labels"]["T2"]
        .as_str()
        .expect("tier 2 label")
        .contains("TPM EK / Apple App Attest / NVIDIA GB10"));
}

#[tokio::test]
async fn embeddings_endpoint_uses_routed_engine_and_records_receipt() {
    let state = GatewayState::from_models(vec![routed_embedding_test_model()])
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
async fn embeddings_endpoint_supports_base64_float32_encoding() {
    let state = GatewayState::from_models(vec![routed_embedding_test_model()])
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
        .contains("does not support embeddings"));
}

#[tokio::test]
async fn image_generation_endpoint_uses_routed_engine_and_records_receipt() {
    let state = GatewayState::from_models(vec![routed_image_generation_test_model()])
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

    assert_eq!(status, StatusCode::OK);
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
async fn image_generation_scales_step_usage_by_resolution_and_validates_size() {
    let state = GatewayState::from_models(vec![routed_image_generation_test_model()])
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

    assert_eq!(status, StatusCode::OK);
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
async fn image_generation_endpoint_real_sd_cli_records_receipt_when_enabled() {
    if std::env::var_os("MAYHEM_RUN_STABLE_DIFFUSION_CPP_REAL").is_none() {
        return;
    }
    let state = GatewayState::from_models(vec![routed_image_generation_test_model()])
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
    let state = GatewayState::from_models(vec![routed_audio_speech_test_model()])
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
async fn audio_transcription_endpoint_uses_routed_engine_and_records_receipt() {
    let state = GatewayState::from_models(vec![routed_audio_transcription_test_model()])
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
    let state = GatewayState::from_models(vec![routed_test_model()])
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
    let state = GatewayState::from_models(vec![routed_test_model()])
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
    let seen_content = Arc::new(Mutex::new(Vec::new()));
    let state = GatewayState::from_models(vec![model]).with_session_backend(Arc::new(
        VisionInspectBackend {
            seen_content: seen_content.clone(),
        },
    ));
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

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["choices"][0]["message"]["content"], "vision ok");
    assert_eq!(
        seen_content.lock().expect("seen content")[0][1]["image_url"]["url"],
        "data:image/png;base64,aW1hZ2U="
    );
}

#[tokio::test]
async fn chat_completion_exposes_direct_session_artifact_summary() {
    let state = GatewayState::from_models(vec![routed_test_model()])
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
    let state = GatewayState::from_models(vec![routed_test_model()])
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
    let state = GatewayState::from_models(vec![routed_test_model()])
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
async fn automatic_context_needle_probe_marks_long_context_truncation_slashable() {
    let mut model = routed_test_model();
    model.mayhem.caps.ctx = 131_072;
    model.mayhem.route_candidates[0].caps = json!({ "ctx": 131_072 });
    let state = GatewayState::from_models(vec![model])
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
    let state = GatewayState::from_models(vec![model])
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
    let state = GatewayState::from_models(vec![routed_test_model_with_providers(&[
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
    let state = GatewayState::from_models(vec![routed_test_model_with_providers(&providers)])
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
    let state = GatewayState::from_models(vec![routed_test_model_with_providers(&[
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

    assert_eq!(status, StatusCode::OK);
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
    let state = GatewayState::from_models(vec![routed_test_model_with_providers(&[
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
    let state = GatewayState::from_models(vec![routed_test_model_with_providers(&[
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
    model.mayhem.route_candidates[1].att_tier = 3;
    model.mayhem.attestation_tiers = BTreeMap::from([("T1".to_owned(), 1), ("T3".to_owned(), 1)]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = GatewayState::from_models(vec![model]).with_session_backend(Arc::new(
        RetryThenDirectSessionBackend {
            retry_provider: "ff".repeat(32),
            calls: calls.clone(),
        },
    ));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Use Tier 3 only." }]
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
    let tier2_provider = "66".repeat(32);
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
    let state = GatewayState::from_models(vec![model]).with_session_backend(Arc::new(
        RetryThenDirectSessionBackend {
            retry_provider: "ff".repeat(32),
            calls: calls.clone(),
        },
    ));
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
    let state = GatewayState::from_models(vec![routed_test_model_with_providers(&[
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
    let state = GatewayState::from_models(vec![model]).with_session_backend(Arc::new(
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
    let state = GatewayState::from_models(vec![routed_test_model_with_providers(&[
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
    for candidate in &mut model.mayhem.route_candidates {
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
    model.mayhem.adapter.request_shape_family = "openai_images".to_owned();
    model.mayhem.adapter.response_normalization = "openai_images".to_owned();
    for candidate in &mut model.mayhem.route_candidates {
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
    model.mayhem.adapter.request_shape_family = "openai_audio_speech".to_owned();
    model.mayhem.adapter.response_normalization = "openai_audio_speech".to_owned();
    for candidate in &mut model.mayhem.route_candidates {
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
    model.mayhem.adapter.request_shape_family = "openai_audio_transcriptions".to_owned();
    model.mayhem.adapter.response_normalization = "openai_audio_transcriptions".to_owned();
    for candidate in &mut model.mayhem.route_candidates {
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
    ReceiptUsage::from_units([(
        USAGE_AUDIO_SECOND,
        wav_duration_seconds_ceil_for_test(&request.audio).unwrap(),
    )])
}

fn image_steps_for_test(request: &ImageGenerationRequest) -> u64 {
    request.steps.unwrap_or(1).clamp(1, 150)
}

fn image_cfg_scale_for_test(request: &ImageGenerationRequest) -> f32 {
    request.cfg_scale.unwrap_or(1.0).clamp(0.0, 50.0)
}

fn image_size_for_test(request: &ImageGenerationRequest) -> (u32, u32) {
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
    stable_value_hash_for_test(&json!({
        "kind": "image_generation",
        "prompt": &request.prompt,
        "n": request.n.unwrap_or(1).clamp(1, 4),
        "size": request.size.as_deref().unwrap_or("512x512"),
        "steps": image_steps_for_test(request),
        "cfg_scale": image_cfg_scale_for_test(request),
        "response_format": request.response_format.as_deref().unwrap_or("b64_json"),
        "seed": request.seed,
    }))
}

fn audio_speech_prompt_hash_for_test(request: &AudioSpeechRequest) -> String {
    stable_value_hash_for_test(&json!({
        "kind": "audio_speech",
        "input": &request.input,
        "response_format": request.response_format.as_deref().unwrap_or("wav"),
        "voice": request.voice.as_deref(),
        "speed": request.speed,
    }))
}

fn audio_transcription_prompt_hash_for_test(request: &AudioTranscriptionRequest) -> String {
    stable_value_hash_for_test(&json!({
        "kind": "audio_transcription",
        "audio": {
            "encoding": "hex",
            "content_type": request.content_type.as_deref().unwrap_or("audio/wav"),
            "filename": request.filename.as_deref().unwrap_or("audio.wav"),
            "data": hex::encode(&request.audio),
        },
        "audio_seconds": wav_duration_seconds_ceil_for_test(&request.audio).unwrap(),
        "response_format": request.response_format.as_deref().unwrap_or("json"),
        "language": request.language.as_deref(),
        "prompt": request.prompt.as_deref(),
    }))
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
    bytes.resize(44 + data_len as usize, 0);
    bytes
}

fn wav_duration_seconds_ceil_for_test(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let data_len = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]) as u64;
    let byte_rate = u32::from_le_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]) as u64;
    Some(data_len.div_ceil(byte_rate).max(1))
}

fn stable_value_hash_for_test(value: &Value) -> String {
    blake3::hash(stable_json_value_for_test(value).to_string().as_bytes())
        .to_hex()
        .to_string()
}

fn stable_json_value_for_test(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(stable_json_value_for_test).collect()),
        Value::Object(map) => {
            let mut stable = serde_json::Map::new();
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (key, value) in entries {
                if value.is_null() {
                    continue;
                }
                stable.insert(key.clone(), stable_json_value_for_test(value));
            }
            Value::Object(stable)
        }
        other => other.clone(),
    }
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
                match_min_bps: 9_000,
                verification_method: "token_fingerprint".to_owned(),
                verification_tolerance_bps: None,
                prompts: vec![GatewayCanaryPrompt {
                    id: "fixed-probe".to_owned(),
                    messages: vec![ChatMessage {
                        role: "user".to_owned(),
                        content: json!("fixed canary prompt"),
                        name: None,
                        extra: BTreeMap::new(),
                    }],
                    tools: None,
                    max_tokens: 8,
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
                default_fingerprint: None,
                default_token_prefixes: None,
                default_perceptual_hashes: None,
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
            failover: mayhem_gateway::openai::GatewayFailoverPolicyConfig::default(),
            source: "contract".to_owned(),
            kyb_identities: Vec::new(),
            route_candidates: providers
                .iter()
                .enumerate()
                .map(|(idx, provider)| routed_test_candidate(provider, idx))
                .collect(),
        },
    }
}

fn routed_test_candidate(provider: &str, idx: usize) -> GatewayRouteCandidate {
    let identity = routed_test_identity();
    let room_id = format!("{:02x}", idx + 160).repeat(16);
    GatewayRouteCandidate {
        provider: provider.to_owned(),
        accepted_rails: vec!["fiat".to_owned(), "tap".to_owned(), "tnk".to_owned()],
        enclave_id: catalog_enclave_id(&identity),
        room_id,
        price_ver: 7,
        price_ref_au: None,
        min_ask_au: 0,
        att_tier: 1,
        quant: "int4".to_owned(),
        admin_pubkey: identity.admin_pubkey,
        artifact_root: identity.artifact_root,
        artifact_sidecar_roots: BTreeMap::new(),
        manifest_hash: identity.manifest_hash,
        binary_hash: identity.binary_hash,
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
        transport_peer: None,
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
        },
        att: HeartbeatAttestation {
            epoch: 3,
            head: candidate.binary_hash.clone(),
        },
        ts: 1_782_950_400,
        nonce: format!("network-dashboard-test-{}", candidate.room_id),
        sig: "11".repeat(64),
    }
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
async fn chat_completion_streams_normalized_tool_call_delta() {
    let mut model = routed_test_model();
    model.mayhem.adapter = ShapeAdapterInfo {
        tool_call_strategy: "openai_tool_calls".to_owned(),
        ..ShapeAdapterInfo::default()
    };
    let state = GatewayState::from_models(vec![model])
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
    assert!(body.contains("\"id\":\"call-normalized\""));
    assert!(body.contains("\"type\":\"function\""));
    assert!(body.contains("\"name\":\"write\""));
    assert!(body.contains("\"arguments\":\"{\\\"filePath\\\":\\\"ok.txt\\\"}\""));
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
    let state = GatewayState::from_models(vec![routed_test_model()])
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
        .contains("Mayhem completion"));
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

    let (status, headers, bytes) =
        raw_request(app.clone(), Method::GET, dashboard_path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("set-cookie").is_some());
    let body = String::from_utf8(bytes).expect("dashboard html");
    assert!(body.contains("Runs entirely on this machine. No external network calls."));
    assert_no_external_urls(&body);

    let cookie = headers
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .expect("dashboard session cookie")
        .to_owned();
    let (status, _, _) = raw_request_with_headers(
        app.clone(),
        Method::GET,
        "/mayhem/dashboard",
        None,
        &[("cookie", &cookie)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let session_path = format!("/mayhem/dashboard/session{query}");
    let (status, body) = json_request(app, Method::GET, &session_path, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    let expires = body["expires_in_seconds"].as_u64().expect("expiry seconds");
    assert!(expires > 0);
    assert!(expires <= 900);
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
        raw_request(app.clone(), Method::GET, dashboard_path, None).await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("dashboard html");
    for expected in [
        "@font-face",
        "/mayhem/dashboard/assets/exo-latin.woff2",
        "--surface-card",
        "class=\"wordmark\"",
        "class=\"status-dot\"",
        "class=\"copy-chip\"",
        "class=\"count-chip\"",
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
    let (status, headers, bytes) = raw_request_with_headers(
        app,
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
async fn dashboard_price_chart_follow_list_persists_in_cookie() {
    let state = GatewayState::from_models(vec![routed_test_model()]);
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let pinned_path = format!("{dashboard_path}&pin=mayhem%2Frouted-test");
    let app = openai_router(state);

    let (status, headers, bytes) = raw_request(app.clone(), Method::GET, &pinned_path, None).await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("dashboard html");
    assert!(body.contains("price-chart-svg"));
    assert!(body.contains("Following"));
    let cookies = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(
        cookies
            .iter()
            .any(|cookie| cookie.contains("mayhem_dashboard_user_pins=mayhem%2Frouted-test")),
        "pin cookie missing from {cookies:?}"
    );
    let cookie_header = cookies
        .iter()
        .filter_map(|cookie| cookie.split(';').next())
        .collect::<Vec<_>>()
        .join("; ");

    let (status, _, bytes) = raw_request_with_headers(
        app,
        Method::GET,
        dashboard_path,
        None,
        &[("cookie", &cookie_header)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("dashboard html");
    assert!(body.contains("Following"));
    assert_no_external_urls(&body);
}

#[tokio::test]
async fn user_dashboard_renders_live_gateway_data() {
    let state = GatewayState::from_embedded_catalog()
        .with_dev_session_shim()
        .with_receipt_balance_au(1_000_000_000_000_000_000);
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let app = openai_router(state);
    let model = first_model_id().await;
    let request = json!({
        "model": model,
        "messages": [{"role": "user", "content": "hello"}]
    });
    let (status, _) =
        json_request(app.clone(), Method::POST, "/v1/chat/completions", request).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, bytes) = raw_request_with_headers(
        app,
        Method::GET,
        dashboard_path,
        None,
        &[("host", "127.0.0.1:11435")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("dashboard html");
    assert!(body.contains("User dashboard"));
    assert!(body.contains("$1.00"));
    assert!(body.contains("TAP rate not loaded"));
    assert!(body.contains("http://127.0.0.1:11435/v1"));
    assert!(body.contains("OPENAI_BASE_URL=http://127.0.0.1:11435/v1"));
    assert!(body.contains("Sessions"));
    assert!(body.contains("Models"));
    assert!(body.contains("Spend"));
    assert!(body.contains("price-chart-svg"));
    assert!(body.contains("Ctx bucket"));
    assert!(body.contains("Timeframe"));
    assert!(body.contains("Only Tier 3 keeps prompts private"));
    assert!(body.contains("Tier 4 can still read prompts"));
    assert!(body.contains("not a privacy ladder"));
    assert!(body.contains(&model));
    assert!(!body.contains("1,240.00 TAP"));
    assert_no_external_urls(&body);
}

#[tokio::test]
async fn user_dashboard_shows_model_worker_route_counts() {
    let providers = vec!["41".repeat(32), "42".repeat(32), "43".repeat(32)];
    let mut model = routed_test_model_with_providers(&providers);
    model.mayhem.providers_online = 2;
    let state = GatewayState::from_models(vec![model]);
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let app = openai_router(state);

    let (status, _, bytes) = raw_request_with_headers(
        app,
        Method::GET,
        dashboard_path,
        None,
        &[("host", "127.0.0.1:11435")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("user dashboard html");
    assert!(body.contains("mayhem/routed-test"));
    assert!(body.contains("3 worker routes"));
}

#[tokio::test]
async fn provider_dashboard_renders_routes_receipts_and_earnings() {
    let provider = "55".repeat(32);
    let state = GatewayState::from_models(vec![routed_test_model_with_providers(
        std::slice::from_ref(&provider),
    )])
    .with_provider_earnings(vec![json!({
        "provider": provider,
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
    .with_session_backend(Arc::new(TestDirectSessionBackend));
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let query = dashboard_path
        .strip_prefix("/mayhem/dashboard?")
        .expect("dashboard token query");
    let provider_path = format!("/mayhem/dashboard/provider?{query}&provider={}", provider);
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

    let (status, _, bytes) = raw_request_with_headers(
        app,
        Method::GET,
        &provider_path,
        None,
        &[("host", "127.0.0.1:11435")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("provider dashboard html");
    assert!(body.contains("Provider dashboard"));
    assert!(body.contains("matches mayhem earnings"));
    assert!(body.contains("$2.50"));
    assert!(body.contains("$1.75"));
    assert!(body.contains("$0.25"));
    assert!(body.contains("mayhem/routed-test"));
    assert!(body.contains("Enclaves"));
    assert!(body.contains("Live sessions"));
    assert!(body.contains("Earnings"));
    assert!(body.contains("Reputation / Holdback"));
    assert!(body.contains("Hardware / Health"));
    assert!(body.contains("Workers"));
    assert!(body.contains("Markets"));
    assert!(body.contains("price-chart-svg"));
    assert!(body.contains("Ctx bucket"));
    assert!(body.contains("mayhem earnings --provider"));
    assert!(body.contains("mayhem withdraw --claim-proof"));
    assert!(!body.contains("ledger earnings not loaded"));
    assert_no_external_urls(&body);
}

#[tokio::test]
async fn provider_dashboard_counts_multi_enclave_workers_and_markets() {
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
    let state = GatewayState::from_models(vec![chat, embedding]);
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let query = dashboard_path
        .strip_prefix("/mayhem/dashboard?")
        .expect("dashboard token query");
    let provider_path = format!("/mayhem/dashboard/provider?{query}&provider={provider}");
    let app = openai_router(state);

    let (status, _, bytes) = raw_request_with_headers(
        app,
        Method::GET,
        &provider_path,
        None,
        &[("host", "127.0.0.1:11435")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("provider dashboard html");
    assert!(body.contains(r#"<span class="label">Workers</span><p class="value mono">2</p>"#));
    assert!(body.contains(r#"<span class="label">Markets</span><p class="value mono">2</p>"#));
    assert!(body.contains("mayhem/chat-small"));
    assert!(body.contains("mayhem/embed-small"));
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
            "updated_at_ms": 1_782_950_400_000_u64
        }))
        .expect("progress json"),
    )
    .expect("write progress");
    let state = GatewayState::from_models(vec![model])
        .with_provider_load_progress_dir(progress_dir.path().to_path_buf());
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let query = dashboard_path
        .strip_prefix("/mayhem/dashboard?")
        .expect("dashboard token query");
    let provider_path = format!("/mayhem/dashboard/provider?{query}&provider={provider}");
    let app = openai_router(state);

    let (status, _, bytes) = raw_request_with_headers(
        app,
        Method::GET,
        &provider_path,
        None,
        &[("host", "127.0.0.1:11435")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("provider dashboard html");
    assert!(body.contains("Loading"));
    assert!(body.contains("verify 42%"));
    assert!(body.contains("style=\"--w:42%\""));
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
    let state = GatewayState::from_models(Vec::new())
        .with_provider_load_progress_dir(progress_dir.path().to_path_buf());
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let query = dashboard_path
        .strip_prefix("/mayhem/dashboard?")
        .expect("dashboard token query");
    let provider_path = format!("/mayhem/dashboard/provider?{query}&provider={provider}");
    let app = openai_router(state);

    let (status, _, bytes) = raw_request_with_headers(
        app,
        Method::GET,
        &provider_path,
        None,
        &[("host", "127.0.0.1:11435")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("provider dashboard html");
    assert!(body.contains("mayhem/loading-test"));
    assert!(body.contains("pending"));
    assert!(body.contains("download 70%"));
    assert!(body.contains("Loading"));
    assert!(!body.contains("No provider routes loaded"));
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
    let state = GatewayState::from_models(Vec::new())
        .with_provider_load_progress_dir(progress_dir.path().to_path_buf());
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let query = dashboard_path
        .strip_prefix("/mayhem/dashboard?")
        .expect("dashboard token query");
    let provider_path = format!("/mayhem/dashboard/provider?{query}&provider={provider}");
    let app = openai_router(state);

    let (status, _, bytes) = raw_request_with_headers(
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
    assert!(body.contains("No provider routes loaded"));
    assert_no_external_urls(&body);
}

#[tokio::test]
async fn network_dashboard_renders_live_catalog_and_provider_state() {
    let provider_a = "88".repeat(32);
    let provider_b = "99".repeat(32);
    let mut model = routed_test_model_with_providers(&[provider_a.clone(), provider_b.clone()]);
    model.mayhem.providers_online = 2;
    model.mayhem.rooms = 2;
    model.mayhem.price_ref_au.derivation = Some(json!({
        "type": "price_derivation",
        "schema_version": 1,
        "epoch": 12,
        "enclave_id": model.mayhem.route_candidates[0].enclave_id,
        "model_id": model.id,
        "denom": "au_usd",
        "price_ver": 7,
        "price_source": "market_float",
        "usage": {
            "usage_root": "ab".repeat(32),
            "active_demand_au": "2500000000000000000",
            "session_count": 5u64
        },
        "controller": {
            "source": "market_float",
            "active_supply": 2u64,
            "utilization_bps": 8_750u64,
            "ema_utilization_bps": 8_320u64,
            "multiplier_bps": 10_550u64,
            "frozen": false,
            "frozen_reason": null,
            "constants": {
                "target_utilization_bps": 8_500u64,
                "max_step_bps": 1_000u64
            }
        },
        "seed_price": { "ver": 1, "rate_map": [], "per_req_au": "0", "min_session_au": "0" },
        "result_price": { "ver": 7, "rate_map": [], "per_req_au": "0", "min_session_au": "0" },
        "derivation_hash": "cd".repeat(32),
        "price_root": "ef".repeat(32)
    }));
    model.mayhem.route_candidates[0].accepted_rails = vec!["fiat".to_owned(), "tap".to_owned()];
    model.mayhem.route_candidates[0].caps = json!({
        "engine": "vllm",
        "tools": true,
        "json": true,
        "ctx": 16384
    });
    model.mayhem.route_candidates[0].local_run = Some(GatewayLocalRunBadge {
        marker: "◐".to_owned(),
        status: "runs_reduced".to_owned(),
        label: "runs reduced".to_owned(),
        reason: "ctx auto-fallback matched provider load gate".to_owned(),
        requested_ctx: 65_536,
        served_ctx: 16_384,
        estimated_tok_s: Some("42.5".to_owned()),
        memory_required_human: "914.00 MiB".to_owned(),
        memory_budget_human: "1.00 GiB".to_owned(),
        download_human: "2.52 GiB".to_owned(),
        eta: "measured after the first download chunk".to_owned(),
    });
    model.mayhem.route_candidates[1].accepted_rails = vec!["tnk".to_owned()];
    model.mayhem.route_candidates[1].att_tier = 2;
    model.mayhem.route_candidates[1].reputation_bps = 8_750;
    model.mayhem.route_candidates[1].caps = json!({
        "engine": "llama.cpp",
        "tools": false,
        "json": true,
        "ctx": 4096
    });

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
    let app = openai_router(state);

    let (status, _, bytes) = raw_request_with_headers(
        app,
        Method::GET,
        &network_path,
        None,
        &[("host", "127.0.0.1:11435")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("network dashboard html");
    assert!(body.contains("Network explorer"));
    assert!(body.contains("mayhem/routed-test"));
    assert!(body.contains("mayhem/unavailable-test"));
    assert!(body.contains("price-chart-svg"));
    assert!(body.contains("Timeframe"));
    assert!(body.contains("vllm"));
    assert!(body.contains("llama.cpp"));
    assert!(body.contains("fiat, tap"));
    assert!(body.contains("tnk"));
    assert!(body.contains("Online"));
    assert!(body.contains("Joined"));
    assert!(body.contains("Unavailable"));
    assert!(body.contains("sat 42%"));
    assert!(body.contains("slots 2/4"));
    assert!(body.contains("76.5 tok/s"));
    assert!(body.contains("local ◐ runs reduced"));
    assert!(body.contains("ctx 16384/65536"));
    assert!(body.contains("42.5 tok/s est"));
    assert!(body.contains("download 2.52 GiB"));
    assert!(body.contains("ETA measured after the first download chunk"));
    assert!(body.contains("T2"));
    assert!(body.contains("rep 87.50%"));
    assert!(body.contains("price = f(seed v1, U 87.50%, demand $2.50, 5 sessions, supply 2)"));
    assert!(body.contains("epoch 12"));
    assert!(body.contains("root efefefefe..."));
    assert!(body.contains("leaf cdcdcdcdc..."));
    assert!(body.contains("no canonical provider route"));
    assert!(body.contains("ctx 16384"));
    assert!(body.contains("ctx 4096"));
    assert!(!body.contains(">vision<"));
    assert!(!body.contains("1,240"));
    assert!(!body.contains("42 tok/s"));
    assert!(!body.contains("mx/s/session"));
    assert_no_external_urls(&body);
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
