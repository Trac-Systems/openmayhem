use super::{
    validate_load_config, verify_artifact, verify_safetensors_header_as, wav_duration_seconds_ceil,
    ArtifactChunk, ArtifactFormat, ArtifactSink, CancellationToken, EngineBackend, EngineError,
    GenerateOutput, GenerateRequest, LoadConfig, LoadedModelInfo, Result, SpeechOutput,
    SpeechReferenceAudio, SpeechRequest, SpeechValidation, TokenSink, Tokenization,
};
use base64::Engine as _;
use mayhem_enclave::{
    SandboxConfig, SandboxedChild, SandboxedChildStderr, SandboxedChildStdin, SandboxedChildStdout,
    SandboxedCommand, SandboxedStderr,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const WORKER: &str = include_str!("chatterbox_worker.py");
const WORKER_STDIN_BOOTSTRAP: &str = concat!(
    "import base64,sys;",
    "exec(compile(base64.b64decode(sys.stdin.buffer.readline()),",
    "'<mayhem-chatterbox-worker>','exec'))"
);
const PYTHON_ENV: &str = "MAYHEM_CHATTERBOX_PYTHON";
const DEVICE_ENV: &str = "MAYHEM_CHATTERBOX_DEVICE";
const RUNTIME_PACKAGE: &str = "chatterbox-tts";
const RUNTIME_VERSION: &str = "0.1.7";
const MODEL_FAMILY: &str = "original_english";
const MAX_INPUT_CHARACTERS: u32 = 16 * 1024;
const MAX_INPUT_BYTES: usize = MAX_INPUT_CHARACTERS as usize * 4;
const T3_MAX_TEXT_TOKENS: u32 = 2_048;
const MAX_REFERENCE_AUDIO_BYTES: usize = 16 * 1024 * 1024;
const MAX_REFERENCE_AUDIO_SECONDS: u64 = 10;
const T3_REFERENCE_SECONDS: u32 = 6;
const S3GEN_REFERENCE_SECONDS: u32 = 10;
const MAX_OUTPUT_AUDIO_BYTES: usize = 64 * 1024 * 1024;
const MAX_OUTPUT_AUDIO_SECONDS: f64 = 120.0;
const ARTIFACT_CHUNK_BYTES: usize = 256 * 1024;
const WORKER_STDERR_TAIL_BYTES: usize = 64 * 1024;
const MAX_WORKER_REQUEST_LINE_BYTES: usize = 24 * 1024 * 1024;
const MAX_WORKER_RESPONSE_LINE_BYTES: usize = 96 * 1024 * 1024;
const CANONICAL_MODEL_FILES: &[ModelFileSpec] = &[
    ModelFileSpec {
        name: "ve.safetensors",
        size: 5_695_784,
        sha256: "f0921cab452fa278bc25cd23ffd59d36f816d7dc5181dd1bef9751a7fb61f63c",
    },
    ModelFileSpec {
        name: "t3_cfg.safetensors",
        size: 2_129_653_744,
        sha256: "914cb1696f47527fe8852ca8f1fe1fa63cb34f76f9c715e84e067b744dd0da81",
    },
    ModelFileSpec {
        name: "s3gen.safetensors",
        size: 1_056_484_620,
        sha256: "2b78103c654207393955e4900aac14a12de8ef25f4b09424f1ef91941f161d4e",
    },
    ModelFileSpec {
        name: "tokenizer.json",
        size: 25_470,
        sha256: "d71e3a44eabb1784df9a68e9f95b251ecbf1a7af6a9f50835856b2ca9d8c14a5",
    },
    ModelFileSpec {
        name: "conds.pt",
        size: 107_374,
        sha256: "6552d70568833628ba019c6b03459e77fe71ca197d5c560cef9411bee9d87f4e",
    },
];
const EXPECTED_HANDLED_CONTROLS: &[&str] = &[
    "cfg_weight",
    "exaggeration",
    "input",
    "min_p",
    "reference_audio",
    "repetition_penalty",
    "seed",
    "temperature",
    "top_p",
];
const SEMANTICALLY_VALIDATED_SPEECH_CONTROLS: &[&str] = &[
    "cfg_weight",
    "exaggeration",
    "input",
    "min_p",
    "reference_audio",
    "repetition_penalty",
    "response_format",
    "seed",
    "speed",
    "temperature",
    "top_p",
    "voice",
];

pub const CHATTERBOX_SOURCE_COMMIT: &str = "59bc590b3cad826e5d5987745bf6844627a21ad5";
pub const CHATTERBOX_MODEL_REVISION: &str = "5bb1f6ee58e50c3b8d408bc82a6d3740c2db6e18";
pub const CHATTERBOX_PERTH_COMMIT: &str = "ce86c49d029f42272c1902eccb675556b9ed2330";
const CHATTERBOX_TTS_SOURCE_SHA256: &str =
    "7896787bc17e20eafcd1dce7b8a4a6ea3a6478baab771c60d63e9e81f5564195";

#[derive(Clone, Copy, Debug)]
struct ModelFileSpec {
    name: &'static str,
    size: u64,
    sha256: &'static str,
}

pub type ChatterboxReferenceAudio = SpeechReferenceAudio;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatterboxSpeechRequest {
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_audio: Option<ChatterboxReferenceAudio>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exaggeration: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cfg_weight: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repetition_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
}

impl ChatterboxSpeechRequest {
    #[must_use]
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
            reference_audio: None,
            response_format: Some("wav".to_owned()),
            exaggeration: None,
            cfg_weight: None,
            temperature: None,
            seed: None,
            repetition_penalty: None,
            min_p: None,
            top_p: None,
        }
    }

    #[must_use]
    pub fn with_reference_audio(mut self, reference_audio: ChatterboxReferenceAudio) -> Self {
        self.reference_audio = Some(reference_audio);
        self
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatterboxExecutionConfig {
    pub api_version: u32,
    pub model_family: String,
    pub runtime_package: String,
    pub runtime_version: String,
    pub source_commit: String,
    pub model_revision: String,
    pub runtime_source_sha256: String,
    pub perth_commit: String,
    pub device: String,
    pub sample_rate: u32,
    pub input_character_limit: u32,
    pub input_byte_limit: u32,
    pub max_text_tokens: u32,
    pub reference_audio_limit_seconds: u32,
    pub t3_reference_seconds: u32,
    pub s3gen_reference_seconds: u32,
    pub supports_voice_cloning: bool,
    pub seed_semantics: String,
}

pub struct ChatterboxBackend {
    python: PathBuf,
    expected_device: Option<String>,
    worker: Option<ChatterboxWorker>,
    loaded: Option<LoadedModelInfo>,
    config: Option<LoadConfig>,
    model_root: Option<PathBuf>,
    worker_cache: Option<PathBuf>,
    execution_config: Option<ChatterboxExecutionConfig>,
    next_id: u64,
    next_artifact_id: u64,
}

impl ChatterboxBackend {
    pub fn new() -> Result<Self> {
        let python = env::var_os(PYTHON_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("python3"));
        Self::with_python(python)
    }

    pub fn with_python(python: impl Into<PathBuf>) -> Result<Self> {
        let expected_device = env::var(DEVICE_ENV)
            .ok()
            .map(|device| normalize_expected_device(&device))
            .transpose()?
            .flatten();
        Self::with_python_and_expected_device(python, expected_device)
    }

    pub fn with_python_for_device(
        python: impl Into<PathBuf>,
        expected_device: &str,
    ) -> Result<Self> {
        let expected_device = normalize_expected_device(expected_device)?.ok_or_else(|| {
            EngineError::InvalidConfig(
                "Chatterbox expected device must be cpu, cuda, or mps".to_owned(),
            )
        })?;
        Self::with_python_and_expected_device(python, Some(expected_device))
    }

    fn with_python_and_expected_device(
        python: impl Into<PathBuf>,
        expected_device: Option<String>,
    ) -> Result<Self> {
        Ok(Self {
            python: python.into(),
            expected_device,
            worker: None,
            loaded: None,
            config: None,
            model_root: None,
            worker_cache: None,
            execution_config: None,
            next_id: 1,
            next_artifact_id: 1,
        })
    }

    #[must_use]
    pub fn execution_config(&self) -> Option<&ChatterboxExecutionConfig> {
        self.execution_config.as_ref()
    }

    pub fn synthesize_chatterbox(
        &mut self,
        request: ChatterboxSpeechRequest,
        artifact_sink: &mut dyn ArtifactSink,
        cancellation: &CancellationToken,
    ) -> Result<SpeechOutput> {
        if cancellation.is_cancelled() {
            self.stop_worker();
            return Err(EngineError::Cancelled);
        }
        self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
        let input_character_limit = self
            .execution_config
            .as_ref()
            .map(|config| config.input_character_limit)
            .ok_or(EngineError::NotLoaded)?;
        let normalized = NormalizedSpeechRequest::from_request(request, input_character_limit)?;
        let expects_reference = normalized.reference_audio.is_some();
        let expects_seed = normalized.seed.is_some_and(|seed| seed != 0);
        let response: WorkerSynthesisResult = self.call(
            "synthesize",
            normalized.worker_payload(),
            Some(cancellation),
        )?;
        self.emit_synthesis_result(
            response,
            expects_reference,
            expects_seed,
            artifact_sink,
            cancellation,
        )
    }

    fn emit_synthesis_result(
        &mut self,
        response: WorkerSynthesisResult,
        expects_reference: bool,
        expects_seed: bool,
        artifact_sink: &mut dyn ArtifactSink,
        cancellation: &CancellationToken,
    ) -> Result<SpeechOutput> {
        if response.content_type != "audio/wav" {
            self.stop_worker();
            return Err(EngineError::Chatterbox(format!(
                "worker returned content type {}, expected audio/wav",
                response.content_type
            )));
        }
        if response.reference_audio_used != expects_reference {
            self.stop_worker();
            return Err(EngineError::Chatterbox(
                "worker reported the wrong reference-audio state".to_owned(),
            ));
        }
        if response.seed_applied != expects_seed {
            self.stop_worker();
            return Err(EngineError::Chatterbox(
                "worker reported the wrong seed state".to_owned(),
            ));
        }
        let expected_controls = EXPECTED_HANDLED_CONTROLS
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        if response.handled_controls != expected_controls {
            self.stop_worker();
            return Err(EngineError::Chatterbox(
                "worker did not confirm the exact Chatterbox control surface".to_owned(),
            ));
        }
        let expected_sample_rate = self
            .execution_config
            .as_ref()
            .map(|config| config.sample_rate)
            .ok_or(EngineError::NotLoaded)?;
        if response.sample_rate != expected_sample_rate || response.sample_count == 0 {
            self.stop_worker();
            return Err(EngineError::Chatterbox(
                "worker returned invalid audio sample metadata".to_owned(),
            ));
        }
        if !response.duration_seconds.is_finite()
            || response.duration_seconds <= 0.0
            || response.duration_seconds > MAX_OUTPUT_AUDIO_SECONDS
        {
            self.stop_worker();
            return Err(EngineError::Chatterbox(
                "worker returned an out-of-bounds audio duration".to_owned(),
            ));
        }
        let expected_duration =
            response.sample_count as f64 / f64::from(response.sample_rate.max(1));
        if (response.duration_seconds - expected_duration).abs() > 0.001 {
            self.stop_worker();
            return Err(EngineError::Chatterbox(
                "worker audio duration does not match its sample metadata".to_owned(),
            ));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(response.data_base64)
            .map_err(|error| {
                self.stop_worker();
                EngineError::Chatterbox(format!("worker returned invalid base64 audio: {error}"))
            })?;
        if bytes.len() < 44 || bytes.len() > MAX_OUTPUT_AUDIO_BYTES {
            self.stop_worker();
            return Err(EngineError::Chatterbox(
                "worker returned an out-of-bounds audio artifact".to_owned(),
            ));
        }
        let actual_sha256 = format!("{:x}", Sha256::digest(&bytes));
        if actual_sha256 != response.sha256 {
            self.stop_worker();
            return Err(EngineError::Chatterbox(
                "worker audio hash does not match its artifact".to_owned(),
            ));
        }
        let audio_seconds = wav_duration_seconds_ceil(&bytes).ok_or_else(|| {
            self.stop_worker();
            EngineError::Chatterbox("worker returned an invalid WAV artifact".to_owned())
        })?;
        if audio_seconds > MAX_OUTPUT_AUDIO_SECONDS.ceil() as u64 {
            self.stop_worker();
            return Err(EngineError::Chatterbox(
                "worker WAV duration exceeds the output bound".to_owned(),
            ));
        }
        let artifact_id = format!("chatterbox-{}", self.next_artifact_id);
        self.next_artifact_id = self.next_artifact_id.saturating_add(1);
        let chunk_count = bytes.len().div_ceil(ARTIFACT_CHUNK_BYTES);
        for (index, chunk) in bytes.chunks(ARTIFACT_CHUNK_BYTES).enumerate() {
            cancellation.check()?;
            artifact_sink.on_artifact_chunk(ArtifactChunk {
                artifact_id: artifact_id.clone(),
                index: u32::try_from(index).map_err(|_| {
                    EngineError::Chatterbox("audio artifact has too many chunks".to_owned())
                })?,
                content_type: "audio/wav".to_owned(),
                bytes: chunk.to_vec(),
                final_chunk: index.saturating_add(1) == chunk_count,
            })?;
        }
        Ok(SpeechOutput { audio_seconds })
    }

    fn ensure_worker_loaded(&mut self) -> Result<()> {
        if self.worker.is_some() {
            return Ok(());
        }
        let config = self.config.clone().ok_or(EngineError::NotLoaded)?;
        let model_root = self.model_root.clone().ok_or_else(|| {
            EngineError::Chatterbox("Chatterbox model root is not prepared".to_owned())
        })?;
        let worker_cache = self.worker_cache.clone().ok_or_else(|| {
            EngineError::Chatterbox("Chatterbox worker cache is not prepared".to_owned())
        })?;
        purge_worker_inputs(&worker_cache)?;
        self.worker = Some(ChatterboxWorker::spawn(
            &self.python,
            config.memory_limit_bytes,
            &worker_cache,
            &model_root,
            self.expected_device.as_deref(),
        )?);
        let info = match self.load_existing_worker(&model_root, &worker_cache) {
            Ok(info) => info,
            Err(error) => {
                self.stop_worker();
                return Err(error);
            }
        };
        validate_execution_config(&info.execution_config, self.expected_device.as_deref())?;
        self.execution_config = Some(info.execution_config);
        Ok(())
    }

    fn load_existing_worker(
        &mut self,
        model_root: &Path,
        worker_cache: &Path,
    ) -> Result<WorkerLoadInfo> {
        self.call_existing(
            "load",
            json!({
                "cache_root": worker_cache,
                "input_character_limit": self
                    .config
                    .as_ref()
                    .map(|config| config.ctx_size.min(MAX_INPUT_CHARACTERS))
                    .ok_or(EngineError::NotLoaded)?,
                "model_root": model_root,
            }),
            None,
        )
    }

    fn call<T>(
        &mut self,
        operation: &str,
        payload: Value,
        cancellation: Option<&CancellationToken>,
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        self.ensure_worker_loaded()?;
        self.call_existing(operation, payload, cancellation)
    }

    fn call_existing<T>(
        &mut self,
        operation: &str,
        payload: Value,
        cancellation: Option<&CancellationToken>,
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.worker_mut()?.send(id, operation, payload)?;
        loop {
            if cancellation.is_some_and(CancellationToken::is_cancelled) {
                self.stop_worker();
                return Err(EngineError::Cancelled);
            }
            let message = match self.worker_mut()?.read_message(Duration::from_millis(25)) {
                Ok(Some(message)) => message,
                Ok(None) => continue,
                Err(error) => {
                    self.stop_worker();
                    return Err(error);
                }
            };
            if message.id != id {
                self.stop_worker();
                return Err(EngineError::Chatterbox(format!(
                    "worker response id {} did not match request id {id}",
                    message.id
                )));
            }
            if !message.ok {
                return Err(EngineError::Chatterbox(
                    message
                        .error
                        .unwrap_or_else(|| "worker returned an unknown error".to_owned()),
                ));
            }
            return serde_json::from_value(message.result.unwrap_or(Value::Null)).map_err(
                |error| {
                    self.stop_worker();
                    EngineError::Chatterbox(format!(
                        "decoding Chatterbox worker response failed: {error}"
                    ))
                },
            );
        }
    }

    fn worker_mut(&mut self) -> Result<&mut ChatterboxWorker> {
        self.worker
            .as_mut()
            .ok_or_else(|| EngineError::Chatterbox("Chatterbox worker is not running".to_owned()))
    }

    fn stop_worker(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            worker.stop();
        }
        if let Some(cache) = &self.worker_cache {
            let _ = purge_worker_inputs(cache);
        }
    }
}

impl EngineBackend for ChatterboxBackend {
    fn backend_id(&self) -> &'static str {
        "chatterbox"
    }

    fn load(&mut self, config: LoadConfig) -> Result<LoadedModelInfo> {
        validate_load_config(&config)?;
        if config.artifact.format != ArtifactFormat::ChatterboxSafetensors {
            return Err(EngineError::InvalidConfig(format!(
                "Chatterbox requires Chatterbox safetensors artifacts, got {:?}",
                config.artifact.format
            )));
        }
        verify_artifact(&config.artifact)?;
        let model_root = chatterbox_model_root(&config.artifact.path)?;
        validate_model_root(&model_root)?;
        let cache_root = chatterbox_cache_root(config.backend_cache_dir.as_deref());
        let worker_cache = chatterbox_worker_cache(&cache_root, &model_root)?;
        fs::create_dir_all(&worker_cache).map_err(|error| {
            EngineError::Chatterbox(format!(
                "creating Chatterbox worker cache {} failed: {error}",
                worker_cache.display()
            ))
        })?;

        self.stop_worker();
        self.config = Some(config.clone());
        self.model_root = Some(model_root.clone());
        self.worker_cache = Some(worker_cache.clone());
        self.execution_config = None;
        purge_worker_inputs(&worker_cache)?;
        self.worker = Some(ChatterboxWorker::spawn(
            &self.python,
            config.memory_limit_bytes,
            &worker_cache,
            &model_root,
            self.expected_device.as_deref(),
        )?);
        let info = match self.load_existing_worker(&model_root, &worker_cache) {
            Ok(info) => info,
            Err(error) => {
                self.stop_worker();
                return Err(error);
            }
        };
        validate_execution_config(&info.execution_config, self.expected_device.as_deref())?;
        self.execution_config = Some(info.execution_config);
        let loaded = LoadedModelInfo {
            backend: self.backend_id().to_owned(),
            artifact: config.artifact,
            ctx_size: config.ctx_size,
            n_ctx_train: info.n_ctx_train,
            n_vocab: info.n_vocab,
        };
        self.loaded = Some(loaded.clone());
        Ok(loaded)
    }

    fn loaded_backend_evidence(&self) -> Option<Value> {
        self.execution_config
            .as_ref()
            .and_then(|config| serde_json::to_value(config).ok())
    }

    fn component_healthy(&mut self) -> bool {
        match self.worker.as_mut() {
            Some(worker) => matches!(worker.child.try_wait(), Ok(None)),
            None => self.loaded.is_none(),
        }
    }

    fn process_ids(&self) -> Vec<u32> {
        self.worker
            .as_ref()
            .map(|worker| vec![worker.child.id()])
            .unwrap_or_default()
    }

    fn tokenize(&self, text: &str) -> Result<Tokenization> {
        self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
        Ok(Tokenization {
            token_ids: text
                .chars()
                .map(|character| i32::try_from(character as u32).unwrap_or(i32::MAX))
                .collect(),
        })
    }

    fn generate(
        &mut self,
        _request: GenerateRequest,
        _sink: &mut dyn TokenSink,
        _cancellation: &CancellationToken,
    ) -> Result<GenerateOutput> {
        Err(EngineError::InvalidConfig(
            "Chatterbox synthesizes speech; use synthesize_speech".to_owned(),
        ))
    }

    fn synthesize_speech(
        &mut self,
        request: SpeechRequest,
        artifact_sink: &mut dyn ArtifactSink,
        cancellation: &CancellationToken,
    ) -> Result<SpeechOutput> {
        let request = ChatterboxSpeechRequest::try_from(request)?;
        self.synthesize_chatterbox(request, artifact_sink, cancellation)
    }

    fn validate_speech(
        &mut self,
        request: SpeechRequest,
        cancellation: &CancellationToken,
    ) -> Result<Option<SpeechValidation>> {
        cancellation.check()?;
        self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
        let execution_config = self
            .execution_config
            .as_ref()
            .ok_or(EngineError::NotLoaded)?;
        let request = ChatterboxSpeechRequest::try_from(request)?;
        let normalized =
            NormalizedSpeechRequest::from_request(request, execution_config.input_character_limit)?;
        let reference_audio = normalized.reference_audio.as_ref().map(|reference| {
            json!({
                "byte_count": reference.data.len(),
                "content_type": reference.content_type.as_deref().unwrap_or("audio/wav"),
                "duration_seconds": wav_duration_seconds_ceil(&reference.data),
                "sha256": format!("{:x}", Sha256::digest(&reference.data)),
            })
        });
        Ok(Some(SpeechValidation {
            evidence: json!({
                "backend": self.backend_id(),
                "controls": {
                    "cfg_weight": normalized.cfg_weight,
                    "exaggeration": normalized.exaggeration,
                    "min_p": normalized.min_p,
                    "reference_audio": reference_audio,
                    "repetition_penalty": normalized.repetition_penalty,
                    "response_format": "wav",
                    "seed": normalized.seed,
                    "speed": 1.0,
                    "temperature": normalized.temperature,
                    "top_p": normalized.top_p,
                    "voice": if normalized.reference_audio.is_some() {
                        "inline_reference"
                    } else {
                        "builtin"
                    },
                },
                "execution_config": execution_config,
                "input": {
                    "byte_count": normalized.input.len(),
                    "character_count": normalized.input.chars().count(),
                    "sha256": format!("{:x}", Sha256::digest(normalized.input.as_bytes())),
                },
                "model_revision": CHATTERBOX_MODEL_REVISION,
                "perth_commit": CHATTERBOX_PERTH_COMMIT,
                "source_commit": CHATTERBOX_SOURCE_COMMIT,
            }),
            handled_controls: SEMANTICALLY_VALIDATED_SPEECH_CONTROLS
                .iter()
                .map(|control| (*control).to_owned())
                .collect::<BTreeSet<_>>(),
        }))
    }
}

impl Drop for ChatterboxBackend {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

impl TryFrom<SpeechRequest> for ChatterboxSpeechRequest {
    type Error = EngineError;

    fn try_from(request: SpeechRequest) -> Result<Self> {
        if request
            .voice
            .as_deref()
            .is_some_and(|voice| !matches!(voice, "default" | "builtin"))
        {
            return Err(EngineError::InvalidConfig(
                "original Chatterbox has no named voices; use its bounded inline reference-audio request for cloning"
                    .to_owned(),
            ));
        }
        if request
            .speed
            .is_some_and(|speed| !speed.is_finite() || (speed - 1.0).abs() > f64::EPSILON)
        {
            return Err(EngineError::InvalidConfig(
                "original Chatterbox does not expose a speed control".to_owned(),
            ));
        }
        Ok(Self {
            input: request.input,
            reference_audio: request.reference_audio,
            response_format: request.response_format,
            exaggeration: request.exaggeration,
            cfg_weight: request.cfg_weight,
            temperature: request.temperature,
            seed: request.seed,
            repetition_penalty: request.repetition_penalty,
            min_p: request.min_p,
            top_p: request.top_p,
        })
    }
}

#[derive(Debug)]
struct NormalizedSpeechRequest {
    input: String,
    reference_audio: Option<ChatterboxReferenceAudio>,
    exaggeration: f32,
    cfg_weight: f32,
    temperature: f32,
    seed: Option<u32>,
    repetition_penalty: f32,
    min_p: f32,
    top_p: f32,
}

impl TryFrom<ChatterboxSpeechRequest> for NormalizedSpeechRequest {
    type Error = EngineError;

    fn try_from(request: ChatterboxSpeechRequest) -> Result<Self> {
        Self::from_request(request, MAX_INPUT_CHARACTERS)
    }
}

impl NormalizedSpeechRequest {
    fn from_request(request: ChatterboxSpeechRequest, input_character_limit: u32) -> Result<Self> {
        if input_character_limit == 0 || input_character_limit > MAX_INPUT_CHARACTERS {
            return Err(EngineError::InvalidConfig(format!(
                "Chatterbox input_character_limit must be between 1 and {MAX_INPUT_CHARACTERS}"
            )));
        }
        let input = request.input.trim();
        if input.is_empty() {
            return Err(EngineError::InvalidConfig(
                "Chatterbox speech input cannot be empty".to_owned(),
            ));
        }
        if input.len() > MAX_INPUT_BYTES
            || input.chars().count() > usize::try_from(input_character_limit).unwrap_or(usize::MAX)
        {
            return Err(EngineError::InvalidConfig(format!(
                "Chatterbox speech input exceeds its signed limit of {input_character_limit} characters or hard limit of {MAX_INPUT_BYTES} bytes"
            )));
        }
        let response_format = request
            .response_format
            .as_deref()
            .unwrap_or("wav")
            .to_ascii_lowercase();
        if response_format != "wav" {
            return Err(EngineError::InvalidConfig(format!(
                "Chatterbox emits wav audio, got response_format={response_format}"
            )));
        }
        if let Some(reference) = &request.reference_audio {
            validate_reference_audio(reference)?;
        }
        let exaggeration = request.exaggeration.unwrap_or(0.5);
        ensure_f32_range("exaggeration", exaggeration, 0.25, 2.0)?;
        let cfg_weight = request.cfg_weight.unwrap_or(0.5);
        ensure_f32_range("cfg_weight", cfg_weight, 0.0, 1.0)?;
        let temperature = request.temperature.unwrap_or(0.8);
        ensure_f32_range("temperature", temperature, 0.05, 5.0)?;
        let repetition_penalty = request.repetition_penalty.unwrap_or(1.2);
        ensure_f32_range("repetition_penalty", repetition_penalty, 1.0, 2.0)?;
        let min_p = request.min_p.unwrap_or(0.05);
        ensure_f32_range("min_p", min_p, 0.0, 1.0)?;
        let top_p = request.top_p.unwrap_or(1.0);
        ensure_f32_range("top_p", top_p, 0.0, 1.0)?;
        Ok(Self {
            input: input.to_owned(),
            reference_audio: request.reference_audio,
            exaggeration,
            cfg_weight,
            temperature,
            seed: request.seed,
            repetition_penalty,
            min_p,
            top_p,
        })
    }

    fn worker_payload(&self) -> Value {
        let reference_audio = self.reference_audio.as_ref().map(|reference| {
            json!({
                "content_type": reference.content_type.as_deref().unwrap_or("audio/wav"),
                "data_base64": base64::engine::general_purpose::STANDARD.encode(&reference.data),
            })
        });
        json!({
            "cfg_weight": self.cfg_weight,
            "exaggeration": self.exaggeration,
            "input": self.input,
            "min_p": self.min_p,
            "reference_audio": reference_audio,
            "repetition_penalty": self.repetition_penalty,
            "seed": self.seed,
            "temperature": self.temperature,
            "top_p": self.top_p,
        })
    }
}

fn ensure_f32_range(name: &str, value: f32, minimum: f32, maximum: f32) -> Result<()> {
    if !value.is_finite() || !(minimum..=maximum).contains(&value) {
        return Err(EngineError::InvalidConfig(format!(
            "Chatterbox {name} must be finite and between {minimum} and {maximum}"
        )));
    }
    Ok(())
}

fn validate_reference_audio(reference: &ChatterboxReferenceAudio) -> Result<()> {
    let content_type = reference.content_type.as_deref().unwrap_or("audio/wav");
    if !matches!(content_type, "audio/wav" | "audio/x-wav") {
        return Err(EngineError::InvalidConfig(
            "Chatterbox inline reference audio must be audio/wav".to_owned(),
        ));
    }
    if reference.data.len() < 44 || reference.data.len() > MAX_REFERENCE_AUDIO_BYTES {
        return Err(EngineError::InvalidConfig(format!(
            "Chatterbox inline reference audio must be a non-empty WAV of at most {MAX_REFERENCE_AUDIO_BYTES} bytes"
        )));
    }
    let duration = wav_duration_seconds_ceil(&reference.data).ok_or_else(|| {
        EngineError::InvalidConfig(
            "Chatterbox inline reference audio is not a supported PCM WAV".to_owned(),
        )
    })?;
    if duration > MAX_REFERENCE_AUDIO_SECONDS {
        return Err(EngineError::InvalidConfig(format!(
            "Chatterbox inline reference audio must be at most {MAX_REFERENCE_AUDIO_SECONDS} seconds"
        )));
    }
    Ok(())
}

fn chatterbox_model_root(path: &Path) -> Result<PathBuf> {
    if path.is_dir() {
        return Ok(path.to_path_buf());
    }
    if path.file_name().and_then(|name| name.to_str()) != Some("t3_cfg.safetensors") {
        return Err(EngineError::InvalidConfig(format!(
            "original Chatterbox primary artifact must be t3_cfg.safetensors, got {}",
            path.display()
        )));
    }
    path.parent().map(Path::to_path_buf).ok_or_else(|| {
        EngineError::InvalidConfig(format!(
            "Chatterbox artifact {} has no model root",
            path.display()
        ))
    })
}

fn validate_model_root(model_root: &Path) -> Result<()> {
    validate_model_root_with_specs(model_root, CANONICAL_MODEL_FILES)
}

fn validate_model_root_with_specs(model_root: &Path, specs: &[ModelFileSpec]) -> Result<()> {
    let metadata = fs::symlink_metadata(model_root).map_err(|error| {
        EngineError::Chatterbox(format!(
            "reading Chatterbox model root {} failed: {error}",
            model_root.display()
        ))
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(EngineError::Chatterbox(format!(
            "Chatterbox model root {} must be a real directory",
            model_root.display()
        )));
    }
    for spec in specs {
        let path = model_root.join(spec.name);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            EngineError::Chatterbox(format!(
                "Chatterbox model root is missing required file {}: {error}",
                path.display()
            ))
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(EngineError::Chatterbox(format!(
                "Chatterbox model file {} must be a regular non-symlink file",
                path.display()
            )));
        }
        if metadata.len() != spec.size {
            return Err(EngineError::Chatterbox(format!(
                "Chatterbox model file {} has {} bytes, expected {}",
                path.display(),
                metadata.len(),
                spec.size
            )));
        }
        let actual_sha256 = sha256_file(&path)?;
        if actual_sha256 != spec.sha256 {
            return Err(EngineError::Chatterbox(format!(
                "Chatterbox model file {} hash mismatch: expected {}, got {}",
                path.display(),
                spec.sha256,
                actual_sha256
            )));
        }
        if spec.name.ends_with(".safetensors") {
            verify_safetensors_header_as(&path, ArtifactFormat::ChatterboxSafetensors.label())?;
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).map_err(|error| {
        EngineError::Chatterbox(format!(
            "opening Chatterbox model file {} failed: {error}",
            path.display()
        ))
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            EngineError::Chatterbox(format!(
                "reading Chatterbox model file {} failed: {error}",
                path.display()
            ))
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn chatterbox_cache_root(configured: Option<&Path>) -> PathBuf {
    configured
        .map(Path::to_path_buf)
        .or_else(|| {
            env::var_os("MAYHEM_HOME")
                .map(PathBuf::from)
                .map(|home| home.join("cache/chatterbox"))
        })
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".mayhem/cache/chatterbox"))
        })
        .unwrap_or_else(|| env::temp_dir().join("mayhem-chatterbox-cache"))
}

fn chatterbox_worker_cache(cache_root: &Path, model_root: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(model_root).map_err(|error| {
        EngineError::Chatterbox(format!(
            "canonicalizing Chatterbox model root {} failed: {error}",
            model_root.display()
        ))
    })?;
    let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
    Ok(cache_root.join("workers").join(format!("{digest:x}")))
}

fn purge_worker_inputs(cache_root: &Path) -> Result<()> {
    let inputs = cache_root.join("inputs");
    let metadata = match fs::symlink_metadata(&inputs) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(EngineError::Chatterbox(format!(
                "inspecting Chatterbox worker input directory {} failed: {error}",
                inputs.display()
            )));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(EngineError::Chatterbox(format!(
            "Chatterbox worker input path {} must be a non-symlink directory",
            inputs.display()
        )));
    }
    let entries = fs::read_dir(&inputs).map_err(|error| {
        EngineError::Chatterbox(format!(
            "reading Chatterbox worker input directory {} failed: {error}",
            inputs.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            EngineError::Chatterbox(format!(
                "reading an entry in Chatterbox worker input directory {} failed: {error}",
                inputs.display()
            ))
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            EngineError::Chatterbox(format!(
                "inspecting stale Chatterbox worker input {} failed: {error}",
                path.display()
            ))
        })?;
        if metadata.is_file() || metadata.file_type().is_symlink() {
            fs::remove_file(&path).map_err(|error| {
                EngineError::Chatterbox(format!(
                    "removing stale Chatterbox worker input {} failed: {error}",
                    path.display()
                ))
            })?;
        }
    }
    Ok(())
}

fn normalize_expected_device(device: &str) -> Result<Option<String>> {
    let device = device.trim().to_ascii_lowercase();
    match device.as_str() {
        "" | "auto" => Ok(None),
        "cpu" | "cuda" | "mps" => Ok(Some(device)),
        _ => Err(EngineError::InvalidConfig(
            "MAYHEM_CHATTERBOX_DEVICE must be auto, cpu, cuda, or mps".to_owned(),
        )),
    }
}

fn validate_execution_config(
    config: &ChatterboxExecutionConfig,
    expected_device: Option<&str>,
) -> Result<()> {
    if config.api_version != 1
        || config.model_family != MODEL_FAMILY
        || config.runtime_package != RUNTIME_PACKAGE
        || config.runtime_version != RUNTIME_VERSION
        || config.source_commit != CHATTERBOX_SOURCE_COMMIT
        || config.model_revision != CHATTERBOX_MODEL_REVISION
        || config.runtime_source_sha256 != CHATTERBOX_TTS_SOURCE_SHA256
        || config.perth_commit != CHATTERBOX_PERTH_COMMIT
        || config.input_character_limit == 0
        || config.input_character_limit > MAX_INPUT_CHARACTERS
        || config.input_byte_limit != MAX_INPUT_BYTES as u32
        || config.max_text_tokens != T3_MAX_TEXT_TOKENS
        || config.reference_audio_limit_seconds != MAX_REFERENCE_AUDIO_SECONDS as u32
        || config.t3_reference_seconds != T3_REFERENCE_SECONDS
        || config.s3gen_reference_seconds != S3GEN_REFERENCE_SECONDS
        || !config.supports_voice_cloning
        || config.seed_semantics != "official_gradio_global_rng_nonzero"
    {
        return Err(EngineError::Chatterbox(
            "worker reported an unsupported original Chatterbox runtime".to_owned(),
        ));
    }
    if !matches!(config.device.as_str(), "cpu" | "cuda" | "mps") {
        return Err(EngineError::Chatterbox(format!(
            "worker reported unsupported device {}",
            config.device
        )));
    }
    if expected_device.is_some_and(|expected| config.device != expected) {
        return Err(EngineError::Chatterbox(format!(
            "worker reported device {}, but provider admission selected {}",
            config.device,
            expected_device.unwrap_or_default()
        )));
    }
    if !(8_000..=192_000).contains(&config.sample_rate) {
        return Err(EngineError::Chatterbox(
            "worker reported an invalid sample rate".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerLoadInfo {
    n_ctx_train: u32,
    n_vocab: i32,
    execution_config: ChatterboxExecutionConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerMessage {
    id: u64,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerSynthesisResult {
    content_type: String,
    data_base64: String,
    duration_seconds: f64,
    handled_controls: Vec<String>,
    reference_audio_used: bool,
    sample_count: u64,
    sample_rate: u32,
    seed_applied: bool,
    sha256: String,
}

struct ChatterboxWorker {
    child: SandboxedChild,
    stdin: SandboxedChildStdin,
    stdout_rx: Option<Receiver<WorkerRead>>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    stderr_reader: Option<JoinHandle<()>>,
}

impl ChatterboxWorker {
    fn spawn(
        python: &Path,
        memory_limit_bytes: Option<u64>,
        cache_root: &Path,
        model_root: &Path,
        expected_device: Option<&str>,
    ) -> Result<Self> {
        let (python, runtime_roots) = python_runtime_roots(python)?;
        let executable_runtime_roots = runtime_roots.clone();
        let mut read_only_dirs = vec![model_root.to_path_buf()];
        read_only_dirs.extend(runtime_roots);
        let mut sandbox = SandboxConfig::new(read_only_dirs, vec![cache_root.to_path_buf()]);
        sandbox.materialized_read_only_dir(model_root);
        let mut command = SandboxedCommand::new(&python);
        if let Some(memory_limit_bytes) = memory_limit_bytes {
            command.memory_limit_bytes(memory_limit_bytes);
        }
        configure_worker_environment(&mut command, &python, cache_root, expected_device)?;
        for root in executable_runtime_roots {
            command.executable_read_only_dir(root);
        }
        command.allow_code_generation();
        command
            .current_dir(cache_root)
            .stderr(SandboxedStderr::Piped)
            .arg("-I")
            .arg("-u")
            .arg("-c")
            .arg(WORKER_STDIN_BOOTSTRAP);
        let mut child = command.spawn(&sandbox).map_err(|error| {
            EngineError::Chatterbox(format!(
                "starting sandboxed Chatterbox worker with {} failed: {error}",
                python.display()
            ))
        })?;
        let mut stdin = child.take_stdin().ok_or_else(|| {
            EngineError::Chatterbox("opening Chatterbox worker stdin failed".to_owned())
        })?;
        let worker_source = base64::engine::general_purpose::STANDARD.encode(WORKER.as_bytes());
        stdin
            .write_all(worker_source.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .map_err(|error| {
                EngineError::Chatterbox(format!(
                    "sending Chatterbox worker source over the private bootstrap pipe failed: {error}"
                ))
            })?;
        let stdout = child.take_stdout().ok_or_else(|| {
            EngineError::Chatterbox("opening Chatterbox worker stdout failed".to_owned())
        })?;
        let stderr = child.take_stderr().ok_or_else(|| {
            EngineError::Chatterbox("opening Chatterbox worker stderr failed".to_owned())
        })?;
        let (stdout_tx, stdout_rx) = mpsc::sync_channel(super::WORKER_STDOUT_QUEUE_CAPACITY);
        let stdout_reader = thread::spawn(move || read_worker_stdout(stdout, stdout_tx));
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&stderr_tail);
        let stderr_reader = thread::spawn(move || read_worker_stderr(stderr, capture));
        Ok(Self {
            child,
            stdin,
            stdout_rx: Some(stdout_rx),
            stdout_reader: Some(stdout_reader),
            stderr_tail,
            stderr_reader: Some(stderr_reader),
        })
    }

    fn send(&mut self, id: u64, operation: &str, payload: Value) -> Result<()> {
        let bytes = serde_json::to_vec(&json!({
            "id": id,
            "op": operation,
            "payload": payload,
        }))?;
        if bytes.len() > MAX_WORKER_REQUEST_LINE_BYTES {
            return Err(EngineError::Chatterbox(
                "Chatterbox worker request exceeds the protocol bound".to_owned(),
            ));
        }
        self.stdin.write_all(&bytes)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn read_message(&mut self, wait: Duration) -> Result<Option<WorkerMessage>> {
        let read = match self
            .stdout_rx
            .as_ref()
            .ok_or_else(|| {
                EngineError::Chatterbox("Chatterbox worker stdout reader is closed".to_owned())
            })?
            .recv_timeout(wait)
        {
            Ok(read) => read,
            Err(RecvTimeoutError::Timeout) => return Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                return Err(EngineError::Chatterbox(
                    "Chatterbox worker stdout reader stopped".to_owned(),
                ))
            }
        };
        let line = match read {
            WorkerRead::Line(line) => line,
            WorkerRead::Eof => {
                return Err(self.exit_error("Chatterbox worker exited before replying"))
            }
            WorkerRead::Error(error) => return Err(EngineError::Chatterbox(error)),
        };
        serde_json::from_str(line.trim_end())
            .map(Some)
            .map_err(|error| {
                EngineError::Chatterbox(format!(
                    "decoding Chatterbox worker protocol line failed: {error}"
                ))
            })
    }

    fn stop(&mut self) {
        let _ = self.send(0, "shutdown", Value::Null);
        self.stdout_rx.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.stdout_reader.take() {
            let _ = reader.join();
        }
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
    }

    fn exit_error(&mut self, message: &str) -> EngineError {
        let status = match self.child.try_wait() {
            Ok(Some(status)) => status.to_string(),
            Ok(None) => self
                .child
                .wait()
                .map(|status| status.to_string())
                .unwrap_or_else(|error| format!("unavailable ({error})")),
            Err(error) => format!("unavailable ({error})"),
        };
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
        let stderr = worker_stderr_text(&self.stderr_tail);
        if stderr.is_empty() {
            EngineError::Chatterbox(format!("{message}; exit status {status}; stderr was empty"))
        } else {
            EngineError::Chatterbox(format!(
                "{message}; exit status {status}; stderr tail: {stderr}"
            ))
        }
    }
}

fn configure_worker_environment(
    command: &mut SandboxedCommand,
    python: &Path,
    cache_root: &Path,
    expected_device: Option<&str>,
) -> Result<()> {
    for (name, path) in [
        ("HOME", cache_root.join("home")),
        ("TMPDIR", cache_root.join("tmp")),
        ("TMP", cache_root.join("tmp")),
        ("TEMP", cache_root.join("tmp")),
        ("DARWIN_USER_CACHE_DIR", cache_root.join("darwin-cache")),
        ("DARWIN_USER_TEMP_DIR", cache_root.join("darwin-tmp")),
        ("CFFIXED_USER_HOME", cache_root.join("home")),
        ("XDG_CACHE_HOME", cache_root.join("xdg")),
        ("XDG_RUNTIME_DIR", cache_root.join("xdg-runtime")),
        ("HF_HOME", cache_root.join("huggingface")),
        ("HF_HUB_CACHE", cache_root.join("huggingface/hub")),
        ("TRANSFORMERS_CACHE", cache_root.join("transformers")),
        ("TORCH_HOME", cache_root.join("torch")),
        ("MPLCONFIGDIR", cache_root.join("matplotlib")),
        ("NUMBA_CACHE_DIR", cache_root.join("numba")),
    ] {
        fs::create_dir_all(&path).map_err(|error| {
            EngineError::Chatterbox(format!(
                "creating Chatterbox cache directory {} failed: {error}",
                path.display()
            ))
        })?;
        command.env(name, path);
    }
    command
        .env("HF_HUB_OFFLINE", "1")
        .env("HF_DATASETS_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .env("DIFFUSERS_OFFLINE", "1")
        .env("HF_HUB_DISABLE_TELEMETRY", "1")
        .env("DO_NOT_TRACK", "1")
        .env("PIP_NO_INDEX", "1")
        .env("UV_OFFLINE", "1")
        .env("TOKENIZERS_PARALLELISM", "false")
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for name in [
        "PYTHONHOME",
        "PYTHONPATH",
        "PYTHONSTARTUP",
        "PYTHONINSPECT",
        "LD_PRELOAD",
        "DYLD_INSERT_LIBRARIES",
        "HF_TOKEN",
        "HUGGING_FACE_HUB_TOKEN",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
    ] {
        command.env_remove(name);
    }
    if let Some(device) = expected_device {
        command.env(DEVICE_ENV, device);
    } else {
        command.env_remove(DEVICE_ENV);
    }
    if let Some(python_bin) = python.parent() {
        let mut paths = vec![python_bin.to_path_buf()];
        #[cfg(unix)]
        paths.extend([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
        #[cfg(windows)]
        if let Some(system_root) = env::var_os("SystemRoot") {
            paths.push(PathBuf::from(system_root).join("System32"));
        }
        let path = env::join_paths(paths).map_err(|error| {
            EngineError::Chatterbox(format!("building Chatterbox PATH failed: {error}"))
        })?;
        command.env("PATH", path);
    }
    Ok(())
}

fn python_runtime_roots(python: &Path) -> Result<(PathBuf, Vec<PathBuf>)> {
    let python = resolve_python_program(python)?;
    let canonical_python = fs::canonicalize(&python).map_err(|error| {
        EngineError::Chatterbox(format!(
            "canonicalizing Chatterbox Python {} failed: {error}",
            python.display()
        ))
    })?;
    let executable_root = canonical_python.parent().ok_or_else(|| {
        EngineError::Chatterbox(format!(
            "Chatterbox Python {} has no runtime directory",
            canonical_python.display()
        ))
    })?;
    let mut candidates = vec![executable_root.to_path_buf()];
    if let Some(environment_root) = python
        .parent()
        .filter(|parent| {
            parent
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("bin") || name == "Scripts")
        })
        .and_then(Path::parent)
    {
        candidates.push(environment_root.to_path_buf());
        collect_python_library_roots(environment_root, &mut candidates)?;
        let config = environment_root.join("pyvenv.cfg");
        if let Ok(config) = fs::read_to_string(config) {
            if let Some(home) = config.lines().find_map(|line| {
                line.split_once('=')
                    .filter(|(key, _)| key.trim().eq_ignore_ascii_case("home"))
                    .map(|(_, value)| PathBuf::from(value.trim()))
            }) {
                let base = if home
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("bin") || name == "Scripts")
                {
                    home.parent().unwrap_or(&home)
                } else {
                    home.as_path()
                };
                candidates.push(base.to_path_buf());
                collect_python_library_roots(base, &mut candidates)?;
            }
        }
    }
    let mut roots = Vec::new();
    for candidate in candidates {
        if !candidate.is_dir() {
            continue;
        }
        let candidate = fs::canonicalize(candidate).map_err(|error| {
            EngineError::Chatterbox(format!(
                "canonicalizing Chatterbox Python runtime failed: {error}"
            ))
        })?;
        insert_non_overlapping_root(&mut roots, candidate);
    }
    if roots.is_empty() {
        return Err(EngineError::Chatterbox(
            "Chatterbox Python has no readable runtime roots".to_owned(),
        ));
    }
    Ok((python, roots))
}

fn resolve_python_program(python: &Path) -> Result<PathBuf> {
    let candidates = if python.is_absolute() || python.components().count() > 1 {
        vec![if python.is_absolute() {
            python.to_path_buf()
        } else {
            env::current_dir()
                .map_err(|error| {
                    EngineError::Chatterbox(format!(
                        "resolving current directory for Chatterbox Python failed: {error}"
                    ))
                })?
                .join(python)
        }]
    } else {
        env::var_os("PATH")
            .into_iter()
            .flat_map(|value| env::split_paths(&value).collect::<Vec<_>>())
            .map(|directory| directory.join(python))
            .collect()
    };
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            EngineError::Chatterbox(format!(
                "Chatterbox Python {} was not found",
                python.display()
            ))
        })
}

fn collect_python_library_roots(root: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for library_root in [root.join("lib"), root.join("lib64")] {
        let Ok(entries) = fs::read_dir(&library_root) else {
            continue;
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("python"))
            {
                output.push(entry.path());
            }
        }
    }
    let windows_packages = root.join("Lib/site-packages");
    if windows_packages.is_dir() {
        output.push(windows_packages);
    }
    Ok(())
}

fn insert_non_overlapping_root(roots: &mut Vec<PathBuf>, candidate: PathBuf) {
    if roots.iter().any(|root| candidate.starts_with(root)) {
        return;
    }
    roots.retain(|root| !root.starts_with(&candidate));
    roots.push(candidate);
}

enum WorkerRead {
    Line(String),
    Eof,
    Error(String),
}

fn read_worker_stdout(stdout: SandboxedChildStdout, sender: mpsc::SyncSender<WorkerRead>) {
    let mut stdout = BufReader::new(stdout);
    loop {
        match read_bounded_line(&mut stdout, MAX_WORKER_RESPONSE_LINE_BYTES) {
            Ok(Some(line)) => {
                if sender.send(WorkerRead::Line(line)).is_err() {
                    return;
                }
            }
            Ok(None) => {
                let _ = sender.send(WorkerRead::Eof);
                return;
            }
            Err(error) => {
                let _ = sender.send(WorkerRead::Error(format!(
                    "reading Chatterbox worker stdout failed: {error}"
                )));
                return;
            }
        }
    }
}

fn read_bounded_line(reader: &mut impl BufRead, maximum: usize) -> std::io::Result<Option<String>> {
    let mut bytes = Vec::new();
    loop {
        let (chunk, done) = {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                if bytes.is_empty() {
                    return Ok(None);
                }
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "worker protocol line ended without a newline",
                ));
            }
            let length = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |index| index + 1);
            (available[..length].to_vec(), available[length - 1] == b'\n')
        };
        if bytes.len().saturating_add(chunk.len()) > maximum {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "worker protocol line exceeded its bound",
            ));
        }
        reader.consume(chunk.len());
        bytes.extend_from_slice(&chunk);
        if done {
            return String::from_utf8(bytes)
                .map(Some)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error));
        }
    }
}

fn read_worker_stderr(mut stderr: SandboxedChildStderr, tail: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0_u8; 4096];
    loop {
        match stderr.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                let mut tail = match tail.lock() {
                    Ok(tail) => tail,
                    Err(_) => return,
                };
                if read >= WORKER_STDERR_TAIL_BYTES {
                    tail.clear();
                    tail.extend_from_slice(&chunk[read - WORKER_STDERR_TAIL_BYTES..read]);
                    continue;
                }
                let overflow = tail
                    .len()
                    .saturating_add(read)
                    .saturating_sub(WORKER_STDERR_TAIL_BYTES);
                if overflow > 0 {
                    tail.drain(..overflow);
                }
                tail.extend_from_slice(&chunk[..read]);
            }
        }
    }
}

fn worker_stderr_text(tail: &Arc<Mutex<Vec<u8>>>) -> String {
    tail.lock()
        .map(|tail| String::from_utf8_lossy(&tail).trim().to_owned())
        .unwrap_or_else(|_| "stderr capture lock was poisoned".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn normalized_request_preserves_original_upstream_defaults() {
        let request =
            NormalizedSpeechRequest::try_from(ChatterboxSpeechRequest::new("Hello world"))
                .expect("normalize");
        assert_eq!(request.exaggeration, 0.5);
        assert_eq!(request.cfg_weight, 0.5);
        assert_eq!(request.temperature, 0.8);
        assert_eq!(request.repetition_penalty, 1.2);
        assert_eq!(request.min_p, 0.05);
        assert_eq!(request.top_p, 1.0);
        assert_eq!(request.seed, None);
        let payload = request.worker_payload();
        assert_eq!(payload["input"], "Hello world");
        assert!(payload["reference_audio"].is_null());
        assert!(payload["seed"].is_null());
        for (field, expected) in [
            ("cfg_weight", 0.5),
            ("exaggeration", 0.5),
            ("min_p", 0.05),
            ("repetition_penalty", 1.2),
            ("temperature", 0.8),
            ("top_p", 1.0),
        ] {
            let actual = payload[field].as_f64().unwrap();
            assert!((actual - expected).abs() < 0.000_001, "{field}");
        }
    }

    #[test]
    fn inline_clone_is_bounded_and_encoded_for_the_private_worker() {
        let wav = pcm_wav(24_000, 24_000);
        let mut speech = ChatterboxSpeechRequest::new("Clone this voice");
        speech.reference_audio = Some(ChatterboxReferenceAudio::wav(wav.clone()));
        speech.exaggeration = Some(0.7);
        speech.cfg_weight = Some(0.3);
        speech.temperature = Some(1.1);
        speech.seed = Some(7);
        let request = NormalizedSpeechRequest::try_from(speech).expect("normalize");
        let payload = request.worker_payload();
        for (field, expected) in [
            ("exaggeration", 0.7),
            ("cfg_weight", 0.3),
            ("temperature", 1.1),
        ] {
            let actual = payload[field].as_f64().unwrap();
            assert!((actual - expected).abs() < 0.000_001, "{field}");
        }
        assert_eq!(payload["seed"], 7);
        assert_eq!(payload["reference_audio"]["content_type"], "audio/wav");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(payload["reference_audio"]["data_base64"].as_str().unwrap())
                .unwrap(),
            wav
        );
    }

    #[test]
    fn invalid_controls_and_non_wav_references_are_rejected() {
        let mut request = ChatterboxSpeechRequest::new("Hello");
        request.exaggeration = Some(2.01);
        assert!(NormalizedSpeechRequest::try_from(request).is_err());

        let mut request = ChatterboxSpeechRequest::new("Hello");
        request.reference_audio = Some(ChatterboxReferenceAudio {
            content_type: Some("audio/mpeg".to_owned()),
            data: vec![0; 128],
        });
        assert!(NormalizedSpeechRequest::try_from(request).is_err());
    }

    #[test]
    fn generic_speech_rejects_controls_the_original_api_does_not_have() {
        let named_voice = SpeechRequest {
            input: "Hello".to_owned(),
            voice: Some("alloy".to_owned()),
            response_format: Some("wav".to_owned()),
            speed: None,
            ..SpeechRequest::new("")
        };
        assert!(ChatterboxSpeechRequest::try_from(named_voice).is_err());

        let speed = SpeechRequest {
            input: "Hello".to_owned(),
            voice: None,
            response_format: Some("wav".to_owned()),
            speed: Some(1.25),
            ..SpeechRequest::new("")
        };
        assert!(ChatterboxSpeechRequest::try_from(speed).is_err());
    }

    #[test]
    fn shared_speech_request_carries_clone_audio_and_all_controls() {
        let wav = pcm_wav(24_000, 24_000);
        let mut shared = SpeechRequest::new("Shared provider path");
        shared.response_format = Some("wav".to_owned());
        shared.reference_audio = Some(SpeechReferenceAudio::wav(wav.clone()));
        shared.exaggeration = Some(0.7);
        shared.cfg_weight = Some(0.3);
        shared.temperature = Some(1.1);
        shared.seed = Some(11);
        shared.repetition_penalty = Some(1.4);
        shared.min_p = Some(0.08);
        shared.top_p = Some(0.9);

        let chatterbox = ChatterboxSpeechRequest::try_from(shared).expect("convert shared request");
        assert_eq!(chatterbox.reference_audio.unwrap().data, wav);
        assert_eq!(chatterbox.exaggeration, Some(0.7));
        assert_eq!(chatterbox.cfg_weight, Some(0.3));
        assert_eq!(chatterbox.temperature, Some(1.1));
        assert_eq!(chatterbox.seed, Some(11));
        assert_eq!(chatterbox.repetition_penalty, Some(1.4));
        assert_eq!(chatterbox.min_p, Some(0.08));
        assert_eq!(chatterbox.top_p, Some(0.9));
    }

    #[test]
    fn semantic_speech_validation_checks_controls_without_starting_a_worker() {
        let wav = pcm_wav(24_000, 24_000);
        let mut request = SpeechRequest::new("Validate without generating audio");
        request.voice = Some("default".to_owned());
        request.response_format = Some("wav".to_owned());
        request.speed = Some(1.0);
        request.reference_audio = Some(SpeechReferenceAudio::wav(wav.clone()));
        request.exaggeration = Some(0.7);
        request.cfg_weight = Some(0.3);
        request.temperature = Some(1.1);
        request.seed = Some(11);
        request.repetition_penalty = Some(1.4);
        request.min_p = Some(0.08);
        request.top_p = Some(0.9);

        let mut backend = ChatterboxBackend::with_python("unused-python").unwrap();
        backend.loaded = Some(LoadedModelInfo {
            backend: "chatterbox".to_owned(),
            artifact: crate::ModelArtifact {
                path: PathBuf::from("unused"),
                format: ArtifactFormat::ChatterboxSafetensors,
                sha256: None,
                sha256_path: None,
            },
            ctx_size: MAX_INPUT_CHARACTERS,
            n_ctx_train: T3_MAX_TEXT_TOKENS,
            n_vocab: 0,
        });
        backend.execution_config = Some(test_execution_config());

        assert!(backend.process_ids().is_empty());
        let validation = backend
            .validate_speech(request, &CancellationToken::new())
            .unwrap()
            .expect("semantic validation");
        assert!(backend.process_ids().is_empty());
        assert_eq!(
            validation.handled_controls,
            SEMANTICALLY_VALIDATED_SPEECH_CONTROLS
                .iter()
                .map(|value| (*value).to_owned())
                .collect()
        );
        assert_eq!(
            validation.evidence["controls"]["reference_audio"]["byte_count"],
            wav.len()
        );
        assert_eq!(
            validation.evidence["controls"]["reference_audio"]["sha256"],
            format!("{:x}", Sha256::digest(&wav))
        );
        assert!(
            !serde_json::to_string(&validation)
                .unwrap()
                .contains(&base64::engine::general_purpose::STANDARD.encode(&wav)),
            "semantic evidence retained the private reference bytes"
        );
    }

    #[test]
    fn canonical_manifest_pins_all_original_english_artifacts() {
        assert_eq!(
            CANONICAL_MODEL_FILES
                .iter()
                .map(|spec| (spec.name, spec.size, spec.sha256))
                .collect::<Vec<_>>(),
            vec![
                (
                    "ve.safetensors",
                    5_695_784,
                    "f0921cab452fa278bc25cd23ffd59d36f816d7dc5181dd1bef9751a7fb61f63c"
                ),
                (
                    "t3_cfg.safetensors",
                    2_129_653_744,
                    "914cb1696f47527fe8852ca8f1fe1fa63cb34f76f9c715e84e067b744dd0da81"
                ),
                (
                    "s3gen.safetensors",
                    1_056_484_620,
                    "2b78103c654207393955e4900aac14a12de8ef25f4b09424f1ef91941f161d4e"
                ),
                (
                    "tokenizer.json",
                    25_470,
                    "d71e3a44eabb1784df9a68e9f95b251ecbf1a7af6a9f50835856b2ca9d8c14a5"
                ),
                (
                    "conds.pt",
                    107_374,
                    "6552d70568833628ba019c6b03459e77fe71ca197d5c560cef9411bee9d87f4e"
                ),
            ]
        );
    }

    #[test]
    fn model_root_rejects_same_size_corruption_and_wrong_sizes() {
        let tree = TestTree::new();
        let path = tree.path.join("tokenizer.json");
        fs::write(&path, b"abc").unwrap();
        let specs = [ModelFileSpec {
            name: "tokenizer.json",
            size: 3,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        }];
        validate_model_root_with_specs(&tree.path, &specs).expect("valid fixture");

        fs::write(&path, b"abd").unwrap();
        let error = validate_model_root_with_specs(&tree.path, &specs).unwrap_err();
        assert!(error.to_string().contains("hash mismatch"));

        fs::write(&path, b"ab").unwrap();
        let error = validate_model_root_with_specs(&tree.path, &specs).unwrap_err();
        assert!(error.to_string().contains("has 2 bytes, expected 3"));
    }

    #[test]
    fn worker_source_is_original_offline_and_has_no_remote_fetch_path() {
        assert!(WORKER.contains("from chatterbox.tts import ChatterboxTTS"));
        assert!(WORKER.contains("chatterbox_tts.from_local"));
        assert!(WORKER.contains("HF_HUB_OFFLINE"));
        assert!(WORKER.contains("model.conds = None"));
        assert!(!WORKER.contains("ChatterboxTurboTTS"));
        assert!(!WORKER.contains("from_pretrained"));
        assert!(!WORKER.contains("hf_hub_download"));
        assert!(!WORKER.contains("requests."));
    }

    #[test]
    fn bounded_protocol_reader_accepts_one_line_and_rejects_overflow() {
        let mut reader = Cursor::new(b"{\"ok\":true}\n".to_vec());
        assert_eq!(
            read_bounded_line(&mut reader, 32).unwrap().unwrap(),
            "{\"ok\":true}\n"
        );
        let mut reader = Cursor::new(b"oversized\n".to_vec());
        assert_eq!(
            read_bounded_line(&mut reader, 4).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn worker_startup_purges_stale_inputs_without_following_links() {
        let tree = TestTree::new();
        let worker_cache = tree.path.join("worker-cache");
        let inputs = worker_cache.join("inputs");
        fs::create_dir_all(&inputs).unwrap();

        let stale_reference = inputs.join("reference-stale.wav");
        fs::write(&stale_reference, b"private voice bytes").unwrap();
        let link_target = tree.path.join("link-target.wav");
        fs::write(&link_target, b"must survive cleanup").unwrap();
        let stale_link = inputs.join("reference-link.wav");
        create_file_symlink(&link_target, &stale_link);
        let preserved_directory = inputs.join("not-a-worker-input");
        fs::create_dir(&preserved_directory).unwrap();

        purge_worker_inputs(&worker_cache).expect("startup cleanup");

        assert!(!stale_reference.exists());
        assert!(fs::symlink_metadata(&stale_link).is_err());
        assert_eq!(fs::read(&link_target).unwrap(), b"must survive cleanup");
        assert!(preserved_directory.is_dir());
    }

    #[cfg(unix)]
    fn create_file_symlink(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[cfg(windows)]
    fn create_file_symlink(target: &Path, link: &Path) {
        std::os::windows::fs::symlink_file(target, link).unwrap();
    }

    fn pcm_wav(sample_rate: u32, sample_count: u32) -> Vec<u8> {
        let data_len = sample_count * 2;
        let mut bytes = Vec::with_capacity(44 + data_len as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.resize(44 + data_len as usize, 0);
        bytes
    }

    fn test_execution_config() -> ChatterboxExecutionConfig {
        ChatterboxExecutionConfig {
            api_version: 1,
            model_family: MODEL_FAMILY.to_owned(),
            runtime_package: RUNTIME_PACKAGE.to_owned(),
            runtime_version: RUNTIME_VERSION.to_owned(),
            source_commit: CHATTERBOX_SOURCE_COMMIT.to_owned(),
            model_revision: CHATTERBOX_MODEL_REVISION.to_owned(),
            runtime_source_sha256: CHATTERBOX_TTS_SOURCE_SHA256.to_owned(),
            perth_commit: CHATTERBOX_PERTH_COMMIT.to_owned(),
            device: "cpu".to_owned(),
            sample_rate: 24_000,
            input_character_limit: MAX_INPUT_CHARACTERS,
            input_byte_limit: MAX_INPUT_BYTES as u32,
            max_text_tokens: T3_MAX_TEXT_TOKENS,
            reference_audio_limit_seconds: MAX_REFERENCE_AUDIO_SECONDS as u32,
            t3_reference_seconds: T3_REFERENCE_SECONDS,
            s3gen_reference_seconds: S3GEN_REFERENCE_SECONDS,
            supports_voice_cloning: true,
            seed_semantics: "official_gradio_global_rng_nonzero".to_owned(),
        }
    }

    #[test]
    fn execution_config_must_match_selected_admission_device() {
        let mut config = test_execution_config();
        validate_execution_config(&config, Some("cpu")).expect("matching CPU device");
        let error = validate_execution_config(&config, Some("cuda"))
            .expect_err("CPU report must not satisfy CUDA admission");
        assert!(error
            .to_string()
            .contains("provider admission selected cuda"));

        config.device = "cuda".to_owned();
        validate_execution_config(&config, Some("cuda")).expect("matching CUDA device");
        assert!(validate_execution_config(&config, Some("mps")).is_err());
    }

    #[test]
    fn expected_device_rejects_unknown_values_and_keeps_auto_unpinned() {
        assert_eq!(normalize_expected_device("auto").unwrap(), None);
        assert_eq!(
            normalize_expected_device(" CUDA ").unwrap().as_deref(),
            Some("cuda")
        );
        assert!(normalize_expected_device("rocm").is_err());
    }

    struct TestTree {
        path: PathBuf,
    }

    impl TestTree {
        fn new() -> Self {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "mayhem-chatterbox-test-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TestTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
