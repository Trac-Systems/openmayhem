use super::{
    validate_load_config, verify_artifact, ArtifactChunk, ArtifactFormat, ArtifactSink,
    CancellationToken, EngineBackend, EngineError, GenerateOutput, GenerateRequest,
    LlamaCppBackend, LoadConfig, LoadedModelInfo, MediaGenerationOutput, MediaGenerationRequest,
    MediaGenerationValidation, MediaInput, ModelArtifact, NoopTokenSink, Result, TokenSink,
    Tokenization, MTMD_MEDIA_MARKER,
};
use base64::Engine as _;
use mayhem_enclave::{
    SandboxConfig, SandboxedChild, SandboxedChildStderr, SandboxedChildStdin, SandboxedChildStdout,
    SandboxedCommand, SandboxedStderr,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const WORKER: &str = include_str!("sulphur_worker.py");
const WORKER_STDIN_BOOTSTRAP: &str = concat!(
    "import base64,sys;",
    "exec(compile(base64.b64decode(sys.stdin.buffer.readline()),",
    "'<mayhem-sulphur-worker>','exec'))"
);
const PYTHON_ENV: &str = "MAYHEM_SULPHUR_PYTHON";
const RUNTIME_MODULE_ENV: &str = "MAYHEM_SULPHUR_RUNTIME_MODULE";
const FFMPEG_ENV: &str = "MAYHEM_SULPHUR_FFMPEG";
const FFPROBE_ENV: &str = "MAYHEM_SULPHUR_FFPROBE";
const PROMPT_ENHANCER_GPU_LAYERS_ENV: &str = "MAYHEM_SULPHUR_ENHANCER_GPU_LAYERS";
const DEFAULT_RUNTIME_MODULE: &str = "mayhem_sulphur_runtime";
const MLX_RUNTIME_MANIFEST_NAME: &str = "mayhem-sulphur-mlx-runtime.json";
const ENDPOINT_OPENAI_VIDEOS: &str = "openai_videos";
const ENDPOINT_HF_TEXT_TO_VIDEO: &str = "hf_text_to_video";
pub const LTX_RUNTIME_COMMIT: &str = "9377758131b1ffde4b7f766804590a6617bf2ab9";
pub const SULPHUR_SOURCE_COMMIT: &str = "875e886e556b955d21149316fd631cc121db6cc1";
const DISTILLED_STAGE_1_INTERVALS: u64 = 8;
const DISTILLED_STAGE_2_INTERVALS: u64 = 3;
const DISTILLED_DENOISE_INTERVALS: u64 = DISTILLED_STAGE_1_INTERVALS + DISTILLED_STAGE_2_INTERVALS;
const DEFAULT_WIDTH: u64 = 768;
const DEFAULT_HEIGHT: u64 = 512;
const DEFAULT_FPS: f64 = 24.0;
const MAX_PROMPT_BYTES: usize = 32 * 1024;
const MAX_CONDITIONING_IMAGES: usize = 16;
const MAX_CONDITIONING_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_CONDITIONING_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const DEFAULT_CONDITIONING_CRF: u64 = 33;
const MAX_OUTPUT_BYTES: u64 = 1024 * 1024 * 1024;
const PROMPT_ENHANCER_CONTEXT: u32 = 16 * 1024;
const PROMPT_ENHANCER_MAX_NEW_TOKENS: u32 = 2 * 1024;
const ARTIFACT_CHUNK_BYTES: usize = 256 * 1024;
const WORKER_STDERR_TAIL_BYTES: usize = 64 * 1024;
const MEDIA_TOOL_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const MEDIA_TOOL_PROBE_MAX_OUTPUT_BYTES: u64 = 2 * 1024 * 1024;
static MEDIA_TOOL_PROBE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct SulphurBackend {
    python: PathBuf,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    runtime_module: String,
    worker: Option<SulphurWorker>,
    loaded: Option<LoadedModelInfo>,
    config: Option<LoadConfig>,
    model_root: Option<PathBuf>,
    artifact_backend: Option<String>,
    worker_cache: Option<PathBuf>,
    execution_config: Option<SulphurExecutionConfig>,
    prompt_enhancer: Option<LlamaCppBackend>,
    next_id: u64,
    next_artifact_id: u64,
}

impl SulphurBackend {
    pub fn new() -> Result<Self> {
        let python = env::var_os(PYTHON_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("python3"));
        let runtime_module =
            env::var(RUNTIME_MODULE_ENV).unwrap_or_else(|_| DEFAULT_RUNTIME_MODULE.to_owned());
        Self::with_runtime(python, runtime_module)
    }

    pub fn with_python(python: impl Into<PathBuf>) -> Result<Self> {
        Self::with_runtime(python, DEFAULT_RUNTIME_MODULE)
    }

    pub fn with_runtime(
        python: impl Into<PathBuf>,
        runtime_module: impl Into<String>,
    ) -> Result<Self> {
        let runtime_module = runtime_module.into();
        validate_module_name(&runtime_module)?;
        Ok(Self {
            python: python.into(),
            ffmpeg: env::var_os(FFMPEG_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("ffmpeg")),
            ffprobe: env::var_os(FFPROBE_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("ffprobe")),
            runtime_module,
            worker: None,
            loaded: None,
            config: None,
            model_root: None,
            artifact_backend: None,
            worker_cache: None,
            execution_config: None,
            prompt_enhancer: None,
            next_id: 1,
            next_artifact_id: 1,
        })
    }

    pub fn execution_config(&self) -> Option<&SulphurExecutionConfig> {
        self.execution_config.as_ref()
    }

    fn preflight_media_tools(&mut self) -> Result<()> {
        let tools = preflight_media_tools(&self.ffmpeg, &self.ffprobe)?;
        self.ffmpeg = tools.ffmpeg;
        self.ffprobe = tools.ffprobe;
        Ok(())
    }

    fn ensure_worker_loaded(&mut self) -> Result<()> {
        if self.worker.is_some() {
            return Ok(());
        }
        self.preflight_media_tools()?;
        let config = self.config.clone().ok_or(EngineError::NotLoaded)?;
        let model_root = self
            .model_root
            .clone()
            .ok_or_else(|| EngineError::Sulphur("Sulphur model root is not prepared".to_owned()))?;
        let backend = self.artifact_backend.clone().ok_or_else(|| {
            EngineError::Sulphur("Sulphur artifact backend is not prepared".to_owned())
        })?;
        let worker_cache = self.worker_cache.clone().ok_or_else(|| {
            EngineError::Sulphur("Sulphur worker cache is not prepared".to_owned())
        })?;
        self.worker = Some(SulphurWorker::spawn(
            &self.python,
            &self.ffmpeg,
            &self.ffprobe,
            config.memory_limit_bytes,
            &worker_cache,
            &model_root,
        )?);
        let info = match self.load_existing_worker(&config, &model_root, &worker_cache, &backend) {
            Ok(info) => info,
            Err(error) => {
                self.stop_worker();
                return Err(error);
            }
        };
        let mut execution_config = info.execution_config;
        execution_config.prompt_enhancer = self.prompt_enhancer.is_some();
        self.execution_config = Some(execution_config);
        Ok(())
    }

    fn load_existing_worker(
        &mut self,
        config: &LoadConfig,
        model_root: &Path,
        worker_cache: &Path,
        backend: &str,
    ) -> Result<WorkerLoadInfo> {
        let info: WorkerLoadInfo = self.call_existing(
            "load",
            json!({
                "artifact_path": config.artifact.path,
                "backend": backend,
                "cache_root": worker_cache,
                "ffmpeg_path": resolve_program(&self.ffmpeg, "ffmpeg")?,
                "ffprobe_path": resolve_program(&self.ffprobe, "ffprobe")?,
                "model_root": model_root,
                "runtime_module": self.runtime_module,
            }),
            None,
        )?;
        validate_execution_config(&info.execution_config, backend)?;
        if info.worker_pid == 0 {
            self.stop_worker();
            return Err(EngineError::Sulphur(
                "Sulphur worker returned an invalid PID".to_owned(),
            ));
        }
        #[cfg(unix)]
        if info.process_group_id != Some(info.worker_pid) {
            self.stop_worker();
            return Err(EngineError::Sulphur(
                "Sulphur worker is not the leader of its cancellation process group".to_owned(),
            ));
        }
        self.worker_mut()?.worker_pid = Some(info.worker_pid);
        Ok(info)
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
                return Err(EngineError::Sulphur(format!(
                    "worker response id {} did not match request id {id}",
                    message.id
                )));
            }
            if !message.ok {
                return Err(EngineError::Sulphur(
                    message
                        .error
                        .unwrap_or_else(|| "worker returned an unknown error".to_owned()),
                ));
            }
            return serde_json::from_value(message.result.unwrap_or(Value::Null)).map_err(
                |error| {
                    self.stop_worker();
                    EngineError::Sulphur(format!(
                        "decoding Sulphur worker response failed: {error}"
                    ))
                },
            );
        }
    }

    fn worker_mut(&mut self) -> Result<&mut SulphurWorker> {
        self.worker
            .as_mut()
            .ok_or_else(|| EngineError::Sulphur("Sulphur worker is not running".to_owned()))
    }

    fn stop_worker(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            worker.stop();
        }
        if let Some(cache) = &self.worker_cache {
            purge_worker_inputs(cache);
        }
    }

    fn load_prompt_enhancer(&mut self, config: &LoadConfig) -> Result<()> {
        self.prompt_enhancer = None;
        let (Some(model), Some(projector)) = (
            config.prompt_enhancer_model.as_ref(),
            config.prompt_enhancer_projector.as_ref(),
        ) else {
            if config.prompt_enhancer_model.is_some() || config.prompt_enhancer_projector.is_some()
            {
                return Err(EngineError::InvalidConfig(
                    "Sulphur prompt enhancement requires both its signed GGUF and mmproj"
                        .to_owned(),
                ));
            }
            return Ok(());
        };
        if model.format != ArtifactFormat::Gguf || projector.format != ArtifactFormat::Gguf {
            return Err(EngineError::InvalidConfig(
                "Sulphur prompt enhancer and projector must both be GGUF artifacts".to_owned(),
            ));
        }
        let enhancer_config =
            prompt_enhancer_load_config(config, model, projector, prompt_enhancer_gpu_layers()?);
        let mut enhancer = LlamaCppBackend::new()?;
        enhancer.load(enhancer_config)?;
        self.prompt_enhancer = Some(enhancer);
        Ok(())
    }

    fn enhance_prompt(
        &mut self,
        request: &mut NormalizedVideoRequest,
        cancellation: &CancellationToken,
    ) -> Result<()> {
        if !request.enhance_prompt {
            return Ok(());
        }
        let enhancer = self.prompt_enhancer.as_mut().ok_or_else(|| {
            EngineError::InvalidConfig(
                "Sulphur prompt enhancement was requested but this provider did not load the signed enhancer"
                    .to_owned(),
            )
        })?;
        let mut prompt = String::from("<|im_start|>user\n");
        let mut media = Vec::new();
        if let Some(image) = request.images.first() {
            prompt.push_str(MTMD_MEDIA_MARKER);
            media.push(MediaInput {
                kind: "image".to_owned(),
                content_type: Some(image.content_type.clone()),
                url: None,
                data: Some(image.data_base64.clone()),
                frames: Vec::new(),
                num_frames: None,
                fps: None,
            });
        }
        prompt.push_str(&request.prompt);
        prompt.push_str("<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n");
        let mut generation = GenerateRequest::new(prompt);
        generation.media = media;
        generation.max_new_tokens = PROMPT_ENHANCER_MAX_NEW_TOKENS;
        generation.temperature = Some(0.0);
        generation.seed = Some(u32::try_from(request.seed).map_err(|_| {
            EngineError::InvalidConfig("Sulphur enhancer seed exceeds u32".to_owned())
        })?);
        generation.stop = vec!["<|im_end|>".to_owned()];
        let mut sink = NoopTokenSink;
        let output = enhancer.generate(generation, &mut sink, cancellation)?;
        let enhanced = output.text.trim();
        if enhanced.is_empty() || enhanced.len() > MAX_PROMPT_BYTES {
            return Err(EngineError::Sulphur(
                "Sulphur prompt enhancer returned an empty or oversized prompt".to_owned(),
            ));
        }
        request.prompt = enhanced.to_owned();
        Ok(())
    }
}

fn prompt_enhancer_load_config(
    parent: &LoadConfig,
    model: &ModelArtifact,
    projector: &ModelArtifact,
    gpu_layers: u32,
) -> LoadConfig {
    let mut config = LoadConfig::gguf(&model.path);
    config.artifact = model.clone();
    config.vision_projector = Some(projector.clone());
    config.ctx_size = PROMPT_ENHANCER_CONTEXT;
    config.threads = parent.threads;
    config.gpu_layers = Some(gpu_layers);
    config.kv_cache_dtype = Some("q8_0".to_owned());
    config.kv_cache_bits = Some(8);
    config.kv_cache_group_size = Some(32);
    config.kv_cache_quantized_start_tokens = Some(0);
    config.use_mmap = true;
    config.use_mlock = false;
    config
}

fn prompt_enhancer_gpu_layers() -> Result<u32> {
    let Some(value) = env::var_os(PROMPT_ENHANCER_GPU_LAYERS_ENV) else {
        return Ok(0);
    };
    value
        .to_str()
        .ok_or_else(|| {
            EngineError::InvalidConfig(format!(
                "{PROMPT_ENHANCER_GPU_LAYERS_ENV} must be valid UTF-8"
            ))
        })?
        .parse::<u32>()
        .map_err(|_| {
            EngineError::InvalidConfig(format!(
                "{PROMPT_ENHANCER_GPU_LAYERS_ENV} must be an unsigned integer"
            ))
        })
}

fn purge_worker_inputs(cache_root: &Path) {
    let inputs = cache_root.join("inputs");
    let Ok(entries) = fs::read_dir(inputs) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if fs::symlink_metadata(&path)
            .is_ok_and(|metadata| metadata.is_file() || metadata.file_type().is_symlink())
        {
            let _ = fs::remove_file(path);
        }
    }
}

impl EngineBackend for SulphurBackend {
    fn backend_id(&self) -> &'static str {
        "sulphur"
    }

    fn load(&mut self, mut config: LoadConfig) -> Result<LoadedModelInfo> {
        self.preflight_media_tools()?;
        validate_load_config(&config)?;
        let (backend, model_root) = match config.artifact.format {
            ArtifactFormat::Gguf => {
                let parent = config.artifact.path.parent().ok_or_else(|| {
                    EngineError::InvalidConfig(format!(
                        "Sulphur GGUF artifact {} has no model root",
                        config.artifact.path.display()
                    ))
                })?;
                ("gguf", parent.to_path_buf())
            }
            ArtifactFormat::MlxSafetensors => {
                if !config.artifact.path.is_dir() {
                    return Err(EngineError::InvalidConfig(
                        "Sulphur MLX artifact must be a model directory".to_owned(),
                    ));
                }
                let model_root = config.artifact.path.parent().ok_or_else(|| {
                    EngineError::InvalidConfig(format!(
                        "Sulphur MLX artifact {} has no signed bundle root",
                        config.artifact.path.display()
                    ))
                })?;
                if !model_root.join(MLX_RUNTIME_MANIFEST_NAME).is_file() {
                    return Err(EngineError::InvalidConfig(format!(
                        "Sulphur MLX artifact {} is missing {} in its signed bundle root",
                        config.artifact.path.display(),
                        MLX_RUNTIME_MANIFEST_NAME
                    )));
                }
                ("mlx", model_root.to_path_buf())
            }
            ref actual => {
                return Err(EngineError::InvalidConfig(format!(
                    "Sulphur requires a GGUF or MLX artifact, got {actual:?}"
                )))
            }
        };
        verify_artifact(&config.artifact)?;
        config.artifact.path = fs::canonicalize(&config.artifact.path).map_err(|error| {
            EngineError::Sulphur(format!(
                "canonicalizing Sulphur artifact {} failed: {error}",
                config.artifact.path.display()
            ))
        })?;
        let model_root = fs::canonicalize(&model_root).map_err(|error| {
            EngineError::Sulphur(format!(
                "canonicalizing Sulphur model root {} failed: {error}",
                model_root.display()
            ))
        })?;
        let cache_root = sulphur_cache_root(config.backend_cache_dir.as_deref());
        let worker_cache = sulphur_worker_cache(&cache_root, &model_root)?;
        fs::create_dir_all(&worker_cache).map_err(|error| {
            EngineError::Sulphur(format!(
                "creating Sulphur worker cache {} failed: {error}",
                worker_cache.display()
            ))
        })?;
        let worker_cache = fs::canonicalize(&worker_cache).map_err(|error| {
            EngineError::Sulphur(format!(
                "canonicalizing Sulphur worker cache {} failed: {error}",
                worker_cache.display()
            ))
        })?;

        self.stop_worker();
        self.loaded = None;
        self.config = Some(config.clone());
        self.model_root = Some(model_root.clone());
        self.artifact_backend = Some(backend.to_owned());
        self.worker_cache = Some(worker_cache.clone());
        self.execution_config = None;
        self.prompt_enhancer = None;
        self.worker = Some(SulphurWorker::spawn(
            &self.python,
            &self.ffmpeg,
            &self.ffprobe,
            config.memory_limit_bytes,
            &worker_cache,
            &model_root,
        )?);
        let worker_info =
            match self.load_existing_worker(&config, &model_root, &worker_cache, backend) {
                Ok(info) => info,
                Err(error) => {
                    self.stop_worker();
                    self.config = None;
                    self.model_root = None;
                    self.artifact_backend = None;
                    self.worker_cache = None;
                    return Err(error);
                }
            };
        if let Err(error) = self.load_prompt_enhancer(&config) {
            self.stop_worker();
            self.config = None;
            self.model_root = None;
            self.artifact_backend = None;
            self.worker_cache = None;
            return Err(error);
        }
        let mut execution_config = worker_info.execution_config;
        execution_config.prompt_enhancer = self.prompt_enhancer.is_some();
        self.execution_config = Some(execution_config);
        let loaded = LoadedModelInfo {
            backend: self.backend_id().to_owned(),
            artifact: config.artifact,
            ctx_size: config.ctx_size,
            n_ctx_train: worker_info.n_ctx_train,
            n_vocab: worker_info.n_vocab,
        };
        self.loaded = Some(loaded.clone());
        Ok(loaded)
    }

    fn loaded_backend_evidence(&self) -> Option<Value> {
        self.execution_config
            .as_ref()
            .and_then(|value| serde_json::to_value(value).ok())
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
            .and_then(|worker| worker.worker_pid)
            .into_iter()
            .collect()
    }

    fn tokenize(&self, text: &str) -> Result<Tokenization> {
        self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
        Ok(Tokenization {
            token_ids: text
                .split_whitespace()
                .enumerate()
                .map(|(index, _)| i32::try_from(index).unwrap_or(i32::MAX))
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
            "Sulphur generates synchronized video and audio; use generate_video".to_owned(),
        ))
    }

    fn generate_video(
        &mut self,
        request: MediaGenerationRequest,
        artifact_sink: &mut dyn ArtifactSink,
        cancellation: &CancellationToken,
    ) -> Result<MediaGenerationOutput> {
        if cancellation.is_cancelled() {
            self.stop_worker();
            return Err(EngineError::Cancelled);
        }
        self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
        let mut normalized = NormalizedVideoRequest::from_media_request(request)?;
        self.enhance_prompt(&mut normalized, cancellation)?;
        let output_path = self.next_output_path()?;
        let _cleanup = OutputCleanup(output_path.clone());
        let expected_controls = normalized.handled_controls();
        let response: WorkerGenerateResult = self.call(
            "generate_video",
            json!({
                "output_path": output_path,
                "request": normalized.worker_payload(),
            }),
            Some(cancellation),
        )?;
        validate_worker_result(&response, &normalized, &expected_controls, &output_path)?;
        let mp4 = inspect_joint_mp4(&output_path)?;
        validate_mp4_timing(&mp4, &response, &normalized)?;
        stream_artifact(
            &output_path,
            format!("sulphur-{}", self.next_artifact_id),
            artifact_sink,
            cancellation,
        )?;
        self.next_artifact_id = self.next_artifact_id.saturating_add(1);
        Ok(MediaGenerationOutput {
            duration_seconds: response.duration_seconds.ceil() as u64,
            frame_count: response.frame_count,
            step_count: DISTILLED_DENOISE_INTERVALS,
        })
    }

    fn validate_media_generation(
        &mut self,
        request: MediaGenerationRequest,
        cancellation: &CancellationToken,
    ) -> Result<Option<MediaGenerationValidation>> {
        cancellation.check()?;
        self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
        let mut normalized = NormalizedVideoRequest::from_media_request(request)?;
        self.enhance_prompt(&mut normalized, cancellation)?;
        let evidence = self.call(
            "validate_video",
            normalized.worker_payload(),
            Some(cancellation),
        )?;
        Ok(Some(MediaGenerationValidation {
            evidence,
            handled_request_attributes: normalized.handled_request_attributes,
        }))
    }
}

impl SulphurBackend {
    fn next_output_path(&self) -> Result<PathBuf> {
        let cache = self.worker_cache.as_ref().ok_or(EngineError::NotLoaded)?;
        let outputs = cache.join("outputs");
        fs::create_dir_all(&outputs).map_err(|error| {
            EngineError::Sulphur(format!(
                "creating Sulphur output directory {} failed: {error}",
                outputs.display()
            ))
        })?;
        Ok(outputs.join(format!(
            "video-{}-{}.mp4",
            std::process::id(),
            self.next_artifact_id
        )))
    }
}

impl Drop for SulphurBackend {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SulphurExecutionConfig {
    pub api_version: u32,
    pub runtime_name: String,
    pub runtime_version: String,
    pub backend: String,
    pub distilled: bool,
    pub joint_audio_video: bool,
    pub prompt_enhancer: bool,
    pub ltx_runtime_commit: String,
    pub sulphur_source_commit: String,
    pub distillation_mode: String,
    pub stage_1_denoise_intervals: u64,
    pub stage_2_denoise_intervals: u64,
    pub ffmpeg_version: String,
    pub ffprobe_version: String,
}

fn validate_execution_config(config: &SulphurExecutionConfig, backend: &str) -> Result<()> {
    let expected_mode = match backend {
        "gguf" => "dev_transformer_plus_pinned_distill_lora",
        "mlx" => "native_distilled_artifact",
        _ => "",
    };
    if config.api_version != 1
        || config.backend != backend
        || !config.distilled
        || !config.joint_audio_video
        || config.ltx_runtime_commit != LTX_RUNTIME_COMMIT
        || config.sulphur_source_commit != SULPHUR_SOURCE_COMMIT
        || config.distillation_mode != expected_mode
        || config.stage_1_denoise_intervals != DISTILLED_STAGE_1_INTERVALS
        || config.stage_2_denoise_intervals != DISTILLED_STAGE_2_INTERVALS
        || !config.ffmpeg_version.starts_with("ffmpeg version ")
        || config.ffmpeg_version.len() > 512
        || !config.ffprobe_version.starts_with("ffprobe version ")
        || config.ffprobe_version.len() > 512
        || config.runtime_name.is_empty()
        || config.runtime_name.len() > 256
        || config.runtime_version.is_empty()
        || config.runtime_version.len() > 256
    {
        return Err(EngineError::Sulphur(
            "worker execution evidence does not match the required distilled joint audio-video runtime"
                .to_owned(),
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerLoadInfo {
    n_ctx_train: u32,
    n_vocab: i32,
    worker_pid: u32,
    process_group_id: Option<u32>,
    execution_config: SulphurExecutionConfig,
}

#[derive(Debug, Deserialize)]
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
struct WorkerGenerateResult {
    output_path: PathBuf,
    output_bytes: u64,
    duration_seconds: f64,
    frame_count: u64,
    stage_1_denoise_intervals: u64,
    stage_2_denoise_intervals: u64,
    handled_controls: Vec<String>,
    media_evidence: MediaEvidence,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MediaEvidence {
    video_duration_seconds: f64,
    audio_duration_seconds: f64,
    duration_delta_seconds: f64,
    fps: f64,
    video_packet_count: u64,
    audio_packet_count: u64,
    timestamps_monotonic: bool,
    audio_peak_s16: u64,
    ffprobe_decodable: bool,
    ffmpeg_audio_decodable: bool,
}

#[derive(Clone, Debug, Serialize)]
struct InlineConditioningImage {
    data_base64: String,
    content_type: String,
    crf: u64,
    frame_index: u64,
    strength: f64,
}

#[derive(Clone, Debug)]
struct NormalizedVideoRequest {
    prompt: String,
    width: u64,
    height: u64,
    frame_count: u64,
    fps: f64,
    duration_seconds: f64,
    seed: u64,
    images: Vec<InlineConditioningImage>,
    negative_prompt: String,
    enhance_prompt: bool,
    handled_request_attributes: BTreeSet<String>,
}

impl NormalizedVideoRequest {
    fn from_media_request(request: MediaGenerationRequest) -> Result<Self> {
        request.validate()?;
        if !matches!(
            request.endpoint_family.as_str(),
            ENDPOINT_OPENAI_VIDEOS | ENDPOINT_HF_TEXT_TO_VIDEO
        ) {
            return Err(EngineError::InvalidConfig(format!(
                "Sulphur does not support endpoint family {}",
                request.endpoint_family
            )));
        }
        let body = request.request.as_object().ok_or_else(|| {
            EngineError::InvalidConfig("Sulphur request body must be an object".to_owned())
        })?;
        let mut handled = BTreeSet::new();
        if body.contains_key("model") {
            handled.insert("model".to_owned());
        }
        let prompt = endpoint_prompt(
            body,
            &request.endpoint_family,
            &request.prompt,
            &mut handled,
        )?;
        let parameters = if request.endpoint_family == ENDPOINT_HF_TEXT_TO_VIDEO {
            body.get("parameters")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()
        } else {
            Map::new()
        };
        validate_request_fields(body, &parameters, &request.endpoint_family)?;
        if request.step_count.is_some() {
            return Err(EngineError::InvalidConfig(
                "Sulphur distilled generation does not expose a user-selected step count"
                    .to_owned(),
            ));
        }
        let (width, height) = endpoint_dimensions(body, &parameters, &mut handled)?;
        let fps = numeric_value(body, &parameters, "fps").unwrap_or(DEFAULT_FPS);
        if !fps.is_finite() || !(1.0..=50.0).contains(&fps) {
            return Err(EngineError::InvalidConfig(
                "Sulphur fps must be finite and between 1 and 50".to_owned(),
            ));
        }
        mark_handled_attribute(body, &parameters, "fps", &mut handled);
        let explicit_frames = integer_value(body, &parameters, "num_frames");
        let explicit_duration = requested_duration_value(body, &parameters)?;
        let resolved_frames = request.frame_count;
        if explicit_frames.is_some() && explicit_duration.is_some() {
            return Err(EngineError::InvalidConfig(
                "Sulphur seconds and num_frames are alternative controls".to_owned(),
            ));
        }
        let requested_duration =
            explicit_duration.or(request.duration_seconds.map(|value| value as f64));
        let frame_count = if request.endpoint_family == ENDPOINT_HF_TEXT_TO_VIDEO {
            validate_ltx_frame_count(explicit_frames.or(request.frame_count).ok_or_else(|| {
                EngineError::InvalidConfig(
                    "Sulphur HF request is missing signed normalized num_frames".to_owned(),
                )
            })?)?
        } else if let Some(frames) = explicit_frames {
            validate_ltx_frame_count(frames)?
        } else if let Some(frames) = resolved_frames {
            validate_ltx_frame_count(frames)?
        } else {
            let duration = requested_duration.unwrap_or(4.0);
            nearest_ltx_frame_count(duration * fps)?
        };
        let duration_seconds = frame_count as f64 / fps;
        if duration_seconds <= 0.0 || duration_seconds > 10.0 + 1.0 / fps {
            return Err(EngineError::InvalidConfig(
                "Sulphur video duration must not exceed 10 seconds plus one frame".to_owned(),
            ));
        }
        let duration_to_compare = if explicit_duration.is_some() {
            explicit_duration
        } else if explicit_frames.is_none() {
            request.duration_seconds.map(|value| value as f64)
        } else {
            None
        };
        if let Some(requested) = duration_to_compare {
            let delta = requested - duration_seconds;
            let resolved_frame_tolerance = 8.0 / fps + 1e-6;
            let nearest_frame_tolerance = 4.0 / fps + 1e-6;
            let conflicts = if explicit_frames.is_none() && resolved_frames.is_some() {
                delta < -1e-6 || delta > resolved_frame_tolerance
            } else {
                delta.abs() > nearest_frame_tolerance
            };
            if conflicts {
                return Err(EngineError::InvalidConfig(
                    "Sulphur duration conflicts with num_frames/fps".to_owned(),
                ));
            }
        }
        mark_handled_attribute(body, &parameters, "num_frames", &mut handled);
        if body.contains_key("seconds") {
            handled.insert("seconds".to_owned());
        }

        let seed = integer_value(body, &parameters, "seed").unwrap_or(0);
        if seed > u64::from(u32::MAX) {
            return Err(EngineError::InvalidConfig(
                "Sulphur seed must fit in an unsigned 32-bit integer".to_owned(),
            ));
        }
        mark_handled_attribute(body, &parameters, "seed", &mut handled);
        let images = normalize_conditioning_images(
            body,
            &parameters,
            &request.endpoint_family,
            frame_count,
            &mut handled,
        )?;
        let negative_prompt = negative_prompt_value(body, &parameters, &request.endpoint_family)?;
        if negative_prompt.len() > MAX_PROMPT_BYTES {
            return Err(EngineError::InvalidConfig(
                "Sulphur negative_prompt must contain at most 32768 bytes".to_owned(),
            ));
        }
        mark_handled_attribute(body, &parameters, "negative_prompt", &mut handled);
        require_single_output(body, &parameters, &mut handled)?;
        let enhance_prompt = boolean_value(body, &parameters, "enhance_prompt")?.unwrap_or(false);
        mark_handled_attribute(body, &parameters, "enhance_prompt", &mut handled);
        Ok(Self {
            prompt,
            width,
            height,
            frame_count,
            fps,
            duration_seconds,
            seed,
            images,
            negative_prompt,
            enhance_prompt,
            handled_request_attributes: handled,
        })
    }

    fn worker_payload(&self) -> Value {
        json!({
            "prompt": self.prompt,
            "width": self.width,
            "height": self.height,
            "num_frames": self.frame_count,
            "frame_rate": self.fps,
            "seed": self.seed,
            "images": self.images,
            "negative_prompt": self.negative_prompt,
            "enhance_prompt": false,
        })
    }

    fn handled_controls(&self) -> BTreeSet<String> {
        [
            "prompt",
            "width",
            "height",
            "num_frames",
            "frame_rate",
            "seed",
            "images",
            "negative_prompt",
            "enhance_prompt",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }
}

fn endpoint_prompt(
    body: &Map<String, Value>,
    endpoint: &str,
    outer_prompt: &str,
    handled: &mut BTreeSet<String>,
) -> Result<String> {
    let field = if endpoint == ENDPOINT_HF_TEXT_TO_VIDEO {
        "inputs"
    } else {
        "prompt"
    };
    let prompt = body
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or(outer_prompt)
        .trim()
        .to_owned();
    if prompt.is_empty() || prompt.len() > MAX_PROMPT_BYTES {
        return Err(EngineError::InvalidConfig(
            "Sulphur prompt must contain 1 to 32768 bytes".to_owned(),
        ));
    }
    if !outer_prompt.trim().is_empty() && outer_prompt.trim() != prompt {
        return Err(EngineError::InvalidConfig(
            "Sulphur request prompt conflicts with the normalized prompt".to_owned(),
        ));
    }
    handled.insert(field.to_owned());
    Ok(prompt)
}

fn validate_request_fields(
    body: &Map<String, Value>,
    parameters: &Map<String, Value>,
    endpoint: &str,
) -> Result<()> {
    let top_level = if endpoint == ENDPOINT_HF_TEXT_TO_VIDEO {
        &["inputs", "parameters"][..]
    } else {
        &[
            "model",
            "prompt",
            "input_reference",
            "conditions",
            "negative_prompt",
            "n",
            "size",
            "width",
            "height",
            "seconds",
            "seed",
            "fps",
            "num_frames",
            "enhance_prompt",
        ][..]
    };
    if let Some(field) = body
        .keys()
        .find(|field| !top_level.contains(&field.as_str()))
    {
        return Err(EngineError::InvalidConfig(format!(
            "Sulphur request contains unsupported field {field}"
        )));
    }
    let allowed_parameters = [
        "num_frames",
        "seed",
        "width",
        "height",
        "fps",
        "conditions",
        "negative_prompt",
        "n",
        "enhance_prompt",
    ];
    if let Some(field) = parameters
        .keys()
        .find(|field| !allowed_parameters.contains(&field.as_str()))
    {
        return Err(EngineError::InvalidConfig(format!(
            "Sulphur parameters contain unsupported field {field}"
        )));
    }
    Ok(())
}

fn normalize_conditioning_images(
    body: &Map<String, Value>,
    parameters: &Map<String, Value>,
    endpoint: &str,
    frame_count: u64,
    handled: &mut BTreeSet<String>,
) -> Result<Vec<InlineConditioningImage>> {
    let input_reference = body.get("input_reference").filter(|value| !value.is_null());
    let conditions = parameters
        .get("conditions")
        .or_else(|| body.get("conditions"))
        .filter(|value| !value.is_null());
    if input_reference.is_some() && conditions.is_some() {
        return Err(EngineError::InvalidConfig(
            "Sulphur input_reference and conditions are alternative controls".to_owned(),
        ));
    }
    if let Some(value) = input_reference {
        if endpoint != ENDPOINT_OPENAI_VIDEOS {
            return Err(EngineError::InvalidConfig(
                "Sulphur input_reference is only exposed by the OpenAI video endpoint".to_owned(),
            ));
        }
        let data_url = match value {
            Value::String(value) => value.as_str(),
            Value::Object(value) if value.len() == 1 => value
                .get("image_url")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    EngineError::InvalidConfig(
                        "Sulphur input_reference object must contain only image_url".to_owned(),
                    )
                })?,
            _ => {
                return Err(EngineError::InvalidConfig(
                    "Sulphur input_reference must be a bounded inline image data URL".to_owned(),
                ))
            }
        };
        let (content_type, data_base64, _) =
            decode_conditioning_image(data_url, "input_reference")?;
        handled.insert("input_reference".to_owned());
        return Ok(vec![InlineConditioningImage {
            data_base64,
            content_type,
            crf: DEFAULT_CONDITIONING_CRF,
            frame_index: 0,
            strength: 1.0,
        }]);
    }
    let Some(value) = conditions else {
        return Ok(Vec::new());
    };
    let entries = value.as_array().ok_or_else(|| {
        EngineError::InvalidConfig("Sulphur conditions must be an ordered array".to_owned())
    })?;
    if entries.len() > MAX_CONDITIONING_IMAGES {
        return Err(EngineError::InvalidConfig(format!(
            "Sulphur conditions must contain at most {MAX_CONDITIONING_IMAGES} images"
        )));
    }
    let mut total_bytes = 0_usize;
    let mut normalized = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let label = format!("condition {index}");
        let object = entry.as_object().ok_or_else(|| {
            EngineError::InvalidConfig(format!("Sulphur {label} must be an object"))
        })?;
        let allowed = ["image_url", "frame_index", "strength", "crf"];
        if let Some(field) = object
            .keys()
            .find(|field| !allowed.contains(&field.as_str()))
        {
            return Err(EngineError::InvalidConfig(format!(
                "Sulphur {label} contains unsupported field {field}"
            )));
        }
        let data_url = object
            .get("image_url")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                EngineError::InvalidConfig(format!(
                    "Sulphur {label} must contain an inline image_url"
                ))
            })?;
        let frame_index = match object.get("frame_index") {
            Some(value) => value.as_u64().ok_or_else(|| {
                EngineError::InvalidConfig(format!(
                    "Sulphur {label} frame_index must be an unsigned integer"
                ))
            })?,
            None => 0,
        };
        if frame_index >= frame_count {
            return Err(EngineError::InvalidConfig(format!(
                "Sulphur {label} frame_index must identify an output frame"
            )));
        }
        let strength = match object.get("strength") {
            Some(value) => value.as_f64().ok_or_else(|| {
                EngineError::InvalidConfig(format!(
                    "Sulphur {label} strength must be a finite number"
                ))
            })?,
            None => 1.0,
        };
        if !strength.is_finite() || !(0.0..=1.0).contains(&strength) {
            return Err(EngineError::InvalidConfig(format!(
                "Sulphur {label} strength must be between 0 and 1"
            )));
        }
        let crf = match object.get("crf") {
            Some(value) => value.as_u64().ok_or_else(|| {
                EngineError::InvalidConfig(format!(
                    "Sulphur {label} crf must be an unsigned integer"
                ))
            })?,
            None => DEFAULT_CONDITIONING_CRF,
        };
        if crf > 51 {
            return Err(EngineError::InvalidConfig(format!(
                "Sulphur {label} crf must be between 0 and 51"
            )));
        }
        let (content_type, data_base64, decoded_bytes) =
            decode_conditioning_image(data_url, &label)?;
        total_bytes = total_bytes.checked_add(decoded_bytes).ok_or_else(|| {
            EngineError::InvalidConfig("Sulphur conditions exceed their aggregate bound".to_owned())
        })?;
        if total_bytes > MAX_CONDITIONING_TOTAL_BYTES {
            return Err(EngineError::InvalidConfig(format!(
                "Sulphur conditions must contain at most {MAX_CONDITIONING_TOTAL_BYTES} decoded bytes in total"
            )));
        }
        normalized.push(InlineConditioningImage {
            data_base64,
            content_type,
            crf,
            frame_index,
            strength,
        });
    }
    handled.insert(if endpoint == ENDPOINT_HF_TEXT_TO_VIDEO {
        "parameters.conditions".to_owned()
    } else {
        "conditions".to_owned()
    });
    Ok(normalized)
}

fn decode_conditioning_image(data_url: &str, label: &str) -> Result<(String, String, usize)> {
    let rest = data_url.strip_prefix("data:").ok_or_else(|| {
        EngineError::InvalidConfig(format!(
            "Sulphur {label} cannot fetch a URL or read a filesystem path"
        ))
    })?;
    let (metadata, encoded) = rest.split_once(',').ok_or_else(|| {
        EngineError::InvalidConfig(format!("Sulphur {label} data URL is missing its payload"))
    })?;
    let mut metadata = metadata.split(';');
    let content_type = metadata.next().unwrap_or_default().to_ascii_lowercase();
    if metadata.next() != Some("base64") || metadata.next().is_some() {
        return Err(EngineError::InvalidConfig(format!(
            "Sulphur {label} must use an exact base64 image data URL"
        )));
    }
    if !matches!(content_type.as_str(), "image/png" | "image/jpeg") {
        return Err(EngineError::InvalidConfig(format!(
            "Sulphur {label} must be a PNG or JPEG image"
        )));
    }
    let encoded_limit = MAX_CONDITIONING_IMAGE_BYTES
        .saturating_mul(4)
        .div_ceil(3)
        .saturating_add(4);
    if encoded.is_empty() || encoded.len() > encoded_limit {
        return Err(EngineError::InvalidConfig(format!(
            "Sulphur {label} must contain 1 to {MAX_CONDITIONING_IMAGE_BYTES} decoded bytes"
        )));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| {
            EngineError::InvalidConfig(format!("Sulphur {label} contains invalid base64"))
        })?;
    if bytes.is_empty()
        || bytes.len() > MAX_CONDITIONING_IMAGE_BYTES
        || !conditioning_image_signature_matches(&bytes, &content_type)
    {
        return Err(EngineError::InvalidConfig(format!(
            "Sulphur {label} bytes do not match the declared bounded image"
        )));
    }
    let decoded_bytes = bytes.len();
    Ok((
        content_type,
        base64::engine::general_purpose::STANDARD.encode(bytes),
        decoded_bytes,
    ))
}

fn conditioning_image_signature_matches(bytes: &[u8], content_type: &str) -> bool {
    match content_type {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        _ => false,
    }
}

fn endpoint_dimensions(
    body: &Map<String, Value>,
    parameters: &Map<String, Value>,
    handled: &mut BTreeSet<String>,
) -> Result<(u64, u64)> {
    let explicit_width = integer_value(body, parameters, "width");
    let explicit_height = integer_value(body, parameters, "height");
    let (width, height) = match (explicit_width, explicit_height) {
        (Some(width), Some(height)) => {
            mark_handled_attribute(body, parameters, "width", handled);
            mark_handled_attribute(body, parameters, "height", handled);
            (width, height)
        }
        (None, None) => match body.get("size").and_then(Value::as_str) {
            Some(size) => {
                let (width, height) = size.split_once('x').ok_or_else(|| {
                    EngineError::InvalidConfig("Sulphur size must be WIDTHxHEIGHT".to_owned())
                })?;
                let width = width.parse::<u64>().map_err(|_| {
                    EngineError::InvalidConfig("Sulphur size width is invalid".to_owned())
                })?;
                let height = height.parse::<u64>().map_err(|_| {
                    EngineError::InvalidConfig("Sulphur size height is invalid".to_owned())
                })?;
                handled.insert("size".to_owned());
                (width, height)
            }
            None => (DEFAULT_WIDTH, DEFAULT_HEIGHT),
        },
        _ => {
            return Err(EngineError::InvalidConfig(
                "Sulphur width and height must be supplied together".to_owned(),
            ))
        }
    };
    if !(256..=2048).contains(&width)
        || !(256..=2048).contains(&height)
        || width % 64 != 0
        || height % 64 != 0
    {
        return Err(EngineError::InvalidConfig(
            "Sulphur width and height must be multiples of 64 between 256 and 2048".to_owned(),
        ));
    }
    Ok((width, height))
}

fn mark_handled_attribute(
    body: &Map<String, Value>,
    parameters: &Map<String, Value>,
    key: &str,
    handled: &mut BTreeSet<String>,
) {
    if parameters.contains_key(key) {
        handled.insert(format!("parameters.{key}"));
    } else if body.contains_key(key) {
        handled.insert(key.to_owned());
    }
}

fn integer_value(
    body: &Map<String, Value>,
    parameters: &Map<String, Value>,
    key: &str,
) -> Option<u64> {
    parameters
        .get(key)
        .or_else(|| body.get(key))
        .and_then(Value::as_u64)
}

fn numeric_value(
    body: &Map<String, Value>,
    parameters: &Map<String, Value>,
    key: &str,
) -> Option<f64> {
    parameters
        .get(key)
        .or_else(|| body.get(key))
        .and_then(Value::as_f64)
}

fn requested_duration_value(
    body: &Map<String, Value>,
    parameters: &Map<String, Value>,
) -> Result<Option<f64>> {
    let Some((field, value)) = parameters
        .get("duration_seconds")
        .map(|value| ("duration_seconds", value))
        .or_else(|| {
            body.get("duration_seconds")
                .map(|value| ("duration_seconds", value))
        })
        .or_else(|| body.get("seconds").map(|value| ("seconds", value)))
    else {
        return Ok(None);
    };
    let duration = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok()))
        .ok_or_else(|| EngineError::InvalidConfig(format!("Sulphur {field} must be a number")))?;
    if !duration.is_finite() || duration <= 0.0 {
        return Err(EngineError::InvalidConfig(format!(
            "Sulphur {field} must be a positive finite number"
        )));
    }
    Ok(Some(duration))
}

fn boolean_value(
    body: &Map<String, Value>,
    parameters: &Map<String, Value>,
    key: &str,
) -> Result<Option<bool>> {
    parameters
        .get(key)
        .or_else(|| body.get(key))
        .map(|value| {
            value.as_bool().ok_or_else(|| {
                EngineError::InvalidConfig(format!("Sulphur {key} must be a boolean"))
            })
        })
        .transpose()
}

fn negative_prompt_value(
    body: &Map<String, Value>,
    parameters: &Map<String, Value>,
    endpoint: &str,
) -> Result<String> {
    let Some(value) = parameters
        .get("negative_prompt")
        .or_else(|| body.get("negative_prompt"))
    else {
        return Ok(String::new());
    };
    if let Some(value) = value.as_str() {
        return Ok(value.to_owned());
    }
    if endpoint == ENDPOINT_HF_TEXT_TO_VIDEO {
        if let Some(values) = value.as_array() {
            if values.len() == 1 {
                if let Some(value) = values[0].as_str() {
                    return Ok(value.to_owned());
                }
            }
        }
    }
    Err(EngineError::InvalidConfig(
        "Sulphur negative_prompt must be text or, on the Hugging Face endpoint, a singleton text array"
            .to_owned(),
    ))
}

fn require_single_output(
    body: &Map<String, Value>,
    parameters: &Map<String, Value>,
    handled: &mut BTreeSet<String>,
) -> Result<()> {
    let Some(value) = parameters.get("n").or_else(|| body.get("n")) else {
        return Ok(());
    };
    if value.as_u64() != Some(1) {
        return Err(EngineError::InvalidConfig(
            "Sulphur n must be exactly 1".to_owned(),
        ));
    }
    mark_handled_attribute(body, parameters, "n", handled);
    Ok(())
}

fn nearest_ltx_frame_count(raw: f64) -> Result<u64> {
    if !raw.is_finite() || raw <= 0.0 {
        return Err(EngineError::InvalidConfig(
            "Sulphur duration/fps produced an invalid frame count".to_owned(),
        ));
    }
    let rounded = raw.round().clamp(1.0, 513.0) as u64;
    let lower = rounded.saturating_sub(1) / 8 * 8 + 1;
    let upper = lower.saturating_add(8).min(513);
    let selected = if rounded.saturating_sub(lower) <= upper.saturating_sub(rounded) {
        lower
    } else {
        upper
    };
    Ok(selected.max(1))
}

fn validate_ltx_frame_count(frames: u64) -> Result<u64> {
    if frames == 0 || frames > 513 || (frames - 1) % 8 != 0 {
        return Err(EngineError::InvalidConfig(
            "Sulphur num_frames must be an 8k+1 value between 1 and 513".to_owned(),
        ));
    }
    Ok(frames)
}

fn validate_worker_result(
    response: &WorkerGenerateResult,
    request: &NormalizedVideoRequest,
    expected_controls: &BTreeSet<String>,
    expected_path: &Path,
) -> Result<()> {
    if response.output_path != expected_path {
        return Err(EngineError::Sulphur(
            "Sulphur worker returned an unexpected output path".to_owned(),
        ));
    }
    if response.output_bytes == 0 || response.output_bytes > MAX_OUTPUT_BYTES {
        return Err(EngineError::Sulphur(
            "Sulphur worker returned an out-of-bounds artifact size".to_owned(),
        ));
    }
    let metadata = fs::symlink_metadata(expected_path).map_err(|error| {
        EngineError::Sulphur(format!("reading generated Sulphur MP4 failed: {error}"))
    })?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() != response.output_bytes
    {
        return Err(EngineError::Sulphur(
            "Sulphur worker output is not the declared real regular file".to_owned(),
        ));
    }
    if response.frame_count != request.frame_count
        || response.stage_1_denoise_intervals != DISTILLED_STAGE_1_INTERVALS
        || response.stage_2_denoise_intervals != DISTILLED_STAGE_2_INTERVALS
        || !response.duration_seconds.is_finite()
        || response.duration_seconds <= 0.0
    {
        return Err(EngineError::Sulphur(
            "Sulphur worker returned generation metadata that differs from the request".to_owned(),
        ));
    }
    let controls = response
        .handled_controls
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if controls.len() != response.handled_controls.len() || controls != *expected_controls {
        return Err(EngineError::Sulphur(
            "Sulphur runtime did not explicitly handle every generation control".to_owned(),
        ));
    }
    validate_media_evidence(&response.media_evidence, response, request)?;
    Ok(())
}

fn validate_media_evidence(
    evidence: &MediaEvidence,
    response: &WorkerGenerateResult,
    request: &NormalizedVideoRequest,
) -> Result<()> {
    let one_frame = 1.0 / request.fps;
    let measured_delta = (evidence.video_duration_seconds - evidence.audio_duration_seconds).abs();
    if !evidence.video_duration_seconds.is_finite()
        || !evidence.audio_duration_seconds.is_finite()
        || !evidence.duration_delta_seconds.is_finite()
        || evidence.video_duration_seconds <= 0.0
        || evidence.audio_duration_seconds <= 0.0
        || (evidence.fps - request.fps).abs() > 1e-6
        || evidence.video_packet_count != request.frame_count
        || evidence.audio_packet_count == 0
        || !evidence.timestamps_monotonic
        || evidence.audio_peak_s16 <= 1
        || !evidence.ffprobe_decodable
        || !evidence.ffmpeg_audio_decodable
        || measured_delta > one_frame + 1e-6
        || (measured_delta - evidence.duration_delta_seconds).abs() > 1e-6
        || (evidence.video_duration_seconds - response.duration_seconds).abs() > one_frame + 1e-6
    {
        return Err(EngineError::Sulphur(
            "Sulphur worker did not return valid synchronized, decodable, non-silent A/V evidence"
                .to_owned(),
        ));
    }
    Ok(())
}

struct OutputCleanup(PathBuf);

impl Drop for OutputCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn stream_artifact(
    path: &Path,
    artifact_id: String,
    sink: &mut dyn ArtifactSink,
    cancellation: &CancellationToken,
) -> Result<()> {
    let size = fs::metadata(path)?.len();
    if size == 0 || size > MAX_OUTPUT_BYTES {
        return Err(EngineError::Sulphur(
            "generated Sulphur MP4 is outside the output bound".to_owned(),
        ));
    }
    let mut file = File::open(path)?;
    let mut remaining = size;
    let mut index = 0_u32;
    while remaining > 0 {
        cancellation.check()?;
        let read_len = usize::try_from(remaining.min(ARTIFACT_CHUNK_BYTES as u64))
            .unwrap_or(ARTIFACT_CHUNK_BYTES);
        let mut bytes = vec![0_u8; read_len];
        file.read_exact(&mut bytes)?;
        remaining = remaining.saturating_sub(read_len as u64);
        sink.on_artifact_chunk(ArtifactChunk {
            artifact_id: artifact_id.clone(),
            index,
            content_type: "video/mp4".to_owned(),
            bytes,
            final_chunk: remaining == 0,
        })?;
        index = index.saturating_add(1);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct Mp4Track {
    kind: [u8; 4],
    duration_seconds: f64,
}

#[derive(Debug)]
struct Mp4Info {
    video: Mp4Track,
    audio: Mp4Track,
}

#[derive(Clone, Copy, Debug)]
struct IsoBox {
    kind: [u8; 4],
    payload_start: u64,
    end: u64,
}

fn inspect_joint_mp4(path: &Path) -> Result<Mp4Info> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let boxes = read_iso_boxes(&mut file, 0, file_len)?;
    if !boxes.iter().any(|item| &item.kind == b"ftyp")
        || !boxes
            .iter()
            .any(|item| &item.kind == b"mdat" && item.end > item.payload_start)
    {
        return Err(EngineError::Sulphur(
            "Sulphur output is not a bounded MP4 with ftyp and media data".to_owned(),
        ));
    }
    let moov = boxes
        .iter()
        .find(|item| &item.kind == b"moov")
        .ok_or_else(|| {
            EngineError::Sulphur("Sulphur MP4 is missing its movie metadata".to_owned())
        })?;
    let moov_boxes = read_iso_boxes(&mut file, moov.payload_start, moov.end)?;
    let mvhd = moov_boxes
        .iter()
        .find(|item| &item.kind == b"mvhd")
        .ok_or_else(|| {
            EngineError::Sulphur("Sulphur MP4 is missing its movie timebase".to_owned())
        })?;
    let movie_timescale = read_mp4_movie_timescale(&mut file, *mvhd)?;
    let mut tracks = Vec::new();
    for item in moov_boxes {
        if &item.kind == b"trak" {
            tracks.push(read_mp4_track(&mut file, item, movie_timescale)?);
        }
    }
    let video = tracks
        .iter()
        .copied()
        .find(|track| &track.kind == b"vide")
        .ok_or_else(|| EngineError::Sulphur("Sulphur MP4 has no video track".to_owned()))?;
    let audio = tracks
        .iter()
        .copied()
        .find(|track| &track.kind == b"soun")
        .ok_or_else(|| EngineError::Sulphur("Sulphur MP4 has no audio track".to_owned()))?;
    Ok(Mp4Info { video, audio })
}

fn read_mp4_track(file: &mut File, track: IsoBox, movie_timescale: u32) -> Result<Mp4Track> {
    let track_boxes = read_iso_boxes(file, track.payload_start, track.end)?;
    let tkhd = track_boxes
        .iter()
        .find(|item| &item.kind == b"tkhd")
        .ok_or_else(|| {
            EngineError::Sulphur("Sulphur MP4 track has no presentation duration".to_owned())
        })?;
    let duration_seconds = read_mp4_track_duration(file, *tkhd, movie_timescale)?;
    let mdia = track_boxes
        .into_iter()
        .find(|item| &item.kind == b"mdia")
        .ok_or_else(|| EngineError::Sulphur("Sulphur MP4 track has no media box".to_owned()))?;
    let boxes = read_iso_boxes(file, mdia.payload_start, mdia.end)?;
    let hdlr = boxes
        .iter()
        .find(|item| &item.kind == b"hdlr")
        .ok_or_else(|| EngineError::Sulphur("Sulphur MP4 track has no handler".to_owned()))?;
    if hdlr.end.saturating_sub(hdlr.payload_start) < 12 {
        return Err(EngineError::Sulphur(
            "Sulphur MP4 handler is truncated".to_owned(),
        ));
    }
    file.seek(SeekFrom::Start(hdlr.payload_start + 8))?;
    let mut kind = [0_u8; 4];
    file.read_exact(&mut kind)?;
    Ok(Mp4Track {
        kind,
        duration_seconds,
    })
}

fn read_mp4_movie_timescale(file: &mut File, mvhd: IsoBox) -> Result<u32> {
    file.seek(SeekFrom::Start(mvhd.payload_start))?;
    let mut header = [0_u8; 4];
    file.read_exact(&mut header)?;
    let timescale_offset = match header[0] {
        0 => 12,
        1 => 20,
        _ => {
            return Err(EngineError::Sulphur(
                "Sulphur MP4 movie header version is unsupported".to_owned(),
            ))
        }
    };
    if mvhd.end.saturating_sub(mvhd.payload_start) < timescale_offset + 4 {
        return Err(EngineError::Sulphur(
            "Sulphur MP4 movie timebase is truncated".to_owned(),
        ));
    }
    file.seek(SeekFrom::Start(mvhd.payload_start + timescale_offset))?;
    let timescale = read_u32(file)?;
    if timescale == 0 {
        return Err(EngineError::Sulphur(
            "Sulphur MP4 movie timebase is invalid".to_owned(),
        ));
    }
    Ok(timescale)
}

fn read_mp4_track_duration(file: &mut File, tkhd: IsoBox, movie_timescale: u32) -> Result<f64> {
    file.seek(SeekFrom::Start(tkhd.payload_start))?;
    let mut header = [0_u8; 4];
    file.read_exact(&mut header)?;
    let (duration_offset, duration_bytes) = match header[0] {
        0 => (20, 4),
        1 => (28, 8),
        _ => {
            return Err(EngineError::Sulphur(
                "Sulphur MP4 track header version is unsupported".to_owned(),
            ))
        }
    };
    if tkhd.end.saturating_sub(tkhd.payload_start) < duration_offset + duration_bytes {
        return Err(EngineError::Sulphur(
            "Sulphur MP4 track duration is truncated".to_owned(),
        ));
    }
    file.seek(SeekFrom::Start(tkhd.payload_start + duration_offset))?;
    let duration = if duration_bytes == 4 {
        u64::from(read_u32(file)?)
    } else {
        read_u64(file)?
    };
    if movie_timescale == 0 || duration == 0 {
        return Err(EngineError::Sulphur(
            "Sulphur MP4 track duration is invalid".to_owned(),
        ));
    }
    Ok(duration as f64 / f64::from(movie_timescale))
}

fn read_iso_boxes(file: &mut File, start: u64, end: u64) -> Result<Vec<IsoBox>> {
    let mut boxes = Vec::new();
    let mut cursor = start;
    while cursor < end {
        if end.saturating_sub(cursor) < 8 {
            return Err(EngineError::Sulphur(
                "Sulphur MP4 contains a truncated box header".to_owned(),
            ));
        }
        file.seek(SeekFrom::Start(cursor))?;
        let size32 = read_u32(file)?;
        let mut kind = [0_u8; 4];
        file.read_exact(&mut kind)?;
        let (size, header_size) = match size32 {
            0 => (end.saturating_sub(cursor), 8),
            1 => (read_u64(file)?, 16),
            value => (u64::from(value), 8),
        };
        if size < header_size || cursor.saturating_add(size) > end {
            return Err(EngineError::Sulphur(
                "Sulphur MP4 contains an out-of-bounds box".to_owned(),
            ));
        }
        boxes.push(IsoBox {
            kind,
            payload_start: cursor + header_size,
            end: cursor + size,
        });
        cursor += size;
    }
    Ok(boxes)
}

fn read_u32(file: &mut File) -> Result<u32> {
    let mut bytes = [0_u8; 4];
    file.read_exact(&mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_u64(file: &mut File) -> Result<u64> {
    let mut bytes = [0_u8; 8];
    file.read_exact(&mut bytes)?;
    Ok(u64::from_be_bytes(bytes))
}

fn validate_mp4_timing(
    mp4: &Mp4Info,
    response: &WorkerGenerateResult,
    request: &NormalizedVideoRequest,
) -> Result<()> {
    let tolerance = 1.0 / request.fps + 1e-6;
    if (mp4.video.duration_seconds - mp4.audio.duration_seconds).abs() > tolerance {
        return Err(EngineError::Sulphur(
            "Sulphur MP4 audio and video tracks are not synchronized".to_owned(),
        ));
    }
    if (mp4.video.duration_seconds - response.duration_seconds).abs() > tolerance
        || (response.duration_seconds - request.duration_seconds).abs() > tolerance
    {
        return Err(EngineError::Sulphur(
            "Sulphur MP4 duration differs from generated metadata".to_owned(),
        ));
    }
    Ok(())
}

struct SulphurWorker {
    child: SandboxedChild,
    worker_pid: Option<u32>,
    stdin: SandboxedChildStdin,
    stdout_rx: Option<Receiver<WorkerRead>>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    stderr_reader: Option<JoinHandle<()>>,
}

impl SulphurWorker {
    fn spawn(
        python: &Path,
        ffmpeg: &Path,
        ffprobe: &Path,
        memory_limit_bytes: Option<u64>,
        cache_root: &Path,
        model_root: &Path,
    ) -> Result<Self> {
        let (python, runtime_roots) = python_runtime_roots(python)?;
        let ffmpeg = resolve_program(ffmpeg, "ffmpeg")?;
        let ffprobe = resolve_program(ffprobe, "ffprobe")?;
        let mut executable_roots = runtime_roots.clone();
        for executable in [&ffmpeg, &ffprobe] {
            let root = executable.parent().ok_or_else(|| {
                EngineError::Sulphur(format!(
                    "Sulphur executable {} has no parent directory",
                    executable.display()
                ))
            })?;
            insert_non_overlapping_root(&mut executable_roots, root.to_path_buf());
        }
        let mut read_only_dirs = vec![model_root.to_path_buf()];
        read_only_dirs.extend(executable_roots.iter().cloned());
        let mut sandbox = SandboxConfig::new(read_only_dirs, vec![cache_root.to_path_buf()]);
        sandbox.materialized_read_only_dir(model_root);
        let mut command = SandboxedCommand::new(&python);
        if let Some(limit) = memory_limit_bytes {
            command.memory_limit_bytes(limit);
        }
        configure_worker_environment(&mut command, &python, &ffmpeg, &ffprobe, cache_root)?;
        for root in executable_roots {
            command.executable_read_only_dir(root);
        }
        command
            .allow_code_generation()
            .current_dir(cache_root)
            .stderr(SandboxedStderr::Piped)
            .arg("-I")
            .arg("-u")
            .arg("-c")
            .arg(WORKER_STDIN_BOOTSTRAP);
        let mut child = command.spawn(&sandbox).map_err(|error| {
            EngineError::Sulphur(format!(
                "starting sandboxed Sulphur worker with {} failed: {error}",
                python.display()
            ))
        })?;
        let mut stdin = child.take_stdin().ok_or_else(|| {
            EngineError::Sulphur("opening Sulphur worker stdin failed".to_owned())
        })?;
        let worker_source = base64::engine::general_purpose::STANDARD.encode(WORKER.as_bytes());
        stdin
            .write_all(worker_source.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .map_err(|error| {
                EngineError::Sulphur(format!(
                    "sending Sulphur worker source over the private bootstrap pipe failed: {error}"
                ))
            })?;
        let stdout = child.take_stdout().ok_or_else(|| {
            EngineError::Sulphur("opening Sulphur worker stdout failed".to_owned())
        })?;
        let stderr = child.take_stderr().ok_or_else(|| {
            EngineError::Sulphur("opening Sulphur worker stderr failed".to_owned())
        })?;
        let (stdout_tx, stdout_rx) = mpsc::sync_channel(super::WORKER_STDOUT_QUEUE_CAPACITY);
        let stdout_reader = thread::spawn(move || read_worker_stdout(stdout, stdout_tx));
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&stderr_tail);
        let stderr_reader = thread::spawn(move || read_worker_stderr(stderr, capture));
        Ok(Self {
            child,
            worker_pid: None,
            stdin,
            stdout_rx: Some(stdout_rx),
            stdout_reader: Some(stdout_reader),
            stderr_tail,
            stderr_reader: Some(stderr_reader),
        })
    }

    fn send(&mut self, id: u64, operation: &str, payload: Value) -> Result<()> {
        serde_json::to_writer(
            &mut self.stdin,
            &json!({"id": id, "op": operation, "payload": payload}),
        )?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn read_message(&mut self, wait: Duration) -> Result<Option<WorkerMessage>> {
        let read = match self
            .stdout_rx
            .as_ref()
            .ok_or_else(|| {
                EngineError::Sulphur("Sulphur worker stdout reader is closed".to_owned())
            })?
            .recv_timeout(wait)
        {
            Ok(read) => read,
            Err(RecvTimeoutError::Timeout) => return Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                return Err(EngineError::Sulphur(
                    "Sulphur worker stdout reader stopped".to_owned(),
                ))
            }
        };
        let line = match read {
            WorkerRead::Line(line) => line,
            WorkerRead::Eof => return Err(self.exit_error("Sulphur worker exited before replying")),
            WorkerRead::Error(error) => return Err(EngineError::Sulphur(error)),
        };
        serde_json::from_str(line.trim_end())
            .map(Some)
            .map_err(|error| {
                EngineError::Sulphur(format!(
                    "decoding Sulphur worker protocol line failed: {error}"
                ))
            })
    }

    fn stop(&mut self) {
        let _ = self.send(0, "shutdown", Value::Null);
        if let Some(pid) = self.worker_pid {
            terminate_process_tree(pid);
        }
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
            EngineError::Sulphur(format!("{message}; exit status {status}; stderr was empty"))
        } else {
            EngineError::Sulphur(format!(
                "{message}; exit status {status}; stderr tail: {stderr}"
            ))
        }
    }
}

#[cfg(unix)]
fn terminate_process_tree(pid: u32) {
    let kill = if Path::new("/bin/kill").is_file() {
        "/bin/kill"
    } else {
        "kill"
    };
    let group = format!("-{pid}");
    let _ = std::process::Command::new(kill)
        .args(["-TERM", "--", &group])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    thread::sleep(Duration::from_millis(100));
    let _ = std::process::Command::new(kill)
        .args(["-KILL", "--", &group])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(windows)]
fn terminate_process_tree(pid: u32) {
    let _ = std::process::Command::new("taskkill.exe")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(not(any(unix, windows)))]
fn terminate_process_tree(_pid: u32) {}

enum WorkerRead {
    Line(String),
    Eof,
    Error(String),
}

fn read_worker_stdout(stdout: SandboxedChildStdout, sender: mpsc::SyncSender<WorkerRead>) {
    let mut stdout = BufReader::new(stdout);
    loop {
        let mut line = String::new();
        match stdout.read_line(&mut line) {
            Ok(0) => {
                let _ = sender.send(WorkerRead::Eof);
                return;
            }
            Ok(_) => {
                if sender.send(WorkerRead::Line(line)).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(WorkerRead::Error(format!(
                    "reading Sulphur worker stdout failed: {error}"
                )));
                return;
            }
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
        .unwrap_or_else(|_| "<stderr capture unavailable>".to_owned())
}

fn validate_module_name(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || value.split('.').any(|part| {
            part.is_empty()
                || !part.bytes().enumerate().all(|(index, byte)| {
                    byte == b'_'
                        || byte.is_ascii_alphanumeric() && (index > 0 || !byte.is_ascii_digit())
                })
        })
    {
        return Err(EngineError::InvalidConfig(
            "Sulphur runtime module must be a safe dotted Python identifier".to_owned(),
        ));
    }
    Ok(())
}

fn sulphur_cache_root(configured: Option<&Path>) -> PathBuf {
    configured
        .map(Path::to_path_buf)
        .or_else(|| {
            env::var_os("MAYHEM_HOME")
                .map(PathBuf::from)
                .map(|home| home.join("cache/sulphur"))
        })
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".mayhem/cache/sulphur"))
        })
        .unwrap_or_else(|| env::temp_dir().join("mayhem-sulphur-cache"))
}

fn sulphur_worker_cache(cache_root: &Path, model_root: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(model_root).map_err(|error| {
        EngineError::Sulphur(format!(
            "canonicalizing Sulphur model root {} failed: {error}",
            model_root.display()
        ))
    })?;
    let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
    Ok(cache_root.join("workers").join(format!("{digest:x}")))
}

fn python_runtime_roots(python: &Path) -> Result<(PathBuf, Vec<PathBuf>)> {
    let python = resolve_python_program(python)?;
    let canonical_python = fs::canonicalize(&python).map_err(|error| {
        EngineError::Sulphur(format!(
            "canonicalizing Sulphur Python {} failed: {error}",
            python.display()
        ))
    })?;
    let executable_root = canonical_python.parent().ok_or_else(|| {
        EngineError::Sulphur(format!(
            "Sulphur Python {} has no runtime directory",
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
        let candidate = fs::canonicalize(candidate)?;
        insert_non_overlapping_root(&mut roots, candidate);
    }
    if roots.is_empty() {
        return Err(EngineError::Sulphur(
            "Sulphur Python has no readable runtime roots".to_owned(),
        ));
    }
    Ok((python, roots))
}

fn insert_non_overlapping_root(roots: &mut Vec<PathBuf>, candidate: PathBuf) {
    if roots.iter().any(|root| candidate.starts_with(root)) {
        return;
    }
    roots.retain(|root| !root.starts_with(&candidate));
    roots.push(candidate);
}

fn resolve_python_program(python: &Path) -> Result<PathBuf> {
    resolve_program_entry(python, "Python")
}

fn resolve_program(program: &Path, label: &str) -> Result<PathBuf> {
    let resolved = resolve_program_entry(program, label)?;
    fs::canonicalize(&resolved).map_err(|error| {
        EngineError::Sulphur(format!(
            "canonicalizing Sulphur {label} executable {} failed: {error}",
            resolved.display()
        ))
    })
}

fn resolve_program_entry(program: &Path, label: &str) -> Result<PathBuf> {
    let candidates = if program.is_absolute() || program.components().count() > 1 {
        vec![if program.is_absolute() {
            program.to_path_buf()
        } else {
            env::current_dir()?.join(program)
        }]
    } else {
        env::var_os("PATH")
            .into_iter()
            .flat_map(|value| env::split_paths(&value).collect::<Vec<_>>())
            .map(|directory| directory.join(program))
            .collect()
    };
    let resolved = candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            EngineError::Sulphur(format!(
                "Sulphur {label} executable {} was not found",
                program.display()
            ))
        })?;
    Ok(resolved)
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

fn configure_worker_environment(
    command: &mut SandboxedCommand,
    python: &Path,
    ffmpeg: &Path,
    ffprobe: &Path,
    cache_root: &Path,
) -> Result<()> {
    for (name, path) in [
        ("HOME", cache_root.join("home")),
        ("TMPDIR", cache_root.join("tmp")),
        ("TMP", cache_root.join("tmp")),
        ("TEMP", cache_root.join("tmp")),
        ("XDG_CACHE_HOME", cache_root.join("xdg")),
        ("XDG_RUNTIME_DIR", cache_root.join("xdg-runtime")),
        ("HF_HOME", cache_root.join("huggingface")),
        ("HF_HUB_CACHE", cache_root.join("huggingface/hub")),
        ("TRANSFORMERS_CACHE", cache_root.join("transformers")),
        ("TORCH_HOME", cache_root.join("torch")),
    ] {
        fs::create_dir_all(&path)?;
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
    ] {
        command.env_remove(name);
    }
    command.env("PATH", sulphur_worker_path(python, ffmpeg, ffprobe)?);
    Ok(())
}

fn sulphur_worker_path(python: &Path, ffmpeg: &Path, ffprobe: &Path) -> Result<OsString> {
    let mut paths = Vec::new();
    for executable in [python, ffmpeg, ffprobe] {
        let parent = executable.parent().ok_or_else(|| {
            EngineError::Sulphur(format!(
                "Sulphur executable {} has no parent directory",
                executable.display()
            ))
        })?;
        if !paths.iter().any(|candidate| candidate == parent) {
            paths.push(parent.to_path_buf());
        }
    }
    #[cfg(unix)]
    paths.extend([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
    #[cfg(windows)]
    if let Some(system_root) = env::var_os("SystemRoot") {
        paths.push(PathBuf::from(system_root).join("System32"));
    }
    env::join_paths(paths).map_err(|error| {
        EngineError::Sulphur(format!(
            "constructing the bounded Sulphur worker PATH failed: {error}"
        ))
    })
}

#[derive(Debug)]
struct SulphurMediaTools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

fn preflight_media_tools(ffmpeg: &Path, ffprobe: &Path) -> Result<SulphurMediaTools> {
    let ffmpeg = resolve_media_tool(ffmpeg, "ffmpeg", FFMPEG_ENV)?;
    let ffprobe = resolve_media_tool(ffprobe, "ffprobe", FFPROBE_ENV)?;

    let ffmpeg_version = run_media_tool_probe(&ffmpeg, &["-version"], "ffmpeg")?;
    require_media_tool_version(&ffmpeg_version, "ffmpeg", FFMPEG_ENV)?;
    let ffprobe_version = run_media_tool_probe(&ffprobe, &["-version"], "ffprobe")?;
    require_media_tool_version(&ffprobe_version, "ffprobe", FFPROBE_ENV)?;

    let decoders =
        run_media_tool_probe(&ffmpeg, &["-hide_banner", "-decoders"], "ffmpeg decoders")?;
    require_media_capabilities(
        &decoders,
        "ffmpeg",
        FFMPEG_ENV,
        "decoder",
        &[("h264", "H.264 video"), ("aac", "AAC audio")],
    )?;
    let encoders =
        run_media_tool_probe(&ffmpeg, &["-hide_banner", "-encoders"], "ffmpeg encoders")?;
    require_media_capabilities(
        &encoders,
        "ffmpeg",
        FFMPEG_ENV,
        "encoder",
        &[("pcm_s16le", "signed 16-bit PCM audio")],
    )?;
    let muxers = run_media_tool_probe(&ffmpeg, &["-hide_banner", "-muxers"], "ffmpeg muxers")?;
    require_media_capabilities(
        &muxers,
        "ffmpeg",
        FFMPEG_ENV,
        "muxer",
        &[("s16le", "raw signed 16-bit PCM")],
    )?;
    let demuxers =
        run_media_tool_probe(&ffprobe, &["-hide_banner", "-demuxers"], "ffprobe demuxers")?;
    require_media_capabilities(
        &demuxers,
        "ffprobe",
        FFPROBE_ENV,
        "demuxer",
        &[("mov", "MP4/QuickTime")],
    )?;

    Ok(SulphurMediaTools { ffmpeg, ffprobe })
}

fn resolve_media_tool(program: &Path, label: &str, environment: &str) -> Result<PathBuf> {
    resolve_program(program, label).map_err(|error| {
        EngineError::Sulphur(format!(
            "{error}; install a complete FFmpeg distribution in the provider user's environment \
             or set {environment} to its {label} executable"
        ))
    })
}

fn require_media_tool_version(output: &[u8], label: &str, environment: &str) -> Result<()> {
    let text = media_probe_text(output, label)?;
    let expected = format!("{label} version ");
    if text.lines().next().is_some_and(|line| {
        line.starts_with(&expected) && !line[expected.len()..].trim().is_empty()
    }) {
        return Ok(());
    }
    Err(EngineError::Sulphur(format!(
        "Sulphur {label} executable returned invalid version evidence; install a complete FFmpeg \
         distribution or set {environment} to its {label} executable"
    )))
}

fn require_media_capabilities(
    output: &[u8],
    label: &str,
    environment: &str,
    capability_kind: &str,
    required: &[(&str, &str)],
) -> Result<()> {
    let text = media_probe_text(output, label)?;
    let missing = required
        .iter()
        .filter(|(name, _)| !media_inventory_has(text, name))
        .map(|(name, description)| format!("{description} ({name})"))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(());
    }
    Err(EngineError::Sulphur(format!(
        "Sulphur {label} lacks required {capability_kind} capability: {}; install a complete \
         FFmpeg distribution or set {environment} to a compatible {label} executable",
        missing.join(", ")
    )))
}

fn media_inventory_has(output: &str, expected: &str) -> bool {
    output.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let Some(flags) = fields.next() else {
            return false;
        };
        let Some(names) = fields.next() else {
            return false;
        };
        if flags.len() > 8
            || flags.is_empty()
            || !flags
                .bytes()
                .all(|byte| byte == b'.' || byte.is_ascii_uppercase())
        {
            return false;
        }
        names.split(',').any(|name| name == expected)
    })
}

fn media_probe_text<'a>(output: &'a [u8], label: &str) -> Result<&'a str> {
    std::str::from_utf8(output).map_err(|_| {
        EngineError::Sulphur(format!(
            "Sulphur {label} capability probe returned non-UTF-8 output"
        ))
    })
}

fn run_media_tool_probe(program: &Path, arguments: &[&str], label: &str) -> Result<Vec<u8>> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = MEDIA_TOOL_PROBE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let prefix = format!(
        "mayhem-sulphur-media-probe-{}-{nonce}-{counter}",
        std::process::id()
    );
    let stdout_path = env::temp_dir().join(format!("{prefix}.stdout"));
    let stderr_path = env::temp_dir().join(format!("{prefix}.stderr"));
    let cleanup = MediaProbeCleanup {
        paths: vec![stdout_path.clone(), stderr_path.clone()],
    };
    let stdout = open_media_probe_file(&stdout_path)?;
    let stderr = open_media_probe_file(&stderr_path)?;
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| {
            EngineError::Sulphur(format!(
                "starting Sulphur {label} capability probe with {} failed: {error}",
                program.display()
            ))
        })?;
    let deadline = Instant::now() + MEDIA_TOOL_PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(EngineError::Sulphur(format!(
                    "Sulphur {label} capability probe exceeded {} seconds",
                    MEDIA_TOOL_PROBE_TIMEOUT.as_secs()
                )));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(EngineError::Sulphur(format!(
                    "waiting for Sulphur {label} capability probe failed: {error}"
                )));
            }
        }
    };
    let stdout = read_media_probe_file(&stdout_path, label)?;
    let stderr = read_media_probe_file(&stderr_path, label)?;
    drop(cleanup);
    if status.success() {
        return Ok(stdout);
    }
    let detail = String::from_utf8_lossy(&stderr);
    let detail = detail.trim();
    let detail = if detail.is_empty() {
        "no diagnostic output".to_owned()
    } else {
        detail.chars().take(512).collect()
    };
    Err(EngineError::Sulphur(format!(
        "Sulphur {label} capability probe failed with {status}: {detail}"
    )))
}

fn open_media_probe_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            EngineError::Sulphur(format!(
                "creating bounded Sulphur media-tool probe output {} failed: {error}",
                path.display()
            ))
        })
}

fn read_media_probe_file(path: &Path, label: &str) -> Result<Vec<u8>> {
    let size = fs::metadata(path)?.len();
    if size > MEDIA_TOOL_PROBE_MAX_OUTPUT_BYTES {
        return Err(EngineError::Sulphur(format!(
            "Sulphur {label} capability probe exceeded its {}-byte output bound",
            MEDIA_TOOL_PROBE_MAX_OUTPUT_BYTES
        )));
    }
    fs::read(path).map_err(EngineError::from)
}

struct MediaProbeCleanup {
    paths: Vec<PathBuf>,
}

impl Drop for MediaProbeCleanup {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoopArtifactSink;
    use std::process::Child;

    struct TestTree {
        root: PathBuf,
    }

    impl TestTree {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let root = env::temp_dir().join(format!(
                "mayhem-engine-sulphur-{label}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("create Sulphur test root");
            Self { root }
        }

        fn gguf(&self) -> PathBuf {
            let model = self.root.join("model/sulphur-dev.gguf");
            fs::create_dir_all(model.parent().unwrap()).expect("create model root");
            fs::write(&model, b"GGUFsulphur-test").expect("write GGUF fixture");
            model
        }

        fn mlx(&self) -> PathBuf {
            let artifact_root = self.root.join("bundle/sulphur");
            fs::create_dir_all(&artifact_root).expect("create MLX artifact root");
            let header = br#"{"__metadata__":{}}"#;
            let mut safetensors = Vec::with_capacity(8 + header.len());
            safetensors.extend_from_slice(&(header.len() as u64).to_le_bytes());
            safetensors.extend_from_slice(header);
            fs::write(
                artifact_root.join("transformer-distilled.safetensors"),
                safetensors,
            )
            .expect("write MLX fixture");
            fs::write(
                self.root.join("bundle").join(MLX_RUNTIME_MANIFEST_NAME),
                b"{}",
            )
            .expect("write MLX manifest fixture");
            artifact_root
        }

        #[cfg(unix)]
        fn mock_worker(&self) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;

            let runtime = self.root.join("runtime/bin");
            fs::create_dir_all(&runtime).expect("create mock runtime");
            let worker = runtime.join("mock-sulphur-worker.py");
            let interpreter =
                resolve_python_program(Path::new("python3")).expect("find Python for mock worker");
            let source = MOCK_WORKER.replacen(
                "#!/usr/bin/env python3",
                &format!("#!{}", interpreter.display()),
                1,
            );
            fs::write(&worker, source).expect("write mock worker");
            fs::set_permissions(&worker, fs::Permissions::from_mode(0o755))
                .expect("make mock worker executable");
            worker
        }

        #[cfg(unix)]
        fn media_tool(&self, name: &str, source: &str) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;

            let tools = self.root.join("media-tools");
            fs::create_dir_all(&tools).expect("create mock media tools");
            let tool = tools.join(name);
            fs::write(&tool, source).expect("write mock media tool");
            fs::set_permissions(&tool, fs::Permissions::from_mode(0o755))
                .expect("make mock media tool executable");
            tool
        }
    }

    impl Drop for TestTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn media_tool_preflight_rejects_missing_binaries_with_actionable_guidance() {
        let tree = TestTree::new("missing-media-tools");
        let error = preflight_media_tools(
            &tree.root.join("missing-ffmpeg"),
            &tree.root.join("missing-ffprobe"),
        )
        .expect_err("missing media tools must fail before model load");
        let error = error.to_string();
        assert!(error.contains("ffmpeg"));
        assert!(error.contains(FFMPEG_ENV));
        assert!(error.contains("complete FFmpeg distribution"));
    }

    #[cfg(unix)]
    #[test]
    fn media_tool_preflight_rejects_an_inadequate_ffmpeg_build() {
        let tree = TestTree::new("inadequate-media-tools");
        let ffmpeg = tree.media_tool("ffmpeg", INADEQUATE_FFMPEG);
        let ffprobe = tree.media_tool("ffprobe", VALID_FFPROBE);
        let error = preflight_media_tools(&ffmpeg, &ffprobe)
            .expect_err("ffmpeg without AAC decoding must fail");
        let error = error.to_string();
        assert!(error.contains("AAC audio (aac)"));
        assert!(error.contains("decoder capability"));
        assert!(error.contains(FFMPEG_ENV));
    }

    #[cfg(unix)]
    #[test]
    fn media_tool_preflight_rejects_missing_or_inadequate_ffprobe() {
        let tree = TestTree::new("invalid-ffprobe");
        let ffmpeg = tree.media_tool("ffmpeg", VALID_FFMPEG);
        let missing = tree.root.join("missing-ffprobe");
        let error =
            preflight_media_tools(&ffmpeg, &missing).expect_err("missing ffprobe must fail");
        assert!(error.to_string().contains(FFPROBE_ENV));

        let ffprobe = tree.media_tool("ffprobe", INADEQUATE_FFPROBE);
        let error = preflight_media_tools(&ffmpeg, &ffprobe)
            .expect_err("ffprobe without MP4 demuxing must fail");
        let error = error.to_string();
        assert!(error.contains("MP4/QuickTime (mov)"));
        assert!(error.contains("demuxer capability"));
        assert!(error.contains(FFPROBE_ENV));
    }

    #[cfg(unix)]
    #[test]
    fn media_tool_preflight_accepts_a_complete_capability_fixture() {
        let tree = TestTree::new("valid-media-tools");
        let ffmpeg = tree.media_tool("ffmpeg", VALID_FFMPEG);
        let ffprobe = tree.media_tool("ffprobe", VALID_FFPROBE);
        let tools =
            preflight_media_tools(&ffmpeg, &ffprobe).expect("complete media tools must pass");
        assert_eq!(tools.ffmpeg, fs::canonicalize(ffmpeg).unwrap());
        assert_eq!(tools.ffprobe, fs::canonicalize(ffprobe).unwrap());
    }

    fn test_iso_box(kind: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = u32::try_from(payload.len() + 8).expect("test box size");
        let mut bytes = Vec::with_capacity(size as usize);
        bytes.extend_from_slice(&size.to_be_bytes());
        bytes.extend_from_slice(&kind);
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn mp4_track_uses_presentation_duration_instead_of_padded_media_duration() {
        let tree = TestTree::new("mp4-presentation-duration");
        let path = tree.root.join("track.bin");

        let mut tkhd = vec![0_u8; 24];
        tkhd[20..24].copy_from_slice(&2_125_u32.to_be_bytes());
        let mut mdhd = vec![0_u8; 20];
        mdhd[12..16].copy_from_slice(&48_000_u32.to_be_bytes());
        mdhd[16..20].copy_from_slice(&144_000_u32.to_be_bytes());
        let mut hdlr = vec![0_u8; 12];
        hdlr[8..12].copy_from_slice(b"soun");

        let mut mdia = test_iso_box(*b"mdhd", &mdhd);
        mdia.extend_from_slice(&test_iso_box(*b"hdlr", &hdlr));
        let mut track = test_iso_box(*b"tkhd", &tkhd);
        track.extend_from_slice(&test_iso_box(*b"mdia", &mdia));
        fs::write(&path, &track).expect("write track fixture");

        let mut file = File::open(&path).expect("open track fixture");
        let parsed = read_mp4_track(
            &mut file,
            IsoBox {
                kind: *b"trak",
                payload_start: 0,
                end: track.len() as u64,
            },
            1_000,
        )
        .expect("parse presentation duration");
        assert_eq!(parsed.kind, *b"soun");
        assert!((parsed.duration_seconds - 2.125).abs() < f64::EPSILON);
    }

    #[test]
    fn version_one_movie_and_track_durations_are_supported() {
        let tree = TestTree::new("mp4-version-one-duration");
        let movie_path = tree.root.join("mvhd.bin");
        let track_path = tree.root.join("tkhd.bin");
        let mut mvhd = vec![0_u8; 24];
        mvhd[0] = 1;
        mvhd[20..24].copy_from_slice(&1_000_u32.to_be_bytes());
        let mut tkhd = vec![0_u8; 36];
        tkhd[0] = 1;
        tkhd[28..36].copy_from_slice(&2_125_u64.to_be_bytes());
        fs::write(&movie_path, &mvhd).expect("write movie header fixture");
        fs::write(&track_path, &tkhd).expect("write track header fixture");

        let mut movie = File::open(&movie_path).expect("open movie header fixture");
        let timescale = read_mp4_movie_timescale(
            &mut movie,
            IsoBox {
                kind: *b"mvhd",
                payload_start: 0,
                end: mvhd.len() as u64,
            },
        )
        .expect("parse movie timescale");
        let mut track = File::open(&track_path).expect("open track header fixture");
        let duration = read_mp4_track_duration(
            &mut track,
            IsoBox {
                kind: *b"tkhd",
                payload_start: 0,
                end: tkhd.len() as u64,
            },
            timescale,
        )
        .expect("parse track duration");
        assert!((duration - 2.125).abs() < f64::EPSILON);
    }

    #[cfg(unix)]
    #[test]
    fn python_runtime_preserves_the_managed_venv_entrypoint() {
        use std::os::unix::fs::symlink;

        let tree = TestTree::new("python-venv-entrypoint");
        let base_python = resolve_program(Path::new("python3"), "Python").expect("base Python");
        let venv = tree.root.join("venv");
        let bin = venv.join("bin");
        fs::create_dir_all(venv.join("lib/python-test/site-packages"))
            .expect("create venv library");
        fs::create_dir_all(&bin).expect("create venv bin");
        let entrypoint = bin.join("python");
        symlink(&base_python, &entrypoint).expect("link venv Python");
        fs::write(
            venv.join("pyvenv.cfg"),
            format!(
                "home = {}\n",
                base_python.parent().expect("base Python parent").display()
            ),
        )
        .expect("write pyvenv config");

        let (resolved, roots) = python_runtime_roots(&entrypoint).expect("resolve venv Python");
        assert_eq!(resolved, entrypoint);
        let canonical_venv = fs::canonicalize(&venv).expect("canonical venv");
        assert!(
            roots.iter().any(|root| root == &canonical_venv),
            "sandbox roots must retain the managed venv"
        );
    }

    #[test]
    fn worker_path_contains_only_the_bound_runtime_and_media_tool_roots() {
        let tree = TestTree::new("worker-path");
        let python = tree.root.join("venv/bin/python");
        let ffmpeg = tree.root.join("media/ffmpeg");
        let ffprobe = tree.root.join("media/ffprobe");
        let path = sulphur_worker_path(&python, &ffmpeg, &ffprobe).expect("worker PATH");
        let entries = env::split_paths(&path).collect::<Vec<_>>();
        assert_eq!(entries[0], python.parent().unwrap());
        assert_eq!(entries[1], ffmpeg.parent().unwrap());
        assert_eq!(
            entries
                .iter()
                .filter(|entry| *entry == ffmpeg.parent().unwrap())
                .count(),
            1
        );
    }

    #[test]
    fn executable_roots_collapse_nested_system_directories() {
        let mut roots = vec![PathBuf::from("/managed/venv"), PathBuf::from("/usr")];
        insert_non_overlapping_root(&mut roots, PathBuf::from("/usr/bin"));
        assert_eq!(
            roots,
            vec![PathBuf::from("/managed/venv"), PathBuf::from("/usr")]
        );

        insert_non_overlapping_root(&mut roots, PathBuf::from("/managed"));
        assert_eq!(
            roots,
            vec![PathBuf::from("/usr"), PathBuf::from("/managed")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn managed_venv_adapter_import_survives_the_worker_sandbox() {
        use std::os::unix::fs::symlink;

        let tree = TestTree::new("python-venv-sandbox-import");
        let base_python = resolve_program(Path::new("python3"), "Python").expect("base Python");
        let version = Command::new(&base_python)
            .args([
                "-c",
                "import sys; print(f'python{sys.version_info.major}.{sys.version_info.minor}')",
            ])
            .output()
            .expect("query Python version");
        assert!(version.status.success());
        let version = String::from_utf8(version.stdout)
            .expect("Python version UTF-8")
            .trim()
            .to_owned();
        let venv = tree.root.join("venv");
        let bin = venv.join("bin");
        let site_packages = venv.join("lib").join(version).join("site-packages");
        fs::create_dir_all(&site_packages).expect("create venv site-packages");
        fs::create_dir_all(&bin).expect("create venv bin");
        let entrypoint = bin.join("python");
        symlink(&base_python, &entrypoint).expect("link venv Python");
        fs::write(
            venv.join("pyvenv.cfg"),
            format!(
                "home = {}\ninclude-system-site-packages = false\n",
                base_python.parent().expect("base Python parent").display()
            ),
        )
        .expect("write pyvenv config");
        fs::write(site_packages.join("sulphur_test_adapter.py"), TEST_ADAPTER)
            .expect("write sandbox adapter");

        let model = tree.gguf();
        let mut config = LoadConfig::gguf(&model);
        config.backend_cache_dir = Some(tree.root.join("cache"));
        let mut backend = SulphurBackend::with_runtime(&entrypoint, "sulphur_test_adapter")
            .expect("construct venv backend");
        let loaded = backend.load(config).expect("load through venv sandbox");
        assert_eq!(loaded.backend, "sulphur");
    }

    fn request(endpoint: &str, body: Value) -> MediaGenerationRequest {
        let prompt = body
            .get(if endpoint == ENDPOINT_HF_TEXT_TO_VIDEO {
                "inputs"
            } else {
                "prompt"
            })
            .and_then(Value::as_str)
            .unwrap_or("test prompt")
            .to_owned();
        MediaGenerationRequest {
            endpoint_family: endpoint.to_owned(),
            prompt,
            request: body,
            duration_seconds: None,
            frame_count: None,
            step_count: None,
            response_format: Some("mp4".to_owned()),
        }
    }

    #[test]
    fn distilled_surface_enforces_source_geometry_and_rejects_unavailable_controls() {
        let valid = NormalizedVideoRequest::from_media_request(request(
            ENDPOINT_HF_TEXT_TO_VIDEO,
            json!({
                "inputs": "joint audio and video",
                "parameters": {
                    "width": 768,
                    "height": 512,
                    "num_frames": 121,
                    "fps": 24,
                    "seed": 7,
                    "negative_prompt": "watermark",
                    "n": 1,
                    "enhance_prompt": false
                }
            }),
        ))
        .expect("normalize source-faithful request");
        assert_eq!(valid.frame_count, 121);
        assert_eq!(valid.worker_payload()["frame_rate"], 24.0);
        assert_eq!(valid.handled_controls().len(), 9);
        assert_eq!(valid.worker_payload()["negative_prompt"], "watermark");
        assert_eq!(
            valid.handled_request_attributes,
            BTreeSet::from([
                "inputs".to_owned(),
                "parameters.enhance_prompt".to_owned(),
                "parameters.fps".to_owned(),
                "parameters.height".to_owned(),
                "parameters.num_frames".to_owned(),
                "parameters.negative_prompt".to_owned(),
                "parameters.n".to_owned(),
                "parameters.seed".to_owned(),
                "parameters.width".to_owned(),
            ])
        );
        assert!(!valid
            .worker_payload()
            .as_object()
            .unwrap()
            .contains_key("steps"));

        for (field, value) in [
            ("guidance_scale", json!(4.0)),
            ("num_inference_steps", json!(11)),
            ("duration_seconds", json!(2.0)),
        ] {
            let mut parameters = Map::new();
            parameters.insert(field.to_owned(), value);
            let error = NormalizedVideoRequest::from_media_request(request(
                ENDPOINT_HF_TEXT_TO_VIDEO,
                json!({
                    "inputs": "unsupported control",
                    "parameters": parameters
                }),
            ))
            .expect_err("distilled path must reject an unavailable control");
            assert!(error.to_string().contains("unsupported field"));
        }

        for (width, frames) in [(769, 121), (768, 120)] {
            let error = NormalizedVideoRequest::from_media_request(request(
                ENDPOINT_HF_TEXT_TO_VIDEO,
                json!({
                    "inputs": "invalid geometry",
                    "parameters": {
                        "width": width,
                        "height": 512,
                        "num_frames": frames
                    }
                }),
            ))
            .expect_err("invalid pinned geometry must fail");
            assert!(matches!(error, EngineError::InvalidConfig(_)));
        }
    }

    #[test]
    fn canonical_gateway_video_shapes_are_accepted_and_hf_frames_are_required() {
        let png = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nfixture");
        for (endpoint, body) in [
            (
                ENDPOINT_HF_TEXT_TO_VIDEO,
                json!({
                    "inputs": "signed HF request",
                    "parameters": {
                        "width": 768,
                        "height": 512,
                        "num_frames": 97,
                        "fps": 24,
                        "seed": 7,
                        "conditions": [{
                            "image_url": format!("data:image/png;base64,{png}"),
                            "frame_index": 0,
                            "strength": 0.7,
                            "crf": 33
                        }],
                        "enhance_prompt": false
                    }
                }),
            ),
            (
                ENDPOINT_OPENAI_VIDEOS,
                json!({
                    "model": "sulphur",
                    "prompt": "signed OpenAI request",
                    "width": 768,
                    "height": 512,
                    "num_frames": 97,
                    "fps": 24,
                    "seed": 7,
                    "conditions": [{
                        "image_url": format!("data:image/png;base64,{png}"),
                        "frame_index": 0,
                        "strength": 0.6,
                        "crf": 33
                    }],
                    "enhance_prompt": false
                }),
            ),
        ] {
            let normalized = NormalizedVideoRequest::from_media_request(request(endpoint, body))
                .expect("gateway-canonical video request");
            assert_eq!(normalized.frame_count, 97);
            assert_eq!(normalized.images.len(), 1);
            assert_eq!(
                normalized.images[0].strength,
                if endpoint == ENDPOINT_HF_TEXT_TO_VIDEO {
                    0.7
                } else {
                    0.6
                }
            );
        }

        let missing = NormalizedVideoRequest::from_media_request(request(
            ENDPOINT_HF_TEXT_TO_VIDEO,
            json!({
                "inputs": "missing signed default",
                "parameters": {"width": 768, "height": 512, "fps": 24}
            }),
        ))
        .expect_err("provider must not invent an HF frame default");
        assert!(missing
            .to_string()
            .contains("missing signed normalized num_frames"));
    }

    #[test]
    fn image_conditioning_is_inline_bounded_and_never_a_path_or_remote_url() {
        let png = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nfixture");
        let normalized = NormalizedVideoRequest::from_media_request(request(
            ENDPOINT_OPENAI_VIDEOS,
            json!({
                "model": "sulphur",
                "prompt": "animate this",
                "seconds": 1,
                "size": "256x256",
                "input_reference": {"image_url": format!("data:image/png;base64,{png}")}
            }),
        ))
        .expect("normalize inline I2V image");
        assert_eq!(normalized.images.len(), 1);
        assert_eq!(normalized.images[0].content_type, "image/png");
        assert_eq!(normalized.images[0].frame_index, 0);
        assert_eq!(normalized.images[0].strength, 1.0);
        assert_eq!(normalized.images[0].crf, 33);
        assert_eq!(
            normalized.handled_request_attributes,
            BTreeSet::from([
                "input_reference".to_owned(),
                "model".to_owned(),
                "prompt".to_owned(),
                "seconds".to_owned(),
                "size".to_owned(),
            ])
        );

        for unsafe_reference in [
            json!("https://example.invalid/input.png"),
            json!("/tmp/input.png"),
            json!({"image_url": "file:///tmp/input.png"}),
        ] {
            let error = NormalizedVideoRequest::from_media_request(request(
                ENDPOINT_OPENAI_VIDEOS,
                json!({
                    "model": "sulphur",
                    "prompt": "unsafe input",
                    "input_reference": unsafe_reference
                }),
            ))
            .expect_err("external image reference must fail");
            assert!(error.to_string().contains("cannot fetch"));
        }

        let error = NormalizedVideoRequest::from_media_request(request(
            ENDPOINT_OPENAI_VIDEOS,
            json!({
                "model": "sulphur",
                "prompt": "unsupported duration alias",
                "duration_seconds": 2
            }),
        ))
        .expect_err("unsigned duration alias must not bypass the endpoint contract");
        assert!(error
            .to_string()
            .contains("unsupported field duration_seconds"));
    }

    #[test]
    fn ordered_conditions_preserve_frame_strength_and_crf_on_both_endpoints() {
        let png = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nfirst");
        let jpeg = base64::engine::general_purpose::STANDARD.encode(b"\xff\xd8\xffsecond");
        for (endpoint, body) in [
            (
                ENDPOINT_OPENAI_VIDEOS,
                json!({
                    "model": "sulphur",
                    "prompt": "interpolate these keyframes",
                    "size": "256x256",
                    "num_frames": 17,
                    "conditions": [
                        {
                            "image_url": format!("data:image/png;base64,{png}"),
                            "frame_index": 0,
                            "strength": 0.75,
                            "crf": 20
                        },
                        {
                            "image_url": format!("data:image/jpeg;base64,{jpeg}"),
                            "frame_index": 16
                        }
                    ],
                    "negative_prompt": "watermark",
                    "n": 1
                }),
            ),
            (
                ENDPOINT_HF_TEXT_TO_VIDEO,
                json!({
                    "inputs": "interpolate these keyframes",
                    "parameters": {
                        "width": 256,
                        "height": 256,
                        "num_frames": 17,
                        "conditions": [
                            {
                                "image_url": format!("data:image/png;base64,{png}"),
                                "frame_index": 0,
                                "strength": 0.75,
                                "crf": 20
                            },
                            {
                                "image_url": format!("data:image/jpeg;base64,{jpeg}"),
                                "frame_index": 16
                            }
                        ],
                        "negative_prompt": ["watermark"],
                        "n": 1
                    }
                }),
            ),
        ] {
            let normalized = NormalizedVideoRequest::from_media_request(request(endpoint, body))
                .expect("normalize ordered conditions");
            assert_eq!(normalized.images.len(), 2);
            assert_eq!(normalized.images[0].frame_index, 0);
            assert_eq!(normalized.images[0].strength, 0.75);
            assert_eq!(normalized.images[0].crf, 20);
            assert_eq!(normalized.images[1].frame_index, 16);
            assert_eq!(normalized.images[1].strength, 1.0);
            assert_eq!(normalized.images[1].crf, 33);
            assert_eq!(normalized.negative_prompt, "watermark");
        }
    }

    #[test]
    fn conditions_reject_unsafe_references_invalid_metadata_and_multiple_outputs() {
        let png = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nfixture");
        for (field, value, expected) in [
            (
                "image_url",
                json!("https://example.invalid/input.png"),
                "cannot fetch",
            ),
            ("frame_index", json!(17), "identify an output frame"),
            ("strength", json!(1.1), "between 0 and 1"),
            ("crf", json!(52), "between 0 and 51"),
        ] {
            let mut condition = Map::from_iter([
                (
                    "image_url".to_owned(),
                    json!(format!("data:image/png;base64,{png}")),
                ),
                ("frame_index".to_owned(), json!(0)),
                ("strength".to_owned(), json!(1.0)),
                ("crf".to_owned(), json!(33)),
            ]);
            condition.insert(field.to_owned(), value);
            let error = NormalizedVideoRequest::from_media_request(request(
                ENDPOINT_OPENAI_VIDEOS,
                json!({
                    "model": "sulphur",
                    "prompt": "invalid condition",
                    "size": "256x256",
                    "num_frames": 17,
                    "conditions": [condition]
                }),
            ))
            .expect_err("invalid condition must fail");
            assert!(error.to_string().contains(expected), "{error}");
        }

        let error = NormalizedVideoRequest::from_media_request(request(
            ENDPOINT_OPENAI_VIDEOS,
            json!({
                "model": "sulphur",
                "prompt": "too many outputs",
                "n": 2
            }),
        ))
        .expect_err("Sulphur is a single-output runtime");
        assert!(error.to_string().contains("n must be exactly 1"));
    }

    #[test]
    fn openai_video_honors_explicit_ltx_frames_without_outer_rounding_drift() {
        let mut explicit = request(
            ENDPOINT_OPENAI_VIDEOS,
            json!({
                "model": "sulphur",
                "prompt": "animate this",
                "seconds": "2",
                "size": "256x256",
                "num_frames": 9,
                "fps": 8
            }),
        );
        explicit.request.as_object_mut().unwrap().remove("seconds");
        explicit.duration_seconds = Some(2);
        explicit.frame_count = Some(9);
        let normalized =
            NormalizedVideoRequest::from_media_request(explicit).expect("honor explicit frames");
        assert_eq!(normalized.frame_count, 9);
        assert_eq!(normalized.duration_seconds, 1.125);

        let error = NormalizedVideoRequest::from_media_request(request(
            ENDPOINT_OPENAI_VIDEOS,
            json!({
                "model": "sulphur",
                "prompt": "conflicting duration",
                "seconds": "10",
                "size": "256x256",
                "num_frames": 9,
                "fps": 8
            }),
        ))
        .expect_err("alternative duration controls must not be combined");
        assert!(error
            .to_string()
            .contains("seconds and num_frames are alternative controls"));
    }

    #[test]
    fn openai_video_honors_contract_resolved_frames_for_duration_requests() {
        let mut resolved = request(
            ENDPOINT_OPENAI_VIDEOS,
            json!({
                "model": "sulphur",
                "prompt": "animate this",
                "seconds": "4",
                "size": "256x256",
                "fps": 24
            }),
        );
        resolved.duration_seconds = Some(4);
        resolved.frame_count = Some(89);
        let normalized = NormalizedVideoRequest::from_media_request(resolved)
            .expect("honor contract-resolved frame count");
        assert_eq!(normalized.frame_count, 89);
        assert!((normalized.duration_seconds - (89.0 / 24.0)).abs() < 1e-9);

        let mut overrun = request(
            ENDPOINT_OPENAI_VIDEOS,
            json!({
                "model": "sulphur",
                "prompt": "animate this",
                "seconds": "4",
                "size": "256x256",
                "fps": 24
            }),
        );
        overrun.duration_seconds = Some(4);
        overrun.frame_count = Some(97);
        let error = NormalizedVideoRequest::from_media_request(overrun)
            .expect_err("resolved frames may not exceed the requested duration");
        assert!(error
            .to_string()
            .contains("duration conflicts with num_frames/fps"));

        let mut fractional = request(
            ENDPOINT_OPENAI_VIDEOS,
            json!({
                "model": "sulphur",
                "prompt": "animate this",
                "seconds": 3.1,
                "size": "256x256",
                "fps": 24
            }),
        );
        fractional.duration_seconds = Some(4);
        fractional.frame_count = Some(73);
        let normalized = NormalizedVideoRequest::from_media_request(fractional)
            .expect("fractional duration must use the contract-resolved frame count");
        assert_eq!(normalized.frame_count, 73);
        assert!((normalized.duration_seconds - (73.0 / 24.0)).abs() < 1e-9);
        assert!(normalized.duration_seconds <= 3.1);
    }

    #[test]
    fn openai_video_accepts_signed_dimensions_and_ltx_duration_quantization() {
        let normalized = NormalizedVideoRequest::from_media_request(request(
            ENDPOINT_OPENAI_VIDEOS,
            json!({
                "model": "sulphur",
                "prompt": "a silver sphere",
                "width": 256,
                "height": 320,
                "seconds": "10",
                "fps": 50
            }),
        ))
        .expect("normalize signed dimensions and quantized duration");

        assert_eq!((normalized.width, normalized.height), (256, 320));
        assert_eq!(normalized.frame_count, 497);
        assert!((normalized.duration_seconds - 9.94).abs() < 1e-9);
        assert!(normalized.handled_request_attributes.contains("width"));
        assert!(normalized.handled_request_attributes.contains("height"));
    }

    #[test]
    fn hf_video_honors_explicit_ltx_frames_without_outer_rounding_drift() {
        let mut explicit = request(
            ENDPOINT_HF_TEXT_TO_VIDEO,
            json!({
                "inputs": "animate this",
                "parameters": {
                    "width": 256,
                    "height": 256,
                    "num_frames": 9,
                    "fps": 8
                }
            }),
        );
        explicit.duration_seconds = Some(2);
        explicit.frame_count = Some(9);
        let normalized =
            NormalizedVideoRequest::from_media_request(explicit).expect("honor explicit frames");
        assert_eq!(normalized.frame_count, 9);
        assert_eq!(normalized.duration_seconds, 1.125);
    }

    #[test]
    fn prompt_enhancer_requires_a_complete_signed_pair_and_worker_never_rewrites() {
        let mut backend = SulphurBackend::with_python("python3").unwrap();
        let mut partial = LoadConfig::gguf("sulphur.gguf");
        partial.prompt_enhancer_model = Some(crate::ModelArtifact::gguf("enhancer.gguf"));
        let error = backend
            .load_prompt_enhancer(&partial)
            .expect_err("partial enhancer configuration must fail");
        assert!(error
            .to_string()
            .contains("requires both its signed GGUF and mmproj"));

        let parent = LoadConfig::gguf("sulphur.gguf");
        let model = ModelArtifact::gguf("enhancer.gguf");
        let projector = ModelArtifact::gguf("enhancer-mmproj.gguf");
        let enhancer = prompt_enhancer_load_config(&parent, &model, &projector, 0);
        validate_load_config(&enhancer).expect("complete enhancer KV-cache profile");
        assert_eq!(enhancer.kv_cache_dtype.as_deref(), Some("q8_0"));
        assert_eq!(enhancer.kv_cache_bits, Some(8));
        assert_eq!(enhancer.kv_cache_group_size, Some(32));
        assert_eq!(enhancer.kv_cache_quantized_start_tokens, Some(0));

        let normalized = NormalizedVideoRequest::from_media_request(request(
            ENDPOINT_OPENAI_VIDEOS,
            json!({
                "model": "sulphur",
                "prompt": "expand this",
                "size": "256x256",
                "num_frames": 9,
                "fps": 8,
                "enhance_prompt": true
            }),
        ))
        .expect("normalize enhancer request");
        assert!(normalized.enhance_prompt);
        assert_eq!(normalized.worker_payload()["enhance_prompt"], false);
    }

    #[test]
    fn pinned_execution_modes_do_not_treat_gguf_as_native_distilled() {
        let config = |backend: &str, mode: &str| SulphurExecutionConfig {
            api_version: 1,
            runtime_name: "ltx-2".to_owned(),
            runtime_version: "test".to_owned(),
            backend: backend.to_owned(),
            distilled: true,
            joint_audio_video: true,
            prompt_enhancer: false,
            ltx_runtime_commit: LTX_RUNTIME_COMMIT.to_owned(),
            sulphur_source_commit: SULPHUR_SOURCE_COMMIT.to_owned(),
            distillation_mode: mode.to_owned(),
            stage_1_denoise_intervals: 8,
            stage_2_denoise_intervals: 3,
            ffmpeg_version: "ffmpeg version test".to_owned(),
            ffprobe_version: "ffprobe version test".to_owned(),
        };
        assert!(validate_execution_config(
            &config("gguf", "dev_transformer_plus_pinned_distill_lora"),
            "gguf"
        )
        .is_ok());
        assert!(
            validate_execution_config(&config("mlx", "native_distilled_artifact"), "mlx").is_ok()
        );
        assert!(
            validate_execution_config(&config("gguf", "native_distilled_artifact"), "gguf")
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn mlx_artifact_keeps_the_outer_signed_bundle_as_worker_model_root() {
        let tree = TestTree::new("mlx-root");
        let artifact_root = tree.mlx();
        let bundle_root = artifact_root.parent().unwrap().to_path_buf();
        let mut config = LoadConfig::mlx_safetensors(&artifact_root);
        config.backend_cache_dir = Some(tree.root.join("cache"));
        let mut backend = SulphurBackend::with_runtime(tree.mock_worker(), "unused.adapter")
            .expect("construct mock backend");

        let loaded = backend.load(config).expect("load MLX bundle");
        assert_eq!(backend.artifact_backend.as_deref(), Some("mlx"));
        assert_eq!(
            backend.model_root.as_deref(),
            Some(fs::canonicalize(bundle_root).unwrap().as_path())
        );
        assert_eq!(
            loaded.artifact.path,
            fs::canonicalize(artifact_root).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_kills_process_tree_and_lazily_reloads_worker() {
        let tree = TestTree::new("cancel");
        let model = tree.gguf();
        create_joint_fixture(model.parent().unwrap().join("fixture.mp4").as_path());
        let mut config = LoadConfig::gguf(&model);
        config.backend_cache_dir = Some(tree.root.join("cache"));
        let mut backend = SulphurBackend::with_runtime(tree.mock_worker(), "unused.adapter")
            .expect("construct mock backend");
        backend.load(config).expect("load mock backend");
        assert!(backend.component_healthy());
        let first_pid = backend.process_ids()[0];

        let cancellation = CancellationToken::new();
        let trigger = cancellation.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            trigger.cancel();
        });
        let error = backend
            .generate_video(
                request(
                    ENDPOINT_HF_TEXT_TO_VIDEO,
                    json!({
                        "inputs": "cancel-me",
                        "parameters": {"width":256,"height":256,"num_frames":9,"fps":8}
                    }),
                ),
                &mut NoopArtifactSink,
                &cancellation,
            )
            .expect_err("generation must be cancelled");
        canceller.join().expect("join cancellation thread");
        assert!(matches!(error, EngineError::Cancelled));
        assert!(backend.process_ids().is_empty());
        assert!(!backend.component_healthy());

        let child_pid_path = find_file_named(&tree.root, "descendant.pid")
            .expect("mock descendant PID was recorded");
        let child_pid = fs::read_to_string(child_pid_path)
            .expect("read descendant PID")
            .trim()
            .parse::<u32>()
            .expect("parse descendant PID");
        assert!(wait_for_process_exit(child_pid));

        let mut chunks = Vec::new();
        let output = backend
            .generate_video(
                request(
                    ENDPOINT_HF_TEXT_TO_VIDEO,
                    json!({
                        "inputs": "normal",
                        "parameters": {"width":256,"height":256,"num_frames":9,"fps":8}
                    }),
                ),
                &mut |chunk: ArtifactChunk| {
                    chunks.push(chunk);
                    Ok(())
                },
                &CancellationToken::new(),
            )
            .expect("generate after lazy reload");
        assert_eq!(output.frame_count, 9);
        assert_eq!(output.step_count, 11);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content_type, "video/mp4");
        assert!(chunks[0].final_chunk);
        assert_ne!(backend.process_ids()[0], first_pid);
    }

    #[cfg(unix)]
    #[test]
    fn embedded_worker_rejects_video_only_silent_or_desynchronized_output() {
        let tree = TestTree::new("worker-media");
        let model = tree.gguf();
        let adapter = tree.root.join("sulphur_test_adapter.py");
        fs::write(&adapter, TEST_ADAPTER).expect("write worker test adapter");
        for mode in ["video_only", "silent", "desync", "valid"] {
            let mut worker = start_embedded_worker(&tree, &model, mode);
            let load = read_json_line(&mut worker, 1);
            assert_eq!(load["ok"], true, "load failed: {load}");
            send_embedded_generation(&mut worker, &tree, mode);
            let generate = read_json_line(&mut worker, 2);
            if mode == "valid" {
                assert_eq!(generate["ok"], true, "valid A/V failed: {generate}");
                assert!(generate["result"]["media_evidence"]["audio_peak_s16"]
                    .as_u64()
                    .is_some_and(|peak| peak > 1));
            } else {
                assert_eq!(
                    generate["ok"], false,
                    "unsafe output was accepted: {generate}"
                );
            }
            assert_eq!(
                fs::read_dir(tree.root.join(format!("worker-{mode}/inputs")))
                    .expect("read worker inputs")
                    .count(),
                0,
                "worker retained materialized conditioning input"
            );
            stop_embedded_worker(worker);
        }
    }

    #[cfg(unix)]
    fn create_joint_fixture(path: &Path) {
        let status = Command::new(resolve_program(Path::new("ffmpeg"), "ffmpeg").unwrap())
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=256x256:r=8:d=1.125",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=16000:duration=1.125",
                "-frames:v",
                "9",
                "-c:v",
                "mpeg4",
                "-bf",
                "0",
                "-c:a",
                "aac",
                "-shortest",
                "-y",
            ])
            .arg(path)
            .status()
            .expect("run ffmpeg fixture generator");
        assert!(status.success(), "ffmpeg fixture generation failed");
    }

    #[cfg(unix)]
    fn find_file_named(root: &Path, name: &str) -> Option<PathBuf> {
        let entries = fs::read_dir(root).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().and_then(|value| value.to_str()) == Some(name) {
                return Some(path);
            }
            if path.is_dir() {
                if let Some(found) = find_file_named(&path, name) {
                    return Some(found);
                }
            }
        }
        None
    }

    #[cfg(unix)]
    fn wait_for_process_exit(pid: u32) -> bool {
        for _ in 0..40 {
            if !Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
            {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[cfg(unix)]
    fn start_embedded_worker(tree: &TestTree, model: &Path, mode: &str) -> Child {
        let python = resolve_python_program(Path::new("python3")).expect("find Python");
        let worker = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sulphur_worker.py");
        let ffmpeg = resolve_program(Path::new("ffmpeg"), "ffmpeg").unwrap();
        let ffprobe = resolve_program(Path::new("ffprobe"), "ffprobe").unwrap();
        let cache = tree.root.join(format!("worker-{mode}"));
        fs::create_dir_all(&cache).expect("create worker cache");
        let mut child = Command::new(python)
            .arg("-u")
            .arg(worker)
            .current_dir(&tree.root)
            .env("PYTHONPATH", &tree.root)
            .env("TEST_SULPHUR_MODE", mode)
            .env("TEST_FFMPEG", &ffmpeg)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start embedded Sulphur worker");
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(
            stdin,
            "{}",
            json!({
                "id": 1,
                "op": "load",
                "payload": {
                    "artifact_path": model,
                    "backend": "gguf",
                    "cache_root": cache,
                    "ffmpeg_path": ffmpeg,
                    "ffprobe_path": ffprobe,
                    "model_root": model.parent().unwrap(),
                    "runtime_module": "sulphur_test_adapter"
                }
            })
        )
        .unwrap();
        stdin.flush().unwrap();
        child
    }

    #[cfg(unix)]
    fn send_embedded_generation(child: &mut Child, tree: &TestTree, mode: &str) {
        let stdin = child.stdin.as_mut().expect("worker stdin");
        writeln!(
            stdin,
            "{}",
            json!({
                "id": 2,
                "op": "generate_video",
                "payload": {
                    "output_path": tree.root.join(format!("worker-{mode}/outputs/result.mp4")),
                    "request": {
                        "prompt": "test",
                        "seed": 1,
                        "width": 256,
                        "height": 256,
                        "num_frames": 9,
                        "frame_rate": 8.0,
                        "images": [{
                            "content_type": "image/png",
                            "crf": 33,
                            "data_base64": base64::engine::general_purpose::STANDARD
                                .encode(b"\x89PNG\r\n\x1a\nworker-fixture"),
                            "frame_index": 0,
                            "strength": 1.0
                        }],
                        "negative_prompt": "",
                        "enhance_prompt": false
                    }
                }
            })
        )
        .unwrap();
        stdin.flush().unwrap();
    }

    #[cfg(unix)]
    fn read_json_line(child: &mut Child, expected_id: u64) -> Value {
        let stdout = child.stdout.as_mut().expect("worker stdout");
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read worker reply");
        let value: Value = serde_json::from_str(&line).expect("decode worker reply");
        assert_eq!(value["id"], expected_id);
        value
    }

    #[cfg(unix)]
    fn stop_embedded_worker(mut child: Child) {
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = writeln!(
                stdin,
                "{}",
                json!({"id": 3, "op": "shutdown", "payload": null})
            );
            let _ = stdin.flush();
        }
        let _ = child.wait();
    }

    #[cfg(unix)]
    const VALID_FFMPEG: &str = r#"#!/bin/sh
case "$*" in
  "-version")
    echo "ffmpeg version mayhem-test"
    ;;
  *"-decoders"*)
    echo " VFS..D h264 H.264 video"
    echo " A....D aac AAC audio"
    ;;
  *"-encoders"*)
    echo " A....D pcm_s16le signed 16-bit PCM"
    ;;
  *"-muxers"*)
    echo " E s16le raw signed 16-bit PCM"
    ;;
  *)
    exit 64
    ;;
esac
"#;

    #[cfg(unix)]
    const INADEQUATE_FFMPEG: &str = r#"#!/bin/sh
case "$*" in
  "-version")
    echo "ffmpeg version mayhem-test"
    ;;
  *"-decoders"*)
    echo " VFS..D h264 H.264 video"
    ;;
  *"-encoders"*)
    echo " A....D pcm_s16le signed 16-bit PCM"
    ;;
  *"-muxers"*)
    echo " E s16le raw signed 16-bit PCM"
    ;;
  *)
    exit 64
    ;;
esac
"#;

    #[cfg(unix)]
    const VALID_FFPROBE: &str = r#"#!/bin/sh
case "$*" in
  "-version")
    echo "ffprobe version mayhem-test"
    ;;
  *"-demuxers"*)
    echo " D mov,mp4,m4a,3gp,3g2,mj2 QuickTime / MOV"
    ;;
  *)
    exit 64
    ;;
esac
"#;

    #[cfg(unix)]
    const INADEQUATE_FFPROBE: &str = r#"#!/bin/sh
case "$*" in
  "-version")
    echo "ffprobe version mayhem-test"
    ;;
  *"-demuxers"*)
    echo " D matroska,webm Matroska / WebM"
    ;;
  *)
    exit 64
    ;;
esac
"#;

    #[cfg(unix)]
    const MOCK_WORKER: &str = r#"#!/usr/bin/env python3
import json
import os
import pathlib
import shutil
import subprocess
import sys
import time

os.setsid()
sys.stdin.buffer.readline()
model_root = None
cache_root = None
for line in sys.stdin:
    message = json.loads(line)
    message_id = message["id"]
    operation = message["op"]
    payload = message["payload"]
    try:
        if operation == "load":
            model_root = pathlib.Path(payload["model_root"])
            cache_root = pathlib.Path(payload["cache_root"])
            result = {
                "n_ctx_train": 0,
                "n_vocab": 0,
                "worker_pid": os.getpid(),
                "process_group_id": os.getpgrp(),
                "execution_config": {
                    "api_version": 1,
                    "runtime_name": "mock-ltx-2",
                    "runtime_version": "test",
                    "backend": payload["backend"],
                    "distilled": True,
                    "joint_audio_video": True,
                    "prompt_enhancer": True,
                    "ltx_runtime_commit": "9377758131b1ffde4b7f766804590a6617bf2ab9",
                    "sulphur_source_commit": "875e886e556b955d21149316fd631cc121db6cc1",
                    "distillation_mode": (
                        "native_distilled_artifact"
                        if payload["backend"] == "mlx"
                        else "dev_transformer_plus_pinned_distill_lora"
                    ),
                    "stage_1_denoise_intervals": 8,
                    "stage_2_denoise_intervals": 3,
                    "ffmpeg_version": "ffmpeg version mock",
                    "ffprobe_version": "ffprobe version mock"
                }
            }
        elif operation == "validate_video":
            result = {"valid": True, "handled_controls": sorted(payload)}
        elif operation == "generate_video":
            request = payload["request"]
            if request["prompt"] == "cancel-me":
                child = subprocess.Popen(["/bin/sleep", "60"])
                (cache_root / "descendant.pid").write_text(str(child.pid))
                time.sleep(60)
            shutil.copyfile(model_root / "fixture.mp4", payload["output_path"])
            result = {
                "output_path": payload["output_path"],
                "output_bytes": pathlib.Path(payload["output_path"]).stat().st_size,
                "duration_seconds": 1.125,
                "frame_count": request["num_frames"],
                "stage_1_denoise_intervals": 8,
                "stage_2_denoise_intervals": 3,
                "handled_controls": sorted(request),
                "media_evidence": {
                    "video_duration_seconds": 1.125,
                    "audio_duration_seconds": 1.125,
                    "duration_delta_seconds": 0.0,
                    "fps": request["frame_rate"],
                    "video_packet_count": request["num_frames"],
                    "audio_packet_count": 18,
                    "timestamps_monotonic": True,
                    "audio_peak_s16": 1000,
                    "ffprobe_decodable": True,
                    "ffmpeg_audio_decodable": True
                }
            }
        elif operation == "shutdown":
            print(json.dumps({"id": message_id, "ok": True, "result": {"shutdown": True}}), flush=True)
            break
        else:
            raise RuntimeError("unsupported operation")
        print(json.dumps({"id": message_id, "ok": True, "result": result}), flush=True)
    except Exception as error:
        print(json.dumps({"id": message_id, "ok": False, "error": str(error)}), flush=True)
"#;

    #[cfg(unix)]
    const TEST_ADAPTER: &str = r#"
import os
import pathlib
import subprocess

MAYHEM_SULPHUR_API_VERSION = 1

def load(**kwargs):
    return kwargs

def describe(runtime):
    return {
        "api_version": 1,
        "runtime_name": "test-ltx-2",
        "runtime_version": "test",
        "backend": runtime["backend"],
        "distilled": True,
        "joint_audio_video": True,
        "prompt_enhancer": False,
        "ltx_runtime_commit": "9377758131b1ffde4b7f766804590a6617bf2ab9",
        "sulphur_source_commit": "875e886e556b955d21149316fd631cc121db6cc1",
        "distillation_mode": "dev_transformer_plus_pinned_distill_lora",
        "stage_1_denoise_intervals": 8,
        "stage_2_denoise_intervals": 3,
    }

def validate_video(runtime, request):
    validate_images(runtime, request)
    return {"valid": True, "handled_controls": sorted(request)}

def validate_images(runtime, request):
    expected_root = (pathlib.Path(runtime["cache_root"]) / "inputs").resolve()
    for image in request["images"]:
        assert set(image) == {"content_type", "crf", "frame_index", "path", "strength"}
        path = pathlib.Path(image["path"])
        assert path.resolve().parent == expected_root
        assert path.name.startswith("conditioning-")
        assert path.is_file() and not path.is_symlink()
        assert 0 < path.stat().st_size <= 32 * 1024 * 1024

def generate_video(runtime, request, output_path):
    validate_images(runtime, request)
    command = [
        os.environ["TEST_FFMPEG"], "-v", "error",
        "-f", "lavfi", "-i", "color=c=black:s=256x256:r=8:d=1.125",
    ]
    mode = os.environ["TEST_SULPHUR_MODE"]
    if mode != "video_only":
        audio = (
            "anullsrc=channel_layout=mono:sample_rate=16000:d=1.125"
            if mode == "silent"
            else (
                "sine=frequency=440:sample_rate=16000:duration=0.5"
                if mode == "desync"
                else "sine=frequency=440:sample_rate=16000:duration=1.125"
            )
        )
        command += ["-f", "lavfi", "-i", audio]
    command += ["-frames:v", "9", "-c:v", "mpeg4", "-bf", "0"]
    if mode != "video_only":
        command += ["-c:a", "aac"]
        if mode != "desync":
            command += ["-shortest"]
    command += ["-y", output_path]
    subprocess.run(command, check=True)
    return {
        "duration_seconds": 1.125,
        "frame_count": request["num_frames"],
        "handled_controls": sorted(request),
        "stage_1_denoise_intervals": 8,
        "stage_2_denoise_intervals": 3,
    }
"#;
}
