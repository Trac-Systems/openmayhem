#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const CRATE_NAME: &str = "mayhem-engine";
pub const DEFAULT_CONTEXT_SIZE: u32 = 2048;
pub const DEFAULT_BATCH_SIZE: u32 = 512;
pub const DEFAULT_UBATCH_SIZE: u32 = 512;
pub const DEFAULT_SEED: u32 = 0x4d415948;
pub const MTMD_MEDIA_MARKER: &str = "<__media__>";
#[cfg(any(
    feature = "ace-step",
    feature = "mlx",
    feature = "vllm",
    feature = "trt-llm",
    feature = "transformers-asr"
))]
const WORKER_STDOUT_QUEUE_CAPACITY: usize = 64;

pub type Result<T> = std::result::Result<T, EngineError>;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("model artifact does not exist: {0}")]
    ModelPathMissing(PathBuf),
    #[error("unsupported artifact format for {path}: expected {expected}, got {actual:?}")]
    UnsupportedArtifactHeader {
        path: PathBuf,
        expected: &'static str,
        actual: Vec<u8>,
    },
    #[error("artifact hash mismatch for {path}: expected {expected}, got {actual}")]
    ArtifactHashMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("invalid engine config: {0}")]
    InvalidConfig(String),
    #[error("engine request cancelled")]
    Cancelled,
    #[error("model has not been loaded")]
    NotLoaded,
    #[error("prompt has {prompt_tokens} tokens, leaving no room in ctx_size={ctx_size}")]
    PromptTooLong { prompt_tokens: usize, ctx_size: u32 },
    #[error("MLX backend error: {0}")]
    Mlx(String),
    #[error("TensorRT-LLM backend error: {0}")]
    TrtLlm(String),
    #[error("vLLM backend error: {0}")]
    Vllm(String),
    #[error("Transformers ASR backend error: {0}")]
    TransformersAsr(String),
    #[error("ACE-Step backend error: {0}")]
    AceStep(String),
    #[error("stable-diffusion.cpp backend error: {0}")]
    StableDiffusionCpp(String),
    #[error("whisper.cpp backend error: {0}")]
    WhisperCpp(String),
    #[error("piper backend error: {0}")]
    Piper(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[cfg(feature = "llama-cpp")]
    #[error("llama.cpp error: {0}")]
    LlamaCpp(#[from] llama_cpp_2::LlamaCppError),
    #[cfg(feature = "llama-cpp")]
    #[error("llama.cpp model load error: {0}")]
    LlamaModelLoad(#[from] llama_cpp_2::LlamaModelLoadError),
    #[cfg(feature = "llama-cpp")]
    #[error("llama.cpp context load error: {0}")]
    LlamaContextLoad(#[from] llama_cpp_2::LlamaContextLoadError),
    #[cfg(feature = "llama-cpp")]
    #[error("llama.cpp decode error: {0}")]
    LlamaDecode(#[from] llama_cpp_2::DecodeError),
    #[cfg(feature = "llama-cpp")]
    #[error("llama.cpp encode error: {0}")]
    LlamaEncode(#[from] llama_cpp_2::EncodeError),
    #[cfg(feature = "llama-cpp")]
    #[error("llama.cpp embedding error: {0}")]
    LlamaEmbeddings(#[from] llama_cpp_2::EmbeddingsError),
    #[cfg(feature = "llama-cpp")]
    #[error("llama.cpp batch error: {0}")]
    LlamaBatch(#[from] llama_cpp_2::llama_batch::BatchAddError),
    #[cfg(feature = "llama-cpp")]
    #[error("llama.cpp tokenization error: {0}")]
    LlamaTokenize(#[from] llama_cpp_2::StringToTokenError),
    #[cfg(feature = "llama-cpp")]
    #[error("llama.cpp token decode error: {0}")]
    LlamaTokenDecode(#[from] llama_cpp_2::TokenToStringError),
    #[cfg(feature = "llama-cpp")]
    #[error("llama.cpp grammar error: {0}")]
    LlamaGrammar(#[from] llama_cpp_2::GrammarError),
}

#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(EngineError::Cancelled)
        } else {
            Ok(())
        }
    }
}

fn run_command_cancellable<F>(
    command: &mut std::process::Command,
    cancellation: &CancellationToken,
    map_io_error: F,
) -> Result<std::process::Output>
where
    F: Fn(std::io::Error) -> EngineError + Copy,
{
    use std::process::Stdio;
    use std::thread;
    use std::time::Duration;

    cancellation.check()?;
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(map_io_error)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| map_io_error(std::io::Error::other("opening child stdout failed")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| map_io_error(std::io::Error::other("opening child stderr failed")))?;
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut stdout = stdout;
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut stderr = stderr;
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });

    let status = loop {
        if cancellation.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(EngineError::Cancelled);
        }
        match child.try_wait().map_err(map_io_error)? {
            Some(status) => break status,
            None => thread::sleep(Duration::from_millis(25)),
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| map_io_error(std::io::Error::other("child stdout reader panicked")))?
        .map_err(map_io_error)?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| map_io_error(std::io::Error::other("child stderr reader panicked")))?
        .map_err(map_io_error)?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactFormat {
    Gguf,
    MlxSafetensors,
    TensorRtLlmCheckpoint,
    VllmSafetensors,
    TransformersSafetensors,
    AceStepSafetensors,
    StableDiffusionCheckpoint,
    WhisperGgml,
    PiperVoice,
    PiperConfig,
}

impl ArtifactFormat {
    fn magic(&self) -> &'static [u8] {
        match self {
            Self::Gguf => b"GGUF",
            Self::MlxSafetensors => b"",
            Self::TensorRtLlmCheckpoint => b"",
            Self::VllmSafetensors => b"",
            Self::TransformersSafetensors => b"",
            Self::AceStepSafetensors => b"",
            Self::StableDiffusionCheckpoint => b"",
            Self::WhisperGgml => b"",
            Self::PiperVoice => b"",
            Self::PiperConfig => b"",
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Gguf => "GGUF",
            Self::MlxSafetensors => "MLX safetensors",
            Self::TensorRtLlmCheckpoint => "TensorRT-LLM checkpoint",
            Self::VllmSafetensors => "vLLM safetensors",
            Self::TransformersSafetensors => "Transformers safetensors",
            Self::AceStepSafetensors => "ACE-Step safetensors",
            Self::StableDiffusionCheckpoint => "stable-diffusion checkpoint",
            Self::WhisperGgml => "whisper.cpp ggml model",
            Self::PiperVoice => "Piper voice",
            Self::PiperConfig => "Piper config",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelArtifact {
    pub path: PathBuf,
    pub format: ArtifactFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct StableDiffusionCppConfig {
    #[serde(default)]
    pub separate_diffusion_model: bool,
    #[serde(default)]
    pub guidance_scale_offset: i32,
    #[serde(default)]
    pub steps_offset: i32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MlxRuntimeConfig {
    #[serde(default)]
    pub multimodal: bool,
}

impl MlxRuntimeConfig {
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

impl StableDiffusionCppConfig {
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    pub fn validate(&self) -> Result<()> {
        if !(-16..=16).contains(&self.guidance_scale_offset) {
            return Err(EngineError::InvalidConfig(
                "stable-diffusion.cpp guidance_scale_offset must be between -16 and 16".to_owned(),
            ));
        }
        if !(-16..=16).contains(&self.steps_offset) {
            return Err(EngineError::InvalidConfig(
                "stable-diffusion.cpp steps_offset must be between -16 and 16".to_owned(),
            ));
        }
        Ok(())
    }
}

impl ModelArtifact {
    pub fn gguf(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            format: ArtifactFormat::Gguf,
            sha256: None,
        }
    }

    pub fn mlx_safetensors(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            format: ArtifactFormat::MlxSafetensors,
            sha256: None,
        }
    }

    pub fn trt_llm_checkpoint(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            format: ArtifactFormat::TensorRtLlmCheckpoint,
            sha256: None,
        }
    }

    pub fn vllm_safetensors(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            format: ArtifactFormat::VllmSafetensors,
            sha256: None,
        }
    }

    pub fn transformers_safetensors(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            format: ArtifactFormat::TransformersSafetensors,
            sha256: None,
        }
    }

    pub fn ace_step_safetensors(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            format: ArtifactFormat::AceStepSafetensors,
            sha256: None,
        }
    }

    pub fn stable_diffusion_checkpoint(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            format: ArtifactFormat::StableDiffusionCheckpoint,
            sha256: None,
        }
    }

    pub fn whisper_ggml(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            format: ArtifactFormat::WhisperGgml,
            sha256: None,
        }
    }

    pub fn piper_voice(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            format: ArtifactFormat::PiperVoice,
            sha256: None,
        }
    }

    pub fn piper_config(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            format: ArtifactFormat::PiperConfig,
            sha256: None,
        }
    }

    #[must_use]
    pub fn with_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.sha256 = Some(sha256.into());
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LoadConfig {
    pub artifact: ModelArtifact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision_projector: Option<ModelArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub piper_config: Option<ModelArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_diffusion_llm: Option<ModelArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_diffusion_vae: Option<ModelArtifact>,
    #[serde(default, skip_serializing_if = "StableDiffusionCppConfig::is_default")]
    pub stable_diffusion_cpp: StableDiffusionCppConfig,
    #[serde(default, skip_serializing_if = "MlxRuntimeConfig::is_default")]
    pub mlx_runtime: MlxRuntimeConfig,
    #[serde(default = "default_context_size")]
    pub ctx_size: u32,
    #[serde(default = "default_batch_size")]
    pub batch_size: u32,
    #[serde(default = "default_ubatch_size")]
    pub ubatch_size: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_layers: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trt_engine_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trt_tensor_parallel: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trt_kv_cache_dtype: Option<String>,
    #[serde(default)]
    pub trt_require_engine_dir: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv_cache_dtype: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv_cache_bits: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv_cache_group_size: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv_cache_quantized_start_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vllm_tensor_parallel: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vllm_dtype: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vllm_gpu_memory_utilization_pct: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vllm_gpu_memory_utilization_floor_pct: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_cache_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_diffusion_backend: Option<String>,
    #[serde(default = "default_true")]
    pub use_mmap: bool,
    #[serde(default)]
    pub use_mlock: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit_bytes: Option<u64>,
}

impl LoadConfig {
    pub fn gguf(path: impl Into<PathBuf>) -> Self {
        Self {
            artifact: ModelArtifact::gguf(path),
            ..Self::default()
        }
    }

    pub fn mlx_safetensors(path: impl Into<PathBuf>) -> Self {
        Self {
            artifact: ModelArtifact::mlx_safetensors(path),
            ..Self::default()
        }
    }

    pub fn trt_llm_checkpoint(path: impl Into<PathBuf>) -> Self {
        Self {
            artifact: ModelArtifact::trt_llm_checkpoint(path),
            ..Self::default()
        }
    }

    pub fn vllm_safetensors(path: impl Into<PathBuf>) -> Self {
        Self {
            artifact: ModelArtifact::vllm_safetensors(path),
            ..Self::default()
        }
    }

    pub fn transformers_safetensors(path: impl Into<PathBuf>) -> Self {
        Self {
            artifact: ModelArtifact::transformers_safetensors(path),
            ..Self::default()
        }
    }

    pub fn ace_step_safetensors(path: impl Into<PathBuf>) -> Self {
        Self {
            artifact: ModelArtifact::ace_step_safetensors(path),
            ..Self::default()
        }
    }

    pub fn stable_diffusion_checkpoint(path: impl Into<PathBuf>) -> Self {
        Self {
            artifact: ModelArtifact::stable_diffusion_checkpoint(path),
            ..Self::default()
        }
    }

    pub fn whisper_ggml(path: impl Into<PathBuf>) -> Self {
        Self {
            artifact: ModelArtifact::whisper_ggml(path),
            ..Self::default()
        }
    }

    pub fn piper_voice(path: impl Into<PathBuf>) -> Self {
        Self {
            artifact: ModelArtifact::piper_voice(path),
            ..Self::default()
        }
    }
}

impl Default for LoadConfig {
    fn default() -> Self {
        Self {
            artifact: ModelArtifact::gguf(PathBuf::new()),
            vision_projector: None,
            piper_config: None,
            stable_diffusion_llm: None,
            stable_diffusion_vae: None,
            stable_diffusion_cpp: StableDiffusionCppConfig::default(),
            mlx_runtime: MlxRuntimeConfig::default(),
            ctx_size: DEFAULT_CONTEXT_SIZE,
            batch_size: DEFAULT_BATCH_SIZE,
            ubatch_size: DEFAULT_UBATCH_SIZE,
            threads: None,
            gpu_layers: None,
            trt_engine_dir: None,
            trt_tensor_parallel: None,
            trt_kv_cache_dtype: None,
            trt_require_engine_dir: false,
            kv_cache_dtype: None,
            kv_cache_bits: None,
            kv_cache_group_size: None,
            kv_cache_quantized_start_tokens: None,
            vllm_tensor_parallel: None,
            vllm_dtype: None,
            vllm_gpu_memory_utilization_pct: None,
            vllm_gpu_memory_utilization_floor_pct: None,
            backend_cache_dir: None,
            stable_diffusion_backend: None,
            use_mmap: true,
            use_mlock: false,
            memory_limit_bytes: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LoadedModelInfo {
    pub backend: String,
    pub artifact: ModelArtifact,
    pub ctx_size: u32,
    pub n_ctx_train: u32,
    pub n_vocab: i32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Tokenization {
    pub token_ids: Vec<i32>,
}

impl Tokenization {
    #[must_use]
    pub fn len(&self) -> usize {
        self.token_ids.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.token_ids.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GenerateRequest {
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media: Vec<MediaInput>,
    #[serde(default = "default_max_new_tokens")]
    pub max_new_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grammar: Option<GrammarSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
    #[serde(default)]
    pub ignore_eos: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speciality_parameters: Vec<GenerateSpecialityParameter>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerateSpecialityTarget {
    ChatTemplateKwarg,
    SamplingParameter,
    PromptSuffix,
    BackendParameter,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GenerateSpecialityParameter {
    pub name: String,
    pub level: String,
    pub target: GenerateSpecialityTarget,
    pub native_path: String,
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_reasoning_tokens: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingRequest {
    pub inputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageGenerationRequest {
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,
    #[serde(default = "default_image_count")]
    pub image_count: u32,
    #[serde(default = "default_image_width")]
    pub width: u32,
    #[serde(default = "default_image_height")]
    pub height: u32,
    #[serde(default = "default_image_steps")]
    pub steps: u32,
    #[serde(default = "default_image_guidance_scale")]
    pub guidance_scale: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_shift: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<String>,
}

impl ImageGenerationRequest {
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            negative_prompt: None,
            image_count: default_image_count(),
            width: default_image_width(),
            height: default_image_height(),
            steps: default_image_steps(),
            guidance_scale: default_image_guidance_scale(),
            flow_shift: None,
            seed: None,
            sampling_method: None,
            scheduler: None,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.prompt.trim().is_empty() {
            return Err(EngineError::InvalidConfig(
                "image prompt must not be empty".to_owned(),
            ));
        }
        if !(1..=4).contains(&self.image_count) {
            return Err(EngineError::InvalidConfig(
                "image_count must be between 1 and 4".to_owned(),
            ));
        }
        if !(64..=2_048).contains(&self.width) || !(64..=2_048).contains(&self.height) {
            return Err(EngineError::InvalidConfig(
                "image dimensions must each be between 64 and 2048".to_owned(),
            ));
        }
        if !(1..=150).contains(&self.steps) {
            return Err(EngineError::InvalidConfig(
                "image steps must be between 1 and 150".to_owned(),
            ));
        }
        if !self.guidance_scale.is_finite() || !(0.0..=50.0).contains(&self.guidance_scale) {
            return Err(EngineError::InvalidConfig(
                "image guidance_scale must be finite and between 0 and 50".to_owned(),
            ));
        }
        if self
            .flow_shift
            .is_some_and(|shift| !shift.is_finite() || !(1.0..=10.0).contains(&shift))
        {
            return Err(EngineError::InvalidConfig(
                "image flow_shift must be finite and between 1 and 10".to_owned(),
            ));
        }
        for (name, value) in [
            ("sampling_method", self.sampling_method.as_deref()),
            ("scheduler", self.scheduler.as_deref()),
        ] {
            if value.is_some_and(|value| value.trim().is_empty() || value.len() > 128) {
                return Err(EngineError::InvalidConfig(format!(
                    "image {name} must be a non-empty string of at most 128 bytes"
                )));
            }
        }
        Ok(())
    }
}

impl EmbeddingRequest {
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            inputs: vec![input.into()],
            dimensions: None,
        }
    }

    pub fn many(inputs: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            inputs: inputs.into_iter().map(Into::into).collect(),
            dimensions: None,
        }
    }

    #[must_use]
    pub fn with_dimensions(mut self, dimensions: usize) -> Self {
        self.dimensions = Some(dimensions);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaInput {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_frames: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f32>,
}

impl GenerateRequest {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            messages: Vec::new(),
            tools: Vec::new(),
            media: Vec::new(),
            max_new_tokens: default_max_new_tokens(),
            grammar: None,
            temperature: None,
            top_p: None,
            top_k: None,
            min_p: None,
            repeat_penalty: None,
            frequency_penalty: None,
            presence_penalty: None,
            stop: Vec::new(),
            seed: None,
            ignore_eos: false,
            speciality_parameters: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_max_new_tokens(mut self, max_new_tokens: u32) -> Self {
        self.max_new_tokens = max_new_tokens;
        self
    }

    #[must_use]
    pub fn with_grammar(mut self, grammar: GrammarSpec) -> Self {
        self.grammar = Some(grammar);
        self
    }

    #[must_use]
    pub fn with_ignore_eos(mut self, ignore_eos: bool) -> Self {
        self.ignore_eos = ignore_eos;
        self
    }

    pub fn validate_sampling(&self) -> Result<()> {
        if self.speciality_parameters.len() > 16 {
            return Err(EngineError::InvalidConfig(
                "at most 16 speciality parameters may be supplied".to_owned(),
            ));
        }
        let mut speciality_names = std::collections::BTreeSet::new();
        for speciality in &self.speciality_parameters {
            if !valid_speciality_name(&speciality.name)
                || !valid_speciality_name(&speciality.level)
                || !valid_speciality_name(&speciality.native_path)
                || !speciality_names.insert(speciality.name.as_str())
            {
                return Err(EngineError::InvalidConfig(
                    "speciality parameters require unique safe names, levels, and native paths"
                        .to_owned(),
                ));
            }
        }
        if self
            .temperature
            .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
        {
            return Err(EngineError::InvalidConfig(
                "temperature must be finite and between 0 and 2".to_owned(),
            ));
        }
        if self
            .top_p
            .is_some_and(|value| !value.is_finite() || value <= 0.0 || value > 1.0)
        {
            return Err(EngineError::InvalidConfig(
                "top_p must be finite and in (0, 1]".to_owned(),
            ));
        }
        if self
            .top_k
            .is_some_and(|value| !(0..=1_000_000).contains(&value))
        {
            return Err(EngineError::InvalidConfig(
                "top_k must be between 0 and 1000000".to_owned(),
            ));
        }
        if self
            .min_p
            .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
        {
            return Err(EngineError::InvalidConfig(
                "min_p must be finite and between 0 and 1".to_owned(),
            ));
        }
        if self
            .repeat_penalty
            .is_some_and(|value| !value.is_finite() || value <= 0.0 || value > 10.0)
        {
            return Err(EngineError::InvalidConfig(
                "repeat_penalty must be finite and in (0, 10]".to_owned(),
            ));
        }
        if self
            .frequency_penalty
            .is_some_and(|value| !value.is_finite() || !(-2.0..=2.0).contains(&value))
        {
            return Err(EngineError::InvalidConfig(
                "frequency_penalty must be finite and between -2 and 2".to_owned(),
            ));
        }
        if self
            .presence_penalty
            .is_some_and(|value| !value.is_finite() || !(-2.0..=2.0).contains(&value))
        {
            return Err(EngineError::InvalidConfig(
                "presence_penalty must be finite and between -2 and 2".to_owned(),
            ));
        }
        if self.stop.len() > 4
            || self
                .stop
                .iter()
                .any(|value| value.is_empty() || value.chars().count() > 1_024)
        {
            return Err(EngineError::InvalidConfig(
                "stop must contain at most 4 non-empty strings of at most 1024 characters"
                    .to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GrammarSpec {
    Gbnf { grammar: String, root: String },
    JsonSchema { schema: Value },
    ToolCall { tools: Vec<ToolSpec> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_tool_parameters")]
    pub parameters: Value,
}

impl ToolSpec {
    pub fn new(name: impl Into<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description: None,
            parameters,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TokenChunk {
    pub index: u32,
    pub token_id: i32,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactChunk {
    pub artifact_id: String,
    pub index: u32,
    pub content_type: String,
    pub bytes: Vec<u8>,
    #[serde(default)]
    pub final_chunk: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GenerateOutput {
    pub text: String,
    pub usage: UsageCounters,
    pub finish_reason: FinishReason,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingOutput {
    pub embeddings: Vec<Vec<f32>>,
    pub usage: UsageCounters,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImageGenerationOutput {
    pub image_count: u32,
    pub steps: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioTranscriptionRequest {
    pub audio: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioTranscriptionOutput {
    pub text: String,
    pub audio_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_language: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<AudioTranscriptionTimestamp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<AudioTranscriptionTimestamp>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioTranscriptionTimestamp {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpeechRequest {
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpeechOutput {
    pub audio_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaGenerationRequest {
    pub endpoint_family: String,
    pub prompt: String,
    pub request: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,
}

impl MediaGenerationRequest {
    pub fn validate(&self) -> Result<()> {
        if self.endpoint_family.trim().is_empty() {
            return Err(EngineError::InvalidConfig(
                "media generation endpoint_family must not be empty".to_owned(),
            ));
        }
        if self.prompt.trim().is_empty() && !music_request_has_validated_task_input(self) {
            return Err(EngineError::InvalidConfig(
                "media generation prompt must not be empty".to_owned(),
            ));
        }
        if !self.request.is_object() {
            return Err(EngineError::InvalidConfig(
                "media generation request must be an object".to_owned(),
            ));
        }
        for (field, value) in [
            ("duration_seconds", self.duration_seconds),
            ("frame_count", self.frame_count),
            ("step_count", self.step_count),
        ] {
            if value == Some(0) {
                return Err(EngineError::InvalidConfig(format!(
                    "media generation {field} must be positive when supplied"
                )));
            }
        }
        Ok(())
    }
}

fn music_request_has_validated_task_input(request: &MediaGenerationRequest) -> bool {
    if request.endpoint_family != "mayhem_music_generations" {
        return false;
    }
    let Some(body) = request.request.as_object() else {
        return false;
    };
    let nonempty_text = |field: &str| {
        body.get(field)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    };
    let has_source_audio = ["source_audio", "src_audio", "input_audio", "audio"]
        .into_iter()
        .any(|field| {
            body.get(field).is_some_and(|value| match value {
                Value::String(encoded) => !encoded.trim().is_empty(),
                Value::Object(descriptor) => descriptor
                    .get("data")
                    .and_then(Value::as_str)
                    .is_some_and(|encoded| !encoded.trim().is_empty()),
                _ => false,
            })
        });
    match body
        .get("task_type")
        .and_then(Value::as_str)
        .unwrap_or("text2music")
    {
        "text2music" => nonempty_text("caption") || nonempty_text("lyrics"),
        "cover" | "cover-nofsq" | "repaint" => has_source_audio,
        _ => false,
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MediaGenerationOutput {
    pub duration_seconds: u64,
    pub frame_count: u64,
    pub step_count: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaGenerationValidation {
    pub evidence: Value,
    pub handled_request_attributes: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct UsageCounters {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(default)]
    pub reasoning_tokens: u32,
    #[serde(default)]
    pub vision_tokens: u32,
    #[serde(default)]
    pub audio_tokens: u32,
}

impl UsageCounters {
    #[must_use]
    pub fn new(prompt_tokens: u32, completion_tokens: u32) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens.saturating_add(completion_tokens),
            reasoning_tokens: 0,
            vision_tokens: 0,
            audio_tokens: 0,
        }
    }
}

fn valid_speciality_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
}

impl fmt::Display for FinishReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stop => f.write_str("stop"),
            Self::Length => f.write_str("length"),
        }
    }
}

pub trait TokenSink {
    fn on_token(&mut self, chunk: TokenChunk) -> Result<()>;
}

impl<F> TokenSink for F
where
    F: FnMut(TokenChunk) -> Result<()>,
{
    fn on_token(&mut self, chunk: TokenChunk) -> Result<()> {
        self(chunk)
    }
}

#[derive(Debug, Default)]
pub struct NoopTokenSink;

impl TokenSink for NoopTokenSink {
    fn on_token(&mut self, _chunk: TokenChunk) -> Result<()> {
        Ok(())
    }
}

pub trait ArtifactSink {
    fn on_artifact_chunk(&mut self, chunk: ArtifactChunk) -> Result<()>;
}

impl<F> ArtifactSink for F
where
    F: FnMut(ArtifactChunk) -> Result<()>,
{
    fn on_artifact_chunk(&mut self, chunk: ArtifactChunk) -> Result<()> {
        self(chunk)
    }
}

#[derive(Debug, Default)]
pub struct NoopArtifactSink;

impl ArtifactSink for NoopArtifactSink {
    fn on_artifact_chunk(&mut self, _chunk: ArtifactChunk) -> Result<()> {
        Ok(())
    }
}

pub trait EngineBackend {
    fn backend_id(&self) -> &'static str;
    fn load(&mut self, config: LoadConfig) -> Result<LoadedModelInfo>;
    fn loaded_backend_evidence(&self) -> Option<Value> {
        None
    }
    fn component_healthy(&mut self) -> bool {
        true
    }
    fn process_ids(&self) -> Vec<u32> {
        Vec::new()
    }
    fn tokenize(&self, text: &str) -> Result<Tokenization>;
    fn generate(
        &mut self,
        request: GenerateRequest,
        sink: &mut dyn TokenSink,
        cancellation: &CancellationToken,
    ) -> Result<GenerateOutput>;
    fn generate_with_artifacts(
        &mut self,
        request: GenerateRequest,
        token_sink: &mut dyn TokenSink,
        _artifact_sink: &mut dyn ArtifactSink,
        cancellation: &CancellationToken,
    ) -> Result<GenerateOutput> {
        self.generate(request, token_sink, cancellation)
    }
    fn embed(
        &mut self,
        _request: EmbeddingRequest,
        _cancellation: &CancellationToken,
    ) -> Result<EmbeddingOutput> {
        Err(EngineError::InvalidConfig(format!(
            "{} backend does not support embeddings",
            self.backend_id()
        )))
    }
    fn generate_image(
        &mut self,
        _request: ImageGenerationRequest,
        _artifact_sink: &mut dyn ArtifactSink,
        _cancellation: &CancellationToken,
    ) -> Result<ImageGenerationOutput> {
        Err(EngineError::InvalidConfig(format!(
            "{} backend does not support image generation",
            self.backend_id()
        )))
    }
    fn transcribe(
        &mut self,
        _request: AudioTranscriptionRequest,
        _cancellation: &CancellationToken,
    ) -> Result<AudioTranscriptionOutput> {
        Err(EngineError::InvalidConfig(format!(
            "{} backend does not support audio transcription",
            self.backend_id()
        )))
    }
    fn synthesize_speech(
        &mut self,
        _request: SpeechRequest,
        _artifact_sink: &mut dyn ArtifactSink,
        _cancellation: &CancellationToken,
    ) -> Result<SpeechOutput> {
        Err(EngineError::InvalidConfig(format!(
            "{} backend does not support speech synthesis",
            self.backend_id()
        )))
    }
    fn generate_video(
        &mut self,
        _request: MediaGenerationRequest,
        _artifact_sink: &mut dyn ArtifactSink,
        _cancellation: &CancellationToken,
    ) -> Result<MediaGenerationOutput> {
        Err(EngineError::InvalidConfig(format!(
            "{} backend does not support video generation",
            self.backend_id()
        )))
    }
    fn generate_audio(
        &mut self,
        _request: MediaGenerationRequest,
        _artifact_sink: &mut dyn ArtifactSink,
        _cancellation: &CancellationToken,
    ) -> Result<MediaGenerationOutput> {
        Err(EngineError::InvalidConfig(format!(
            "{} backend does not support general audio generation",
            self.backend_id()
        )))
    }
    fn generate_music(
        &mut self,
        _request: MediaGenerationRequest,
        _artifact_sink: &mut dyn ArtifactSink,
        _cancellation: &CancellationToken,
    ) -> Result<MediaGenerationOutput> {
        Err(EngineError::InvalidConfig(format!(
            "{} backend does not support music generation",
            self.backend_id()
        )))
    }
    fn validate_media_generation(
        &mut self,
        _request: MediaGenerationRequest,
        _cancellation: &CancellationToken,
    ) -> Result<Option<MediaGenerationValidation>> {
        Ok(None)
    }
}

#[allow(dead_code)]
fn engine_worker_command(program: &Path, memory_limit_bytes: Option<u64>) -> std::process::Command {
    #[cfg(target_os = "linux")]
    {
        if let Some(bytes) = memory_limit_bytes.filter(|bytes| *bytes > 0) {
            let limit_kib = (bytes / 1024).max(1).to_string();
            let limit_bytes = bytes.to_string();
            let mut command = std::process::Command::new("sh");
            command
                .arg("-c")
                .arg(
                    r#"limit_kib="$1"
limit_bytes="$2"
shift 2
cg="/sys/fs/cgroup/mayhem-engine-$$"
if [ -f /sys/fs/cgroup/cgroup.controllers ] && mkdir "$cg" 2>/dev/null; then
  if echo "$limit_bytes" > "$cg/memory.max" 2>/dev/null; then
    export MAYHEM_ENGINE_MEMORY_LIMIT_MODE=linux-cgroup-v2
    "$@" &
    child=$!
    if echo "$child" > "$cg/cgroup.procs" 2>/dev/null; then
      cleanup() {
        kill "$child" 2>/dev/null || true
        wait "$child" 2>/dev/null || true
        rmdir "$cg" 2>/dev/null || true
      }
      trap cleanup INT TERM HUP EXIT
      wait "$child"
      status=$?
      trap - INT TERM HUP EXIT
      rmdir "$cg" 2>/dev/null || true
      exit "$status"
    fi
    kill "$child" 2>/dev/null || true
    wait "$child" 2>/dev/null || true
    rmdir "$cg" 2>/dev/null || true
    ulimit -v "$limit_kib" || exit 127
    export MAYHEM_ENGINE_MEMORY_LIMIT_MODE=linux-rlimit-as
    exec "$@"
  else
    rmdir "$cg" 2>/dev/null || true
    ulimit -v "$limit_kib" || exit 127
    export MAYHEM_ENGINE_MEMORY_LIMIT_MODE=linux-rlimit-as
  fi
else
  ulimit -v "$limit_kib" || exit 127
  export MAYHEM_ENGINE_MEMORY_LIMIT_MODE=linux-rlimit-as
fi
exec "$@""#,
                )
                .arg("mayhem-engine-containment")
                .arg(limit_kib)
                .arg(limit_bytes)
                .arg(program)
                .env("MAYHEM_ENGINE_MEMORY_LIMIT_BYTES", bytes.to_string());
            return command;
        }
    }

    let mut command = std::process::Command::new(program);
    if let Some(bytes) = memory_limit_bytes.filter(|bytes| *bytes > 0) {
        command.env("MAYHEM_ENGINE_MEMORY_LIMIT_BYTES", bytes.to_string());
        #[cfg(target_os = "macos")]
        command.env("MAYHEM_ENGINE_MEMORY_LIMIT_MODE", "macos-provider-watchdog");
        #[cfg(target_os = "windows")]
        command.env(
            "MAYHEM_ENGINE_MEMORY_LIMIT_MODE",
            "windows-job-object-managed-by-provider",
        );
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        command.env("MAYHEM_ENGINE_MEMORY_LIMIT_MODE", "provider-watchdog");
    }
    command
}

#[cfg(target_os = "windows")]
#[derive(Debug, Default)]
#[allow(dead_code)]
struct WorkerContainment {
    _job: Option<win32job::Job>,
}

#[cfg(any(target_os = "windows", test))]
const WINDOWS_MINIMUM_WORKING_SET_BYTES: usize = 1024 * 1024;

#[cfg(any(target_os = "windows", test))]
fn windows_worker_working_set_bounds(bytes: u64) -> Result<(usize, usize)> {
    let maximum = usize::try_from(bytes).map_err(|_| {
        EngineError::InvalidConfig(format!(
            "Windows worker memory limit {bytes} exceeds this process address size"
        ))
    })?;
    if maximum < WINDOWS_MINIMUM_WORKING_SET_BYTES {
        return Err(EngineError::InvalidConfig(format!(
            "Windows worker memory limit {bytes} is below the minimum supported working set of {WINDOWS_MINIMUM_WORKING_SET_BYTES} bytes"
        )));
    }
    Ok((WINDOWS_MINIMUM_WORKING_SET_BYTES, maximum))
}

#[cfg(not(target_os = "windows"))]
#[derive(Debug, Default)]
#[allow(dead_code)]
struct WorkerContainment;

#[cfg(target_os = "windows")]
#[allow(dead_code)]
fn attach_worker_containment(
    child: &std::process::Child,
    memory_limit_bytes: Option<u64>,
) -> Result<WorkerContainment> {
    use std::os::windows::io::AsRawHandle;

    let Some(bytes) = memory_limit_bytes.filter(|bytes| *bytes > 0) else {
        return Ok(WorkerContainment::default());
    };
    let (minimum, maximum) = windows_worker_working_set_bounds(bytes)?;
    let mut limit = win32job::ExtendedLimitInfo::new();
    limit
        .limit_working_memory(minimum, maximum)
        .limit_kill_on_job_close();
    let job = win32job::Job::create_with_limit_info(&limit).map_err(|error| {
        EngineError::Io(std::io::Error::other(format!(
            "creating Windows worker job with working set {minimum}..={maximum} bytes failed: {}",
            std::io::Error::from(error)
        )))
    })?;
    job.assign_process(child.as_raw_handle() as isize)
        .map_err(|error| {
            EngineError::Io(std::io::Error::other(format!(
                "assigning worker process to Windows containment job failed: {}",
                std::io::Error::from(error)
            )))
        })?;
    Ok(WorkerContainment { _job: Some(job) })
}

#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
fn attach_worker_containment(
    _child: &std::process::Child,
    _memory_limit_bytes: Option<u64>,
) -> Result<WorkerContainment> {
    Ok(WorkerContainment)
}

pub fn tool_call_json_schema(tools: &[ToolSpec]) -> Result<Value> {
    if tools.is_empty() {
        return Err(EngineError::InvalidConfig(
            "tool-call grammar requires at least one tool".to_owned(),
        ));
    }

    let mut names = Vec::with_capacity(tools.len());
    for tool in tools {
        validate_tool_name(&tool.name)?;
        names.push(Value::String(tool.name.clone()));
    }

    Ok(json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "MayhemToolCall",
        "type": "object",
        "additionalProperties": false,
        "required": ["tool", "arguments"],
        "properties": {
            "tool": { "type": "string", "enum": names },
            "arguments": { "type": "object" },
        },
    }))
}

pub fn tool_call_gbnf(tools: &[ToolSpec]) -> Result<String> {
    if tools.is_empty() {
        return Err(EngineError::InvalidConfig(
            "tool-call grammar requires at least one tool".to_owned(),
        ));
    }

    let names = tools
        .iter()
        .map(|tool| {
            validate_tool_name(&tool.name)?;
            Ok(format!("\"\\\"{}\\\"\"", tool.name))
        })
        .collect::<Result<Vec<_>>>()?
        .join(" | ");

    Ok(format!(
        r#"root ::= "{{" ws "\"tool\"" ws ":" ws tool-name ws "," ws "\"arguments\"" ws ":" ws object ws "}}" ws
tool-name ::= {names}
object ::= "{{" ws (member (ws "," ws member)*)? ws "}}"
member ::= string ws ":" ws value
array ::= "[" ws (value (ws "," ws value)*)? ws "]"
value ::= object | array | string | number | "true" | "false" | "null"
string ::= "\"" char* "\""
char ::= [^"\\] | "\\" (["\\/bfnrt] | "u" [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F])
number ::= "-"? ([0-9] | [1-9] [0-9]*) ("." [0-9]+)? ([eE] [-+]? [0-9]+)?
ws ::= [ \t\n\r]*"#
    ))
}

fn validate_tool_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(EngineError::InvalidConfig(
            "tool names cannot be empty".to_owned(),
        ));
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':' | '/'))
    {
        return Err(EngineError::InvalidConfig(format!(
            "tool name {name:?} contains unsupported characters"
        )));
    }
    Ok(())
}

pub fn verify_artifact(artifact: &ModelArtifact) -> Result<()> {
    if !artifact.path.exists() {
        return Err(EngineError::ModelPathMissing(artifact.path.clone()));
    }

    let hash_path = match artifact.format {
        ArtifactFormat::Gguf => {
            verify_magic_header(&artifact.path, &artifact.format)?;
            artifact.path.clone()
        }
        ArtifactFormat::MlxSafetensors => {
            let weights = mlx_weights_path(&artifact.path)?;
            verify_safetensors_header_as(&weights, artifact.format.label())?;
            weights
        }
        ArtifactFormat::TensorRtLlmCheckpoint => {
            let payload = trt_llm_payload_path(&artifact.path)?;
            if payload.extension().is_some_and(|ext| ext == "safetensors") {
                verify_safetensors_header_as(&payload, artifact.format.label())?;
            }
            payload
        }
        ArtifactFormat::VllmSafetensors => {
            let payload = vllm_safetensors_payload_path(&artifact.path)?;
            verify_safetensors_header_as(&payload, artifact.format.label())?;
            payload
        }
        ArtifactFormat::TransformersSafetensors => {
            let payload = transformers_safetensors_payload_path(&artifact.path)?;
            verify_safetensors_header_as(&payload, artifact.format.label())?;
            payload
        }
        ArtifactFormat::AceStepSafetensors => {
            if !artifact.path.is_file() {
                return Err(EngineError::InvalidConfig(format!(
                    "ACE-Step primary artifact {} must be a safetensors file",
                    artifact.path.display()
                )));
            }
            verify_safetensors_header_as(&artifact.path, artifact.format.label())?;
            artifact.path.clone()
        }
        ArtifactFormat::StableDiffusionCheckpoint => {
            let payload = stable_diffusion_payload_path(&artifact.path)?;
            if payload.extension().is_some_and(|ext| ext == "safetensors") {
                verify_safetensors_header_as(&payload, artifact.format.label())?;
            } else if payload.extension().is_some_and(|ext| ext == "gguf") {
                verify_magic_header(&payload, &ArtifactFormat::Gguf)?;
            }
            payload
        }
        ArtifactFormat::WhisperGgml => whisper_ggml_payload_path(&artifact.path)?,
        ArtifactFormat::PiperVoice => piper_voice_model_path(&artifact.path)?,
        ArtifactFormat::PiperConfig => {
            if !artifact.path.is_file() {
                return Err(EngineError::InvalidConfig(format!(
                    "Piper config {} is not a file",
                    artifact.path.display()
                )));
            }
            artifact.path.clone()
        }
    };

    if let Some(expected) = &artifact.sha256 {
        let actual = file_sha256_hex(&hash_path)?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(EngineError::ArtifactHashMismatch {
                path: hash_path,
                expected: expected.clone(),
                actual,
            });
        }
    }

    Ok(())
}

fn validate_load_config(config: &LoadConfig) -> Result<()> {
    if config.artifact.path.as_os_str().is_empty() {
        return Err(EngineError::InvalidConfig(
            "artifact path cannot be empty".to_owned(),
        ));
    }
    if config.ctx_size == 0 {
        return Err(EngineError::InvalidConfig(
            "ctx_size must be greater than zero".to_owned(),
        ));
    }
    if config.batch_size == 0 {
        return Err(EngineError::InvalidConfig(
            "batch_size must be greater than zero".to_owned(),
        ));
    }
    if config.ubatch_size == 0 {
        return Err(EngineError::InvalidConfig(
            "ubatch_size must be greater than zero".to_owned(),
        ));
    }

    let kv_fields = [
        config.kv_cache_dtype.is_some(),
        config.kv_cache_bits.is_some(),
        config.kv_cache_group_size.is_some(),
        config.kv_cache_quantized_start_tokens.is_some(),
    ];
    if kv_fields.iter().any(|value| *value) && !kv_fields.iter().all(|value| *value) {
        return Err(EngineError::InvalidConfig(
            "KV-cache runtime configuration must include dtype, bits, group size, and quantized start"
                .to_owned(),
        ));
    }
    if config
        .kv_cache_bits
        .is_some_and(|bits| bits == 0 || bits > 32)
    {
        return Err(EngineError::InvalidConfig(
            "KV-cache bits must be between 1 and 32".to_owned(),
        ));
    }
    if config.kv_cache_group_size == Some(0) {
        return Err(EngineError::InvalidConfig(
            "KV-cache group size must be greater than zero".to_owned(),
        ));
    }
    if config.mlx_runtime.multimodal && config.artifact.format != ArtifactFormat::MlxSafetensors {
        return Err(EngineError::InvalidConfig(
            "MLX multimodal runtime semantics require an MLX safetensors artifact".to_owned(),
        ));
    }

    Ok(())
}

fn verify_magic_header(path: &PathBuf, format: &ArtifactFormat) -> Result<()> {
    let mut file = File::open(path)?;
    let mut actual = vec![0_u8; format.magic().len()];
    file.read_exact(&mut actual)?;
    if actual.as_slice() != format.magic() {
        return Err(EngineError::UnsupportedArtifactHeader {
            path: path.clone(),
            expected: format.label(),
            actual,
        });
    }
    Ok(())
}

fn mlx_weights_path(path: &Path) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if !path.is_dir() {
        return Err(EngineError::ModelPathMissing(path.to_path_buf()));
    }

    let default = path.join("model.safetensors");
    if default.is_file() {
        return Ok(default);
    }

    let mut candidates = std::fs::read_dir(path)?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "safetensors"))
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next().ok_or_else(|| {
        EngineError::InvalidConfig(format!(
            "MLX artifact directory {} contains no .safetensors weights",
            path.display()
        ))
    })
}

fn verify_safetensors_header_as(path: &PathBuf, expected: &'static str) -> Result<()> {
    let mut file = File::open(path)?;
    let mut header_len_bytes = [0_u8; 8];
    file.read_exact(&mut header_len_bytes)
        .map_err(|_| EngineError::UnsupportedArtifactHeader {
            path: path.clone(),
            expected,
            actual: Vec::new(),
        })?;
    let header_len = u64::from_le_bytes(header_len_bytes);
    let file_len = file.metadata()?.len();
    if header_len == 0 || header_len > file_len.saturating_sub(8) || header_len > 256 * 1024 * 1024
    {
        return Err(EngineError::UnsupportedArtifactHeader {
            path: path.clone(),
            expected,
            actual: header_len_bytes.to_vec(),
        });
    }

    let mut header = vec![
        0_u8;
        usize::try_from(header_len).map_err(|err| {
            EngineError::InvalidConfig(format!("safetensors header length overflow: {err}"))
        })?
    ];
    file.read_exact(&mut header)?;
    let header: Value = serde_json::from_slice(&header)?;
    if !header.is_object() {
        return Err(EngineError::UnsupportedArtifactHeader {
            path: path.clone(),
            expected,
            actual: header_len_bytes.to_vec(),
        });
    }
    Ok(())
}

fn trt_llm_payload_path(path: &Path) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if !path.is_dir() {
        return Err(EngineError::ModelPathMissing(path.to_path_buf()));
    }

    let config = path.join("config.json");
    if config.is_file() {
        let config_value: Value = serde_json::from_reader(File::open(&config)?)?;
        if !config_value.is_object() {
            return Err(EngineError::InvalidConfig(format!(
                "TensorRT-LLM checkpoint config {} is not a JSON object",
                config.display()
            )));
        }
    }

    let mut candidates = std::fs::read_dir(path)?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| matches!(ext.to_str(), Some("safetensors" | "engine" | "plan")))
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next().ok_or_else(|| {
        EngineError::InvalidConfig(format!(
            "TensorRT-LLM artifact directory {} contains no .safetensors, .engine, or .plan payload",
            path.display()
        ))
    })
}

fn vllm_safetensors_payload_path(path: &Path) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if !path.is_dir() {
        return Err(EngineError::ModelPathMissing(path.to_path_buf()));
    }

    let config = path.join("config.json");
    if config.is_file() {
        let config_value: Value = serde_json::from_reader(File::open(&config)?)?;
        if !config_value.is_object() {
            return Err(EngineError::InvalidConfig(format!(
                "vLLM checkpoint config {} is not a JSON object",
                config.display()
            )));
        }
    }

    for name in ["model.safetensors", "model-00001-of-00001.safetensors"] {
        let candidate = path.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    let mut candidates = std::fs::read_dir(path)?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "safetensors"))
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next().ok_or_else(|| {
        EngineError::InvalidConfig(format!(
            "vLLM artifact directory {} contains no .safetensors payload",
            path.display()
        ))
    })
}

fn transformers_safetensors_payload_path(path: &Path) -> Result<PathBuf> {
    let model_dir = if path.is_file() {
        path.parent().ok_or_else(|| {
            EngineError::InvalidConfig(format!(
                "Transformers weights path {} has no parent",
                path.display()
            ))
        })?
    } else if path.is_dir() {
        path
    } else {
        return Err(EngineError::ModelPathMissing(path.to_path_buf()));
    };

    for sidecar in [
        "config.json",
        "processor_config.json",
        "tokenizer.json",
        "tokenizer_config.json",
    ] {
        let candidate = model_dir.join(sidecar);
        if !candidate.is_file() {
            return Err(EngineError::InvalidConfig(format!(
                "Transformers ASR artifact {} is missing required sidecar {sidecar}",
                model_dir.display()
            )));
        }
        let value: Value = serde_json::from_reader(File::open(&candidate)?)?;
        if !value.is_object() {
            return Err(EngineError::InvalidConfig(format!(
                "Transformers ASR sidecar {} is not a JSON object",
                candidate.display()
            )));
        }
    }

    if path.is_file()
        && path
            .extension()
            .is_some_and(|extension| extension == "safetensors")
    {
        return Ok(path.to_path_buf());
    }
    for name in ["model.safetensors", "model-00001-of-00001.safetensors"] {
        let candidate = model_dir.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    let mut candidates = std::fs::read_dir(model_dir)?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|candidate| {
            candidate
                .extension()
                .is_some_and(|extension| extension == "safetensors")
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next().ok_or_else(|| {
        EngineError::InvalidConfig(format!(
            "Transformers ASR artifact {} contains no .safetensors weights",
            model_dir.display()
        ))
    })
}

fn stable_diffusion_payload_path(path: &Path) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if !path.is_dir() {
        return Err(EngineError::ModelPathMissing(path.to_path_buf()));
    }

    for name in [
        "model.safetensors",
        "model.gguf",
        "sd-turbo.safetensors",
        "diffusion_pytorch_model.safetensors",
    ] {
        let candidate = path.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    let mut candidates = std::fs::read_dir(path)?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| matches!(ext.to_str(), Some("safetensors" | "gguf" | "ckpt")))
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next().ok_or_else(|| {
        EngineError::InvalidConfig(format!(
            "stable-diffusion artifact directory {} contains no .safetensors, .gguf, or .ckpt payload",
            path.display()
        ))
    })
}

fn whisper_ggml_payload_path(path: &Path) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if !path.is_dir() {
        return Err(EngineError::ModelPathMissing(path.to_path_buf()));
    }

    for name in ["ggml-tiny.en.bin", "ggml-tiny.bin", "model.bin"] {
        let candidate = path.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    let mut candidates = std::fs::read_dir(path)?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "bin"))
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next().ok_or_else(|| {
        EngineError::InvalidConfig(format!(
            "whisper.cpp artifact directory {} contains no .bin payload",
            path.display()
        ))
    })
}

#[derive(Clone, Debug)]
struct PiperVoicePaths {
    model: PathBuf,
    config: PathBuf,
}

fn piper_voice_paths(path: &Path) -> Result<PiperVoicePaths> {
    let model = piper_voice_model_path(path)?;
    piper_voice_paths_from_model(model)
}

fn piper_voice_model_path(path: &Path) -> Result<PathBuf> {
    if path.is_file() {
        Ok(path.to_path_buf())
    } else if path.is_dir() {
        for name in ["model.onnx", "voice.onnx"] {
            let candidate = path.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        let mut candidates = std::fs::read_dir(path)?
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "onnx"))
            .collect::<Vec<_>>();
        candidates.sort();
        Ok(candidates.into_iter().next().ok_or_else(|| {
            EngineError::InvalidConfig(format!(
                "Piper voice directory {} contains no .onnx model",
                path.display()
            ))
        })?)
    } else {
        Err(EngineError::ModelPathMissing(path.to_path_buf()))
    }
}

fn piper_voice_paths_from_model(model: PathBuf) -> Result<PiperVoicePaths> {
    let onnx_json = model.with_extension("onnx.json");
    let stem_json = model.with_extension("json");
    let config = if onnx_json.is_file() {
        onnx_json
    } else if stem_json.is_file() {
        stem_json
    } else {
        return Err(EngineError::InvalidConfig(format!(
            "Piper voice {} is missing config sidecar (expected {} or {})",
            model.display(),
            onnx_json.display(),
            stem_json.display()
        )));
    };
    Ok(PiperVoicePaths { model, config })
}

fn file_sha256_hex(path: &PathBuf) -> Result<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

fn wav_duration_seconds_ceil(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut offset = 12usize;
    let mut sample_rate = None;
    let mut channels = None;
    let mut bits_per_sample = None;
    let mut data_len = None;
    while offset.saturating_add(8) <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_len = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        let chunk_start = offset + 8;
        let chunk_end = chunk_start.saturating_add(chunk_len).min(bytes.len());
        if chunk_id == b"fmt " && chunk_len >= 16 && chunk_end <= bytes.len() {
            channels = Some(u16::from_le_bytes([
                bytes[chunk_start + 2],
                bytes[chunk_start + 3],
            ]));
            sample_rate = Some(u32::from_le_bytes([
                bytes[chunk_start + 4],
                bytes[chunk_start + 5],
                bytes[chunk_start + 6],
                bytes[chunk_start + 7],
            ]));
            bits_per_sample = Some(u16::from_le_bytes([
                bytes[chunk_start + 14],
                bytes[chunk_start + 15],
            ]));
        } else if chunk_id == b"data" {
            data_len = Some(chunk_len);
        }
        let padded = chunk_len + (chunk_len % 2);
        offset = chunk_start.saturating_add(padded);
    }
    let sample_rate = u64::from(sample_rate?);
    let channels = u64::from(channels?);
    let bits_per_sample = u64::from(bits_per_sample?);
    let data_len = u64::try_from(data_len?).ok()?;
    let bytes_per_second = sample_rate
        .saturating_mul(channels)
        .saturating_mul(bits_per_sample)
        .checked_div(8)?;
    if bytes_per_second == 0 {
        return None;
    }
    Some(data_len.div_ceil(bytes_per_second).max(1))
}

fn default_context_size() -> u32 {
    DEFAULT_CONTEXT_SIZE
}

fn default_batch_size() -> u32 {
    DEFAULT_BATCH_SIZE
}

fn default_ubatch_size() -> u32 {
    DEFAULT_UBATCH_SIZE
}

fn default_max_new_tokens() -> u32 {
    64
}

fn default_image_count() -> u32 {
    1
}

fn default_image_width() -> u32 {
    512
}

fn default_image_height() -> u32 {
    512
}

fn default_image_steps() -> u32 {
    1
}

fn default_image_guidance_scale() -> f32 {
    1.0
}

fn default_true() -> bool {
    true
}

fn default_tool_parameters() -> Value {
    json!({
        "type": "object",
        "additionalProperties": true,
    })
}

#[cfg(feature = "llama-cpp")]
pub use llama_cpp_backend::LlamaCppBackend;

#[cfg(feature = "mlx")]
pub use mlx_backend::MlxBackend;

#[cfg(feature = "trt-llm")]
pub use trt_llm_backend::TrtLlmBackend;

#[cfg(feature = "vllm")]
pub use vllm_backend::VllmBackend;

#[cfg(feature = "transformers-asr")]
pub use transformers_asr_backend::TransformersAsrBackend;

#[cfg(feature = "ace-step")]
mod ace_step_backend;

#[cfg(feature = "ace-step")]
pub use ace_step_backend::{
    ensure_ace_step_source, AceStepBackend, AceStepExecutionConfig, ACE_STEP_SOURCE_COMMIT,
    ACE_STEP_SOURCE_SHA256,
};

#[cfg(feature = "vllm")]
pub fn discover_vllm_cuda_home(python: &Path) -> Option<PathBuf> {
    vllm_backend::resolve_vllm_cuda_home(python)
}

#[cfg(feature = "trt-llm")]
pub fn discover_trt_llm_cuda_home(python: &Path) -> Option<PathBuf> {
    trt_llm_backend::resolve_trt_cuda_home(python)
}

#[cfg(any(feature = "trt-llm", feature = "vllm"))]
fn select_runtime_compatible_cuda_home(
    python: &Path,
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Option<PathBuf> {
    let mut candidates = candidates
        .into_iter()
        .filter(|path| path.join("bin/nvcc").is_file())
        .collect::<Vec<_>>();
    candidates.dedup();

    let Some(runtime_version) = python_torch_cuda_version(python) else {
        return candidates.into_iter().next();
    };
    candidates
        .into_iter()
        .find(|path| nvcc_cuda_version(path) == Some(runtime_version))
}

#[cfg(any(feature = "trt-llm", feature = "vllm"))]
fn python_torch_cuda_version(python: &Path) -> Option<(u32, u32)> {
    let venv = python.parent()?.parent()?;
    let mut roots = vec![venv.join("Lib/site-packages")];
    for lib in [venv.join("lib"), venv.join("lib64")] {
        roots.extend(
            std::fs::read_dir(lib)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(std::result::Result::ok)
                .map(|entry| entry.path().join("site-packages")),
        );
    }
    roots.into_iter().find_map(|root| {
        let version = std::fs::read_to_string(root.join("torch/version.py")).ok()?;
        version
            .lines()
            .find(|line| line.trim_start().starts_with("cuda"))
            .and_then(parse_cuda_major_minor)
    })
}

#[cfg(any(feature = "trt-llm", feature = "vllm"))]
fn nvcc_cuda_version(cuda_home: &Path) -> Option<(u32, u32)> {
    let output = std::process::Command::new(cuda_home.join("bin/nvcc"))
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    stdout
        .split_once("release ")
        .and_then(|(_, version)| parse_cuda_major_minor(version))
}

#[cfg(any(feature = "trt-llm", feature = "vllm"))]
fn parse_cuda_major_minor(value: &str) -> Option<(u32, u32)> {
    let start = value.find(|ch: char| ch.is_ascii_digit())?;
    let mut parts = value[start..].splitn(2, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts
        .next()?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    Some((major, minor))
}

pub use piper_backend::PiperBackend;
pub use stable_diffusion_cpp_backend::StableDiffusionCppBackend;
pub use whisper_cpp_backend::WhisperCppBackend;

#[cfg(feature = "transformers-asr")]
mod transformers_asr_backend {
    use super::{
        attach_worker_containment, engine_worker_command, transformers_safetensors_payload_path,
        validate_load_config, verify_artifact, ArtifactFormat, AudioTranscriptionOutput,
        AudioTranscriptionRequest, CancellationToken, EngineBackend, EngineError, GenerateOutput,
        GenerateRequest, LoadConfig, LoadedModelInfo, Result, TokenSink, Tokenization,
        WorkerContainment,
    };
    use base64::Engine as _;
    use serde::de::DeserializeOwned;
    use serde::Deserialize;
    use serde_json::{json, Value};
    use std::env;
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, ChildStdout, Stdio};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    const WORKER: &str = include_str!("transformers_asr_worker.py");
    const PYTHON_ENV: &str = "MAYHEM_TRANSFORMERS_ASR_PYTHON";

    pub struct TransformersAsrBackend {
        python: PathBuf,
        worker: Option<TransformersAsrWorker>,
        loaded: Option<LoadedModelInfo>,
        config: Option<LoadConfig>,
        next_id: u64,
    }

    impl TransformersAsrBackend {
        pub fn new() -> Result<Self> {
            let python = env::var_os(PYTHON_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("python3"));
            Self::with_python(python)
        }

        pub fn with_python(python: impl Into<PathBuf>) -> Result<Self> {
            Ok(Self {
                python: python.into(),
                worker: None,
                loaded: None,
                config: None,
                next_id: 1,
            })
        }

        fn ensure_worker_loaded(&mut self) -> Result<()> {
            if self.worker.is_some() {
                return Ok(());
            }
            let config = self.config.clone().ok_or(EngineError::NotLoaded)?;
            self.worker = Some(TransformersAsrWorker::spawn(
                &self.python,
                config.memory_limit_bytes,
                config.backend_cache_dir.as_deref(),
            )?);
            let model_path = transformers_model_dir(&config.artifact.path)?;
            let _: WorkerLoadInfo =
                self.call_existing("load", json!({ "path": model_path }), None)?;
            Ok(())
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
                    return Err(EngineError::TransformersAsr(format!(
                        "worker response id {} did not match request id {id}",
                        message.id
                    )));
                }
                if message.ok {
                    return Ok(serde_json::from_value(
                        message.result.unwrap_or(Value::Null),
                    )?);
                }
                return Err(EngineError::TransformersAsr(
                    message
                        .error
                        .unwrap_or_else(|| "worker returned an unknown error".to_owned()),
                ));
            }
        }

        fn worker_mut(&mut self) -> Result<&mut TransformersAsrWorker> {
            self.worker
                .as_mut()
                .ok_or_else(|| EngineError::TransformersAsr("ASR worker is not running".to_owned()))
        }

        fn stop_worker(&mut self) {
            if let Some(mut worker) = self.worker.take() {
                worker.stop();
            }
        }
    }

    impl EngineBackend for TransformersAsrBackend {
        fn backend_id(&self) -> &'static str {
            "transformers-asr"
        }

        fn load(&mut self, config: LoadConfig) -> Result<LoadedModelInfo> {
            validate_load_config(&config)?;
            if config.artifact.format != ArtifactFormat::TransformersSafetensors {
                return Err(EngineError::InvalidConfig(format!(
                    "Transformers ASR requires Transformers safetensors artifacts, got {:?}",
                    config.artifact.format
                )));
            }
            verify_artifact(&config.artifact)?;
            self.stop_worker();
            self.config = Some(config.clone());
            self.worker = Some(TransformersAsrWorker::spawn(
                &self.python,
                config.memory_limit_bytes,
                config.backend_cache_dir.as_deref(),
            )?);
            let model_path = transformers_model_dir(&config.artifact.path)?;
            let worker_info: WorkerLoadInfo =
                self.call_existing("load", json!({ "path": model_path }), None)?;
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
                "Transformers ASR transcribes audio; use transcribe".to_owned(),
            ))
        }

        fn transcribe(
            &mut self,
            request: AudioTranscriptionRequest,
            cancellation: &CancellationToken,
        ) -> Result<AudioTranscriptionOutput> {
            cancellation.check()?;
            self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
            if request.audio.is_empty() {
                return Err(EngineError::InvalidConfig(
                    "audio transcription input cannot be empty".to_owned(),
                ));
            }
            if request
                .language
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            {
                return Err(EngineError::InvalidConfig(
                    "this Transformers TDT model detects language automatically and does not accept language forcing"
                        .to_owned(),
                ));
            }
            if request
                .prompt
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            {
                return Err(EngineError::InvalidConfig(
                    "this Transformers TDT model does not support transcription prompts".to_owned(),
                ));
            }
            let audio_base64 = base64::engine::general_purpose::STANDARD.encode(request.audio);
            self.call(
                "transcribe",
                json!({
                    "audio_base64": audio_base64,
                    "content_type": request.content_type,
                }),
                Some(cancellation),
            )
        }
    }

    impl Drop for TransformersAsrBackend {
        fn drop(&mut self) {
            self.stop_worker();
        }
    }

    #[derive(Debug, Deserialize)]
    struct WorkerLoadInfo {
        n_ctx_train: u32,
        n_vocab: i32,
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

    struct TransformersAsrWorker {
        child: Child,
        _containment: WorkerContainment,
        stdin: ChildStdin,
        stdout_rx: Option<Receiver<WorkerRead>>,
        reader: Option<JoinHandle<()>>,
    }

    impl TransformersAsrWorker {
        fn spawn(
            python: &Path,
            memory_limit_bytes: Option<u64>,
            cache_root: Option<&Path>,
        ) -> Result<Self> {
            let mut command = engine_worker_command(python, memory_limit_bytes);
            configure_worker_environment(&mut command, python, cache_root)?;
            command
                .arg("-u")
                .arg("-c")
                .arg(WORKER)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit());
            let mut child = command.spawn().map_err(|error| {
                EngineError::TransformersAsr(format!(
                    "spawning Transformers ASR worker with {} failed: {error}",
                    python.display()
                ))
            })?;
            let stdin = child.stdin.take().ok_or_else(|| {
                EngineError::TransformersAsr("opening ASR worker stdin failed".to_owned())
            })?;
            let stdout = child.stdout.take().ok_or_else(|| {
                EngineError::TransformersAsr("opening ASR worker stdout failed".to_owned())
            })?;
            let containment =
                attach_worker_containment(&child, memory_limit_bytes).map_err(|error| {
                    EngineError::TransformersAsr(format!(
                        "applying ASR worker containment failed: {error}"
                    ))
                })?;
            let (stdout_tx, stdout_rx) = mpsc::sync_channel(super::WORKER_STDOUT_QUEUE_CAPACITY);
            let reader = thread::spawn(move || read_worker_stdout(stdout, stdout_tx));
            Ok(Self {
                child,
                _containment: containment,
                stdin,
                stdout_rx: Some(stdout_rx),
                reader: Some(reader),
            })
        }

        fn send(&mut self, id: u64, operation: &str, payload: Value) -> Result<()> {
            serde_json::to_writer(
                &mut self.stdin,
                &json!({
                    "id": id,
                    "op": operation,
                    "payload": payload,
                }),
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
                    EngineError::TransformersAsr("ASR worker stdout reader is closed".to_owned())
                })?
                .recv_timeout(wait)
            {
                Ok(read) => read,
                Err(RecvTimeoutError::Timeout) => return Ok(None),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(EngineError::TransformersAsr(
                        "ASR worker stdout reader stopped".to_owned(),
                    ))
                }
            };
            let line = match read {
                WorkerRead::Line(line) => line,
                WorkerRead::Eof => {
                    return Err(EngineError::TransformersAsr(
                        "ASR worker exited before replying".to_owned(),
                    ))
                }
                WorkerRead::Error(error) => return Err(EngineError::TransformersAsr(error)),
            };
            serde_json::from_str(line.trim_end())
                .map(Some)
                .map_err(Into::into)
        }

        fn stop(&mut self) {
            let _ = self.send(0, "shutdown", Value::Null);
            self.stdout_rx.take();
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }

    fn configure_worker_environment(
        command: &mut std::process::Command,
        python: &Path,
        cache_root: Option<&Path>,
    ) -> Result<()> {
        let cache_root = cache_root
            .map(Path::to_path_buf)
            .or_else(|| {
                env::var_os("MAYHEM_HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join("cache/transformers-asr"))
            })
            .or_else(|| {
                env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".mayhem/cache/transformers-asr"))
            })
            .unwrap_or_else(|| env::temp_dir().join("mayhem-transformers-asr-cache"));
        for (name, default_path) in [
            ("XDG_CACHE_HOME", cache_root.join("xdg")),
            ("HF_HOME", cache_root.join("huggingface")),
            ("HF_HUB_CACHE", cache_root.join("huggingface/hub")),
            ("TRANSFORMERS_CACHE", cache_root.join("transformers")),
        ] {
            let path = env::var_os(name).map(PathBuf::from).unwrap_or(default_path);
            fs::create_dir_all(&path).map_err(|error| {
                EngineError::TransformersAsr(format!(
                    "creating ASR cache directory {} failed: {error}",
                    path.display()
                ))
            })?;
            command.env(name, path);
        }
        command
            .env("HF_HUB_OFFLINE", "1")
            .env("HF_DATASETS_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .env("TOKENIZERS_PARALLELISM", "false");
        if let Some(python_bin) = python.parent() {
            let mut paths = vec![python_bin.to_path_buf()];
            if let Some(current) = env::var_os("PATH") {
                paths.extend(env::split_paths(&current));
            }
            if let Ok(path) = env::join_paths(paths) {
                command.env("PATH", path);
            }
        }
        Ok(())
    }

    fn transformers_model_dir(path: &Path) -> Result<PathBuf> {
        let payload = transformers_safetensors_payload_path(path)?;
        payload.parent().map(Path::to_path_buf).ok_or_else(|| {
            EngineError::InvalidConfig(format!(
                "Transformers ASR weights path {} has no parent",
                payload.display()
            ))
        })
    }

    enum WorkerRead {
        Line(String),
        Eof,
        Error(String),
    }

    fn read_worker_stdout(stdout: ChildStdout, sender: mpsc::SyncSender<WorkerRead>) {
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
                        "reading ASR worker stdout failed: {error}"
                    )));
                    return;
                }
            }
        }
    }
}

mod whisper_cpp_backend {
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        run_command_cancellable, validate_load_config, verify_artifact, wav_duration_seconds_ceil,
        whisper_ggml_payload_path, ArtifactFormat, AudioTranscriptionOutput,
        AudioTranscriptionRequest, CancellationToken, EngineBackend, EngineError, GenerateOutput,
        GenerateRequest, LoadConfig, LoadedModelInfo, Result, TokenSink, Tokenization,
    };

    static WHISPER_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[derive(Debug)]
    pub struct WhisperCppBackend {
        binary: PathBuf,
        loaded: Option<LoadedModelInfo>,
        config: Option<LoadConfig>,
    }

    impl WhisperCppBackend {
        pub fn new() -> Result<Self> {
            let binary = env::var_os("MAYHEM_WHISPER_CPP_BIN")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("whisper-cli"));
            Ok(Self::with_binary(binary))
        }

        pub fn with_binary(binary: impl Into<PathBuf>) -> Self {
            Self {
                binary: binary.into(),
                loaded: None,
                config: None,
            }
        }

        fn config(&self) -> Result<&LoadConfig> {
            self.config.as_ref().ok_or(EngineError::NotLoaded)
        }
    }

    impl EngineBackend for WhisperCppBackend {
        fn backend_id(&self) -> &'static str {
            "whisper.cpp"
        }

        fn load(&mut self, config: LoadConfig) -> Result<LoadedModelInfo> {
            validate_load_config(&config)?;
            if config.artifact.format != ArtifactFormat::WhisperGgml {
                return Err(EngineError::InvalidConfig(format!(
                    "whisper.cpp backend requires whisper ggml models, got {:?}",
                    config.artifact.format
                )));
            }
            verify_artifact(&config.artifact)?;
            let info = LoadedModelInfo {
                backend: self.backend_id().to_owned(),
                artifact: config.artifact.clone(),
                ctx_size: config.ctx_size,
                n_ctx_train: 0,
                n_vocab: 0,
            };
            self.loaded = Some(info.clone());
            self.config = Some(config);
            Ok(info)
        }

        fn tokenize(&self, text: &str) -> Result<Tokenization> {
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
                "whisper.cpp backend transcribes audio; use transcribe".to_owned(),
            ))
        }

        fn transcribe(
            &mut self,
            request: AudioTranscriptionRequest,
            cancellation: &CancellationToken,
        ) -> Result<AudioTranscriptionOutput> {
            cancellation.check()?;
            if request.audio.is_empty() {
                return Err(EngineError::InvalidConfig(
                    "audio transcription input cannot be empty".to_owned(),
                ));
            }
            let config = self.config()?.clone();
            let model_path = whisper_ggml_payload_path(&config.artifact.path)?;
            let input_path = whisper_input_path(request.content_type.as_deref());
            let output_base = whisper_output_base_path();
            fs::write(&input_path, &request.audio)?;

            let mut command = Command::new(&self.binary);
            command
                .arg("-m")
                .arg(&model_path)
                .arg("-f")
                .arg(&input_path)
                .arg("-otxt")
                .arg("-of")
                .arg(&output_base)
                .arg("-nt");
            if let Some(language) = request
                .language
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                command.arg("-l").arg(language);
            }
            if let Some(prompt) = request.prompt.as_deref().filter(|value| !value.is_empty()) {
                command.arg("--prompt").arg(prompt);
            }
            let output = run_command_cancellable(&mut command, cancellation, |err| {
                EngineError::WhisperCpp(format!("running {} failed: {err}", self.binary.display()))
            })?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                let _ = fs::remove_file(&input_path);
                return Err(EngineError::WhisperCpp(format!(
                    "{} exited with {}; stderr={stderr:?}; stdout={stdout:?}",
                    self.binary.display(),
                    output.status
                )));
            }
            let transcript_path = output_base.with_extension("txt");
            let text = fs::read_to_string(&transcript_path)
                .map_err(|err| {
                    EngineError::WhisperCpp(format!(
                        "{} did not create transcript {}: {err}",
                        self.binary.display(),
                        transcript_path.display()
                    ))
                })?
                .trim()
                .to_owned();
            let _ = fs::remove_file(&input_path);
            let _ = fs::remove_file(&transcript_path);
            if text.is_empty() {
                return Err(EngineError::WhisperCpp(
                    "whisper.cpp produced an empty transcript".to_owned(),
                ));
            }
            Ok(AudioTranscriptionOutput {
                text,
                audio_seconds: wav_duration_seconds_ceil(&request.audio).unwrap_or(1),
                duration_seconds: None,
                detected_language: request.language,
                words: Vec::new(),
                segments: Vec::new(),
            })
        }
    }

    fn whisper_input_path(content_type: Option<&str>) -> PathBuf {
        let extension = match content_type.unwrap_or("audio/wav") {
            "audio/mpeg" | "audio/mp3" => "mp3",
            "audio/flac" => "flac",
            "audio/ogg" => "ogg",
            _ => "wav",
        };
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let seq = WHISPER_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!(
            "mayhem-whisper-{}-{nanos}-{seq}.{extension}",
            std::process::id()
        ))
    }

    fn whisper_output_base_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let seq = WHISPER_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!(
            "mayhem-whisper-out-{}-{nanos}-{seq}",
            std::process::id()
        ))
    }
}

mod piper_backend {
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        piper_voice_model_path, piper_voice_paths, run_command_cancellable, validate_load_config,
        verify_artifact, wav_duration_seconds_ceil, ArtifactChunk, ArtifactFormat, ArtifactSink,
        CancellationToken, EngineBackend, EngineError, GenerateOutput, GenerateRequest, LoadConfig,
        LoadedModelInfo, PiperVoicePaths, Result, SpeechOutput, SpeechRequest, TokenSink,
        Tokenization,
    };

    static PIPER_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[derive(Debug)]
    pub struct PiperBackend {
        binary: PathBuf,
        loaded: Option<LoadedModelInfo>,
        config: Option<LoadConfig>,
    }

    impl PiperBackend {
        pub fn new() -> Result<Self> {
            let binary = env::var_os("MAYHEM_PIPER_BIN")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("piper"));
            Ok(Self::with_binary(binary))
        }

        pub fn with_binary(binary: impl Into<PathBuf>) -> Self {
            Self {
                binary: binary.into(),
                loaded: None,
                config: None,
            }
        }

        fn config(&self) -> Result<&LoadConfig> {
            self.config.as_ref().ok_or(EngineError::NotLoaded)
        }
    }

    impl EngineBackend for PiperBackend {
        fn backend_id(&self) -> &'static str {
            "piper"
        }

        fn load(&mut self, config: LoadConfig) -> Result<LoadedModelInfo> {
            validate_load_config(&config)?;
            if config.artifact.format != ArtifactFormat::PiperVoice {
                return Err(EngineError::InvalidConfig(format!(
                    "piper backend requires Piper voice artifacts, got {:?}",
                    config.artifact.format
                )));
            }
            verify_artifact(&config.artifact)?;
            if let Some(piper_config) = &config.piper_config {
                if piper_config.format != ArtifactFormat::PiperConfig {
                    return Err(EngineError::InvalidConfig(format!(
                        "piper config sidecar must use PiperConfig format, got {:?}",
                        piper_config.format
                    )));
                }
                verify_artifact(piper_config)?;
            } else {
                piper_voice_paths(&config.artifact.path)?;
            }
            let info = LoadedModelInfo {
                backend: self.backend_id().to_owned(),
                artifact: config.artifact.clone(),
                ctx_size: config.ctx_size,
                n_ctx_train: 0,
                n_vocab: 0,
            };
            self.loaded = Some(info.clone());
            self.config = Some(config);
            Ok(info)
        }

        fn tokenize(&self, text: &str) -> Result<Tokenization> {
            Ok(Tokenization {
                token_ids: text.chars().map(|ch| ch as i32).collect(),
            })
        }

        fn generate(
            &mut self,
            _request: GenerateRequest,
            _sink: &mut dyn TokenSink,
            _cancellation: &CancellationToken,
        ) -> Result<GenerateOutput> {
            Err(EngineError::InvalidConfig(
                "piper backend synthesizes speech; use synthesize_speech".to_owned(),
            ))
        }

        fn synthesize_speech(
            &mut self,
            request: SpeechRequest,
            artifact_sink: &mut dyn ArtifactSink,
            cancellation: &CancellationToken,
        ) -> Result<SpeechOutput> {
            cancellation.check()?;
            if request.input.trim().is_empty() {
                return Err(EngineError::InvalidConfig(
                    "speech synthesis input cannot be empty".to_owned(),
                ));
            }
            let format = request
                .response_format
                .as_deref()
                .unwrap_or("wav")
                .to_ascii_lowercase();
            if format != "wav" {
                return Err(EngineError::InvalidConfig(format!(
                    "piper backend currently emits wav audio, got response_format={format}"
                )));
            }
            let config = self.config()?.clone();
            let voice = if let Some(piper_config) = &config.piper_config {
                PiperVoicePaths {
                    model: piper_voice_model_path(&config.artifact.path)?,
                    config: piper_config.path.clone(),
                }
            } else {
                piper_voice_paths(&config.artifact.path)?
            };
            let input_path = piper_input_path();
            let output_path = piper_output_path();
            fs::write(&input_path, &request.input)?;

            let mut command = Command::new(&self.binary);
            command
                .arg("-m")
                .arg(&voice.model)
                .arg("-c")
                .arg(&voice.config)
                .arg("-i")
                .arg(&input_path)
                .arg("-f")
                .arg(&output_path)
                .arg("--noise-scale")
                .arg("0")
                .arg("--noise-w-scale")
                .arg("0");
            if let Some(speed) = request
                .speed
                .filter(|value| value.is_finite() && *value > 0.0)
            {
                let length_scale = (1.0 / speed).clamp(0.25, 4.0);
                command.arg("--length-scale").arg(length_scale.to_string());
            }
            let output = run_command_cancellable(&mut command, cancellation, |err| {
                EngineError::Piper(format!("running {} failed: {err}", self.binary.display()))
            })?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                let _ = fs::remove_file(&input_path);
                return Err(EngineError::Piper(format!(
                    "{} exited with {}; stderr={stderr:?}; stdout={stdout:?}",
                    self.binary.display(),
                    output.status
                )));
            }
            let bytes = fs::read(&output_path).map_err(|err| {
                EngineError::Piper(format!(
                    "{} did not create readable output {}: {err}",
                    self.binary.display(),
                    output_path.display()
                ))
            })?;
            let _ = fs::remove_file(&input_path);
            let _ = fs::remove_file(&output_path);
            if bytes.len() < 44 || !bytes.starts_with(b"RIFF") {
                return Err(EngineError::Piper(
                    "piper produced invalid or empty wav output".to_owned(),
                ));
            }
            let audio_seconds = wav_duration_seconds_ceil(&bytes).unwrap_or(1);
            artifact_sink.on_artifact_chunk(ArtifactChunk {
                artifact_id: "speech-1".to_owned(),
                index: 0,
                content_type: "audio/wav".to_owned(),
                bytes,
                final_chunk: true,
            })?;
            Ok(SpeechOutput { audio_seconds })
        }
    }

    fn piper_input_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let seq = PIPER_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!(
            "mayhem-piper-{}-{nanos}-{seq}.txt",
            std::process::id()
        ))
    }

    fn piper_output_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let seq = PIPER_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!(
            "mayhem-piper-{}-{nanos}-{seq}.wav",
            std::process::id()
        ))
    }
}

mod stable_diffusion_cpp_backend {
    use std::env;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use base64::{engine::general_purpose, Engine as _};
    use serde_json::{json, Value};

    use super::{
        stable_diffusion_payload_path, validate_load_config, verify_artifact, ArtifactChunk,
        ArtifactFormat, ArtifactSink, CancellationToken, EngineBackend, EngineError,
        GenerateOutput, GenerateRequest, ImageGenerationOutput, ImageGenerationRequest, LoadConfig,
        LoadedModelInfo, Result, TokenSink, Tokenization, DEFAULT_SEED,
    };

    const SERVER_CAPTURE_LIMIT: usize = 16 * 1024;
    const HTTP_HEADER_LIMIT: usize = 64 * 1024;
    const HEALTH_RESPONSE_LIMIT: usize = 1024 * 1024;
    const SERVER_EXIT_CLASSIFICATION_TIMEOUT: Duration = Duration::from_millis(250);
    const APPLE_METAL_CPU_TEXT_ENCODER_BACKEND: &str = "te=cpu,diffusion=metal,vae=metal";

    type OutputTail = Arc<Mutex<Vec<u8>>>;

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    enum StableDiffusionDevicePlacement {
        #[default]
        Preferred,
        AppleMetalCpuTextEncoder,
    }

    #[derive(Debug)]
    struct StableDiffusionServerProcess {
        child: Child,
        stdout_tail: OutputTail,
        stderr_tail: OutputTail,
        stdout_thread: Option<JoinHandle<()>>,
        stderr_thread: Option<JoinHandle<()>>,
    }

    impl StableDiffusionServerProcess {
        fn new(mut child: Child) -> Self {
            let stdout_tail = Arc::new(Mutex::new(Vec::new()));
            let stderr_tail = Arc::new(Mutex::new(Vec::new()));
            let stdout_thread = child
                .stdout
                .take()
                .map(|stdout| spawn_output_capture(stdout, Arc::clone(&stdout_tail)));
            let stderr_thread = child
                .stderr
                .take()
                .map(|stderr| spawn_output_capture(stderr, Arc::clone(&stderr_tail)));
            Self {
                child,
                stdout_tail,
                stderr_tail,
                stdout_thread,
                stderr_thread,
            }
        }

        fn output_summary(&self) -> String {
            let stdout = captured_output(&self.stdout_tail);
            let stderr = captured_output(&self.stderr_tail);
            format!("stderr={stderr:?}; stdout={stdout:?}")
        }

        fn join_capture_threads(&mut self) {
            if let Some(thread) = self.stdout_thread.take() {
                let _ = thread.join();
            }
            if let Some(thread) = self.stderr_thread.take() {
                let _ = thread.join();
            }
        }

        fn shutdown(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.join_capture_threads();
        }
    }

    impl Drop for StableDiffusionServerProcess {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    #[derive(Debug)]
    struct HttpResponse {
        status: u16,
        body: Vec<u8>,
    }

    #[derive(Debug)]
    pub struct StableDiffusionCppBackend {
        server_binary: PathBuf,
        loaded: Option<LoadedModelInfo>,
        config: Option<LoadConfig>,
        server_address: Option<SocketAddr>,
        server: Option<StableDiffusionServerProcess>,
        device_placement: StableDiffusionDevicePlacement,
    }

    impl StableDiffusionCppBackend {
        pub fn new() -> Result<Self> {
            let server_binary = env::var_os("MAYHEM_STABLE_DIFFUSION_CPP_SERVER_BIN")
                .map(PathBuf::from)
                .or_else(|| {
                    env::var_os("MAYHEM_STABLE_DIFFUSION_CPP_BIN")
                        .map(PathBuf::from)
                        .map(derive_server_binary)
                })
                .unwrap_or_else(|| PathBuf::from(server_executable_name()));
            Ok(Self::with_server_binary(server_binary))
        }

        pub fn with_binary(binary: impl Into<PathBuf>) -> Self {
            Self::with_server_binary(derive_server_binary(binary.into()))
        }

        pub fn with_server_binary(server_binary: impl Into<PathBuf>) -> Self {
            Self {
                server_binary: server_binary.into(),
                loaded: None,
                config: None,
                server_address: None,
                server: None,
                device_placement: StableDiffusionDevicePlacement::Preferred,
            }
        }

        fn config(&self) -> Result<&LoadConfig> {
            self.config.as_ref().ok_or(EngineError::NotLoaded)
        }

        fn stop_server(&mut self) {
            self.server_address = None;
            if let Some(mut server) = self.server.take() {
                server.shutdown();
            }
        }

        fn restart_exited_server(&mut self, request_error: &EngineError) -> Result<bool> {
            let Some(server) = self.server.as_mut() else {
                return Ok(false);
            };
            let started = Instant::now();
            let (status, output) = loop {
                match server.child.try_wait().map_err(|err| {
                    EngineError::StableDiffusionCpp(format!(
                        "checking {} after a failed request failed: {err}",
                        self.server_binary.display()
                    ))
                })? {
                    Some(status) => {
                        server.join_capture_threads();
                        break (status, server.output_summary());
                    }
                    None if started.elapsed() < SERVER_EXIT_CLASSIFICATION_TIMEOUT => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    None => return Ok(false),
                }
            };

            let config = self.config.clone().ok_or(EngineError::NotLoaded)?;
            self.select_recovery_device_placement(&config, &output);
            self.stop_server();
            let (address, server) = self.start_server(&config).map_err(|restart_error| {
                EngineError::StableDiffusionCpp(format!(
                    "sd-server exited with {status} after request failure ({request_error}); {output}; restarting it once failed: {restart_error}"
                ))
            })?;
            self.server_address = Some(address);
            self.server = Some(server);
            Ok(true)
        }

        fn select_recovery_device_placement(
            &mut self,
            config: &LoadConfig,
            server_output: &str,
        ) -> bool {
            let selected = recovery_device_placement(
                self.device_placement,
                requested_stable_diffusion_backend(config).as_deref(),
                server_output,
            );
            let changed = selected != self.device_placement;
            self.device_placement = selected;
            changed
        }

        fn request_images_once(
            &mut self,
            encoded_body: &[u8],
            image_count: u32,
            width: u32,
            height: u32,
            cancellation: &CancellationToken,
        ) -> Result<Vec<Vec<u8>>> {
            let address = self.server_address.ok_or(EngineError::NotLoaded)?;
            let response = http_request(
                address,
                "POST",
                "/sdapi/v1/txt2img",
                Some(encoded_body),
                max_image_response_bytes(image_count, width, height)?,
                None,
                Some(cancellation),
            )?;
            if !(200..300).contains(&response.status) {
                return Err(EngineError::StableDiffusionCpp(format!(
                    "sd-server returned HTTP {}: {}",
                    response.status,
                    response_snippet(&response.body)
                )));
            }
            let response: Value = serde_json::from_slice(&response.body)?;
            let images = response
                .get("images")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    EngineError::StableDiffusionCpp(
                        "sd-server response is missing the images array".to_owned(),
                    )
                })?;
            if images.len() != image_count as usize {
                return Err(EngineError::StableDiffusionCpp(format!(
                    "sd-server returned {} images for requested batch of {image_count}",
                    images.len()
                )));
            }
            images
                .iter()
                .enumerate()
                .map(|(image_index, image)| {
                    let encoded = image.as_str().ok_or_else(|| {
                        EngineError::StableDiffusionCpp(format!(
                            "sd-server image {} is not base64 text",
                            image_index + 1
                        ))
                    })?;
                    let bytes = general_purpose::STANDARD.decode(encoded).map_err(|err| {
                        EngineError::StableDiffusionCpp(format!(
                            "decoding sd-server image {} failed: {err}",
                            image_index + 1
                        ))
                    })?;
                    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                        return Err(EngineError::StableDiffusionCpp(format!(
                            "sd-server image {} is not a PNG",
                            image_index + 1
                        )));
                    }
                    Ok(bytes)
                })
                .collect()
        }

        fn build_server_command(
            &self,
            config: &LoadConfig,
            address: SocketAddr,
        ) -> Result<Command> {
            let model_path = stable_diffusion_payload_path(&config.artifact.path)?;
            let llm_path = config
                .stable_diffusion_llm
                .as_ref()
                .map(|artifact| stable_diffusion_payload_path(&artifact.path))
                .transpose()?;
            let vae_path = config
                .stable_diffusion_vae
                .as_ref()
                .map(|artifact| stable_diffusion_payload_path(&artifact.path))
                .transpose()?;
            let backend = stable_diffusion_backend_for_placement(
                requested_stable_diffusion_backend(config),
                self.device_placement,
            );

            let mut command = Command::new(&self.server_binary);
            if config.stable_diffusion_cpp.separate_diffusion_model {
                command.arg("--diffusion-model").arg(model_path);
            } else {
                command.arg("-m").arg(model_path);
            }
            if let Some(llm_path) = llm_path {
                command.arg("--llm").arg(llm_path);
            }
            if let Some(vae_path) = vae_path {
                command.arg("--vae").arg(vae_path);
            }
            if let Some(backend) = backend {
                command.arg("--backend").arg(backend);
            }
            command
                .arg("--rng")
                .arg("cpu")
                .arg("--listen-ip")
                .arg(address.ip().to_string())
                .arg("--listen-port")
                .arg(address.port().to_string())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            Ok(command)
        }

        fn start_server(
            &mut self,
            config: &LoadConfig,
        ) -> Result<(SocketAddr, StableDiffusionServerProcess)> {
            let address = reserve_loopback_address()?;
            let mut command = self.build_server_command(config, address)?;
            let child = command.spawn().map_err(|err| {
                EngineError::StableDiffusionCpp(format!(
                    "starting {} failed: {err}",
                    self.server_binary.display()
                ))
            })?;
            let mut server = StableDiffusionServerProcess::new(child);
            let timeout = configured_startup_timeout()?;
            let started = Instant::now();

            loop {
                if let Some(status) = server.child.try_wait().map_err(|err| {
                    EngineError::StableDiffusionCpp(format!(
                        "checking {} failed: {err}",
                        self.server_binary.display()
                    ))
                })? {
                    server.join_capture_threads();
                    return Err(EngineError::StableDiffusionCpp(format!(
                        "{} exited during model load with {status}; {}",
                        self.server_binary.display(),
                        server.output_summary()
                    )));
                }

                if server_ready(address) {
                    return Ok((address, server));
                }

                if timeout.is_some_and(|timeout| started.elapsed() >= timeout) {
                    server.shutdown();
                    return Err(EngineError::StableDiffusionCpp(format!(
                        "{} did not become ready within the configured {} seconds; {}",
                        self.server_binary.display(),
                        timeout.expect("checked above").as_secs(),
                        server.output_summary()
                    )));
                }
                thread::sleep(Duration::from_millis(250));
            }
        }

        #[cfg(test)]
        pub(crate) fn with_ready_server(config: LoadConfig, address: SocketAddr) -> Result<Self> {
            validate_stable_diffusion_config(&config)?;
            Ok(Self {
                server_binary: PathBuf::from(server_executable_name()),
                loaded: Some(loaded_model_info(&config)),
                config: Some(config),
                server_address: Some(address),
                server: None,
                device_placement: StableDiffusionDevicePlacement::Preferred,
            })
        }

        #[cfg(test)]
        pub(crate) fn server_command_args(
            &self,
            config: &LoadConfig,
            address: SocketAddr,
        ) -> Result<Vec<String>> {
            self.build_server_command(config, address).map(|command| {
                command
                    .get_args()
                    .map(|value| value.to_string_lossy().into_owned())
                    .collect()
            })
        }
    }

    impl Drop for StableDiffusionCppBackend {
        fn drop(&mut self) {
            self.stop_server();
        }
    }

    impl EngineBackend for StableDiffusionCppBackend {
        fn backend_id(&self) -> &'static str {
            "stable-diffusion.cpp"
        }

        fn load(&mut self, config: LoadConfig) -> Result<LoadedModelInfo> {
            validate_stable_diffusion_config(&config)?;
            let info = loaded_model_info(&config);
            self.stop_server();
            self.device_placement = StableDiffusionDevicePlacement::Preferred;
            let (address, server) = self.start_server(&config)?;
            self.loaded = Some(info.clone());
            self.config = Some(config);
            self.server_address = Some(address);
            self.server = Some(server);
            Ok(info)
        }

        fn component_healthy(&mut self) -> bool {
            match self.server.as_mut() {
                Some(server) => matches!(server.child.try_wait(), Ok(None)),
                None => self.server_address.is_some(),
            }
        }

        fn process_ids(&self) -> Vec<u32> {
            self.server
                .as_ref()
                .map(|server| vec![server.child.id()])
                .unwrap_or_default()
        }

        fn tokenize(&self, text: &str) -> Result<Tokenization> {
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
                "stable-diffusion.cpp backend emits image artifacts; use generate_with_artifacts"
                    .to_owned(),
            ))
        }

        fn generate_image(
            &mut self,
            request: ImageGenerationRequest,
            artifact_sink: &mut dyn ArtifactSink,
            cancellation: &CancellationToken,
        ) -> Result<ImageGenerationOutput> {
            cancellation.check()?;
            request.validate()?;
            let config = self.config()?.clone();
            if request.prompt.contains("sd_cpp_extra_args") {
                return Err(EngineError::InvalidConfig(
                    "image prompt may not contain stable-diffusion.cpp control tags".to_owned(),
                ));
            }
            let image_count = request.image_count;
            let width = request.width;
            let height = request.height;
            let steps = request.steps;
            let engine_steps = i64::from(steps)
                .checked_add(i64::from(config.stable_diffusion_cpp.steps_offset))
                .filter(|value| (1..=150).contains(value))
                .ok_or_else(|| {
                    EngineError::InvalidConfig(format!(
                        "image steps {steps} with backend offset {} fall outside 1..=150",
                        config.stable_diffusion_cpp.steps_offset
                    ))
                })?;
            let cfg_scale = request.guidance_scale;
            let engine_cfg_scale =
                cfg_scale + config.stable_diffusion_cpp.guidance_scale_offset as f32;
            if !engine_cfg_scale.is_finite() || !(0.0..=50.0).contains(&engine_cfg_scale) {
                return Err(EngineError::InvalidConfig(format!(
                    "image guidance_scale {cfg_scale} with backend offset {} falls outside 0..=50",
                    config.stable_diffusion_cpp.guidance_scale_offset
                )));
            }
            let seed_base = request.seed.unwrap_or(DEFAULT_SEED);
            let mut body = json!({
                "prompt": request.prompt,
                "width": width,
                "height": height,
                "steps": engine_steps,
                "cfg_scale": engine_cfg_scale,
                "seed": seed_base,
                "batch_size": image_count,
            });
            let object = body
                .as_object_mut()
                .expect("stable-diffusion request body is an object");
            if let Some(negative_prompt) = request.negative_prompt {
                object.insert("negative_prompt".to_owned(), Value::String(negative_prompt));
            }
            if let Some(sampling_method) = request.sampling_method {
                object.insert("sampler_name".to_owned(), Value::String(sampling_method));
            }
            if let Some(scheduler) = request.scheduler {
                object.insert("scheduler".to_owned(), Value::String(scheduler));
            }
            if let Some(flow_shift) = request.flow_shift {
                let extra = serde_json::to_string(&json!({
                    "sample_params": {"flow_shift": flow_shift}
                }))?;
                let prompt = object
                    .get("prompt")
                    .and_then(Value::as_str)
                    .expect("stable-diffusion request prompt is text");
                object.insert(
                    "prompt".to_owned(),
                    Value::String(format!(
                        "{prompt} <sd_cpp_extra_args>{extra}</sd_cpp_extra_args>"
                    )),
                );
            }
            let encoded_body = serde_json::to_vec(&body)?;
            let images = match retry_once_after_worker_exit(
                self,
                |backend| {
                    backend.request_images_once(
                        &encoded_body,
                        image_count,
                        width,
                        height,
                        cancellation,
                    )
                },
                |backend, request_error| backend.restart_exited_server(request_error),
            ) {
                Err(EngineError::Cancelled) => {
                    self.stop_server();
                    return Err(EngineError::Cancelled);
                }
                result => result?,
            };
            for (image_index, bytes) in images.into_iter().enumerate() {
                cancellation.check()?;
                artifact_sink.on_artifact_chunk(ArtifactChunk {
                    artifact_id: format!("image-{}", image_index + 1),
                    index: 0,
                    content_type: "image/png".to_owned(),
                    bytes,
                    final_chunk: true,
                })?;
            }

            Ok(ImageGenerationOutput { image_count, steps })
        }
    }

    fn retry_once_after_worker_exit<S, T>(
        state: &mut S,
        mut operation: impl FnMut(&mut S) -> Result<T>,
        mut restart: impl FnMut(&mut S, &EngineError) -> Result<bool>,
    ) -> Result<T> {
        let first_error = match operation(state) {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        if !restart(state, &first_error)? {
            return Err(first_error);
        }
        operation(state)
    }

    fn requested_stable_diffusion_backend(config: &LoadConfig) -> Option<String> {
        env::var("MAYHEM_STABLE_DIFFUSION_CPP_BACKEND")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| config.stable_diffusion_backend.clone())
    }

    fn stable_diffusion_backend_for_placement(
        requested: Option<String>,
        placement: StableDiffusionDevicePlacement,
    ) -> Option<String> {
        match placement {
            StableDiffusionDevicePlacement::Preferred => requested,
            StableDiffusionDevicePlacement::AppleMetalCpuTextEncoder => {
                Some(APPLE_METAL_CPU_TEXT_ENCODER_BACKEND.to_owned())
            }
        }
    }

    fn is_plain_metal_backend(backend: &str) -> bool {
        backend.trim().eq_ignore_ascii_case("metal")
    }

    fn recovery_device_placement(
        current: StableDiffusionDevicePlacement,
        requested_backend: Option<&str>,
        server_output: &str,
    ) -> StableDiffusionDevicePlacement {
        if current == StableDiffusionDevicePlacement::Preferred
            && requested_backend.is_some_and(is_plain_metal_backend)
            && is_metal_text_encoder_empty_state_failure(server_output)
        {
            StableDiffusionDevicePlacement::AppleMetalCpuTextEncoder
        } else {
            current
        }
    }

    fn is_metal_text_encoder_empty_state_failure(output: &str) -> bool {
        let output = output.to_ascii_lowercase();
        (output.contains("conditioner.hpp") || output.contains("conditioner.cpp"))
            && output.contains("hidden_states.empty")
            && (output.contains("assert") || output.contains("ggml_abort"))
    }

    fn validate_stable_diffusion_config(config: &LoadConfig) -> Result<()> {
        validate_load_config(config)?;
        if config.artifact.format != ArtifactFormat::StableDiffusionCheckpoint {
            return Err(EngineError::InvalidConfig(format!(
                "stable-diffusion.cpp backend requires stable-diffusion checkpoints, got {:?}",
                config.artifact.format
            )));
        }
        config.stable_diffusion_cpp.validate()?;
        verify_artifact(&config.artifact)?;
        for (label, artifact) in [
            ("LLM", config.stable_diffusion_llm.as_ref()),
            ("VAE", config.stable_diffusion_vae.as_ref()),
        ] {
            if let Some(artifact) = artifact {
                if artifact.format != ArtifactFormat::StableDiffusionCheckpoint {
                    return Err(EngineError::InvalidConfig(format!(
                        "stable-diffusion.cpp {label} sidecar must use stable-diffusion checkpoint format"
                    )));
                }
                verify_artifact(artifact)?;
            }
        }
        if config.stable_diffusion_cpp.separate_diffusion_model
            && (config.stable_diffusion_llm.is_none() || config.stable_diffusion_vae.is_none())
        {
            return Err(EngineError::InvalidConfig(
                "separate stable-diffusion.cpp diffusion models require pinned LLM and VAE sidecars"
                    .to_owned(),
            ));
        }
        Ok(())
    }

    fn loaded_model_info(config: &LoadConfig) -> LoadedModelInfo {
        LoadedModelInfo {
            backend: "stable-diffusion.cpp".to_owned(),
            artifact: config.artifact.clone(),
            ctx_size: config.ctx_size,
            n_ctx_train: 0,
            n_vocab: 0,
        }
    }

    fn server_executable_name() -> &'static str {
        if cfg!(windows) {
            "sd-server.exe"
        } else {
            "sd-server"
        }
    }

    fn derive_server_binary(binary: PathBuf) -> PathBuf {
        let Some(name) = binary.file_name().and_then(|name| name.to_str()) else {
            return binary;
        };
        let replacement = if name.eq_ignore_ascii_case("sd-cli.exe") {
            Some("sd-server.exe")
        } else if name.eq_ignore_ascii_case("sd-cli") {
            Some("sd-server")
        } else {
            None
        };
        replacement
            .map(|replacement| binary.with_file_name(replacement))
            .unwrap_or(binary)
    }

    fn reserve_loopback_address() -> Result<SocketAddr> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        drop(listener);
        Ok(address)
    }

    fn configured_startup_timeout() -> Result<Option<Duration>> {
        let Some(raw) = env::var_os("MAYHEM_STABLE_DIFFUSION_CPP_STARTUP_TIMEOUT_SECONDS") else {
            return Ok(None);
        };
        let raw = raw.to_string_lossy();
        let seconds = raw.parse::<u64>().map_err(|_| {
            EngineError::InvalidConfig(
                "MAYHEM_STABLE_DIFFUSION_CPP_STARTUP_TIMEOUT_SECONDS must be a positive integer"
                    .to_owned(),
            )
        })?;
        if seconds == 0 {
            return Err(EngineError::InvalidConfig(
                "MAYHEM_STABLE_DIFFUSION_CPP_STARTUP_TIMEOUT_SECONDS must be greater than zero"
                    .to_owned(),
            ));
        }
        Ok(Some(Duration::from_secs(seconds)))
    }

    fn server_ready(address: SocketAddr) -> bool {
        server_ready_with_timeout(address, Duration::from_secs(1))
    }

    fn server_ready_with_timeout(address: SocketAddr, timeout: Duration) -> bool {
        let Ok(response) = http_request(
            address,
            "GET",
            "/v1/models",
            None,
            HEALTH_RESPONSE_LIMIT,
            Some(timeout),
            None,
        ) else {
            return false;
        };
        response.status == 200
            && serde_json::from_slice::<Value>(&response.body)
                .ok()
                .and_then(|value| value.get("data").cloned())
                .is_some_and(|data| data.is_array())
    }

    fn spawn_output_capture<R>(mut reader: R, tail: OutputTail) -> JoinHandle<()>
    where
        R: Read + Send + 'static,
    {
        thread::spawn(move || {
            let mut chunk = [0_u8; 4096];
            while let Ok(count) = reader.read(&mut chunk) {
                if count == 0 {
                    break;
                }
                let Ok(mut captured) = tail.lock() else {
                    break;
                };
                captured.extend_from_slice(&chunk[..count]);
                if captured.len() > SERVER_CAPTURE_LIMIT {
                    let excess = captured.len() - SERVER_CAPTURE_LIMIT;
                    captured.drain(..excess);
                }
            }
        })
    }

    fn captured_output(tail: &OutputTail) -> String {
        tail.lock()
            .map(|captured| String::from_utf8_lossy(&captured).trim().to_owned())
            .unwrap_or_else(|_| "<capture unavailable>".to_owned())
    }

    fn max_image_response_bytes(image_count: u32, width: u32, height: u32) -> Result<usize> {
        let raw_bytes = usize::try_from(image_count)
            .ok()
            .and_then(|count| count.checked_mul(width as usize))
            .and_then(|pixels| pixels.checked_mul(height as usize))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| {
                EngineError::InvalidConfig("image response size exceeds platform limits".to_owned())
            })?;
        raw_bytes
            .checked_mul(2)
            .and_then(|bound| bound.checked_add(1024 * 1024))
            .ok_or_else(|| {
                EngineError::InvalidConfig("image response size exceeds platform limits".to_owned())
            })
    }

    fn http_request(
        address: SocketAddr,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        body_limit: usize,
        read_timeout: Option<Duration>,
        cancellation: Option<&CancellationToken>,
    ) -> Result<HttpResponse> {
        let deadline = read_timeout.map(|timeout| Instant::now() + timeout);
        check_http_control(cancellation, deadline)?;
        let mut stream =
            TcpStream::connect_timeout(&address, Duration::from_millis(250)).map_err(|err| {
                EngineError::StableDiffusionCpp(format!(
                    "connecting to local sd-server at {address} failed: {err}"
                ))
            })?;
        stream.set_nodelay(true)?;
        let poll_timeout =
            (cancellation.is_some() || deadline.is_some()).then_some(Duration::from_millis(25));
        stream.set_read_timeout(poll_timeout)?;
        stream.set_write_timeout(poll_timeout)?;
        let body = body.unwrap_or_default();
        let headers = format!(
            "{method} {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        write_all_cancellable(&mut stream, headers.as_bytes(), cancellation, deadline)?;
        write_all_cancellable(&mut stream, body, cancellation, deadline)?;
        flush_cancellable(&mut stream, cancellation, deadline)?;

        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        if read_line_cancellable(&mut reader, &mut status_line, cancellation, deadline)? == 0 {
            return Err(EngineError::StableDiffusionCpp(
                "local sd-server closed without an HTTP response".to_owned(),
            ));
        }
        let status = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or_else(|| {
                EngineError::StableDiffusionCpp(format!(
                    "local sd-server returned an invalid status line: {:?}",
                    status_line.trim()
                ))
            })?;

        let mut header_bytes = status_line.len();
        let mut content_length = None;
        let mut chunked = false;
        loop {
            let mut line = String::new();
            if read_line_cancellable(&mut reader, &mut line, cancellation, deadline)? == 0 {
                return Err(EngineError::StableDiffusionCpp(
                    "local sd-server closed inside HTTP headers".to_owned(),
                ));
            }
            header_bytes = header_bytes.saturating_add(line.len());
            if header_bytes > HTTP_HEADER_LIMIT {
                return Err(EngineError::StableDiffusionCpp(
                    "local sd-server HTTP headers exceed 64 KiB".to_owned(),
                ));
            }
            if line == "\r\n" || line == "\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = Some(value.trim().parse::<usize>().map_err(|_| {
                        EngineError::StableDiffusionCpp(
                            "local sd-server returned an invalid Content-Length".to_owned(),
                        )
                    })?);
                } else if name.eq_ignore_ascii_case("transfer-encoding")
                    && value
                        .split(',')
                        .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
                {
                    chunked = true;
                }
            }
        }

        let body = if chunked {
            read_chunked_body(&mut reader, body_limit, cancellation, deadline)?
        } else if let Some(content_length) = content_length {
            if content_length > body_limit {
                return Err(EngineError::StableDiffusionCpp(format!(
                    "local sd-server response is {content_length} bytes, above the {body_limit}-byte bound"
                )));
            }
            let mut body = vec![0_u8; content_length];
            read_exact_cancellable(&mut reader, &mut body, cancellation, deadline)?;
            body
        } else {
            let body = read_to_end_cancellable(&mut reader, body_limit, cancellation, deadline)?;
            if body.len() > body_limit {
                return Err(EngineError::StableDiffusionCpp(format!(
                    "local sd-server response exceeds the {body_limit}-byte bound"
                )));
            }
            body
        };
        Ok(HttpResponse { status, body })
    }

    fn read_chunked_body<R: BufRead>(
        reader: &mut R,
        body_limit: usize,
        cancellation: Option<&CancellationToken>,
        deadline: Option<Instant>,
    ) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        loop {
            let mut line = String::new();
            if read_line_cancellable(reader, &mut line, cancellation, deadline)? == 0 {
                return Err(EngineError::StableDiffusionCpp(
                    "local sd-server closed inside a chunked response".to_owned(),
                ));
            }
            let size = line
                .trim()
                .split(';')
                .next()
                .and_then(|value| usize::from_str_radix(value, 16).ok())
                .ok_or_else(|| {
                    EngineError::StableDiffusionCpp(
                        "local sd-server returned an invalid chunk size".to_owned(),
                    )
                })?;
            if size == 0 {
                loop {
                    line.clear();
                    if read_line_cancellable(reader, &mut line, cancellation, deadline)? == 0
                        || line == "\r\n"
                        || line == "\n"
                    {
                        break;
                    }
                }
                break;
            }
            if body.len().saturating_add(size) > body_limit {
                return Err(EngineError::StableDiffusionCpp(format!(
                    "local sd-server response exceeds the {body_limit}-byte bound"
                )));
            }
            let start = body.len();
            body.resize(start + size, 0);
            read_exact_cancellable(reader, &mut body[start..], cancellation, deadline)?;
            let mut terminator = [0_u8; 2];
            read_exact_cancellable(reader, &mut terminator, cancellation, deadline)?;
            if terminator != *b"\r\n" {
                return Err(EngineError::StableDiffusionCpp(
                    "local sd-server returned an invalid chunk terminator".to_owned(),
                ));
            }
        }
        Ok(body)
    }

    fn check_http_control(
        cancellation: Option<&CancellationToken>,
        deadline: Option<Instant>,
    ) -> Result<()> {
        cancellation
            .map(CancellationToken::check)
            .unwrap_or(Ok(()))?;
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(EngineError::StableDiffusionCpp(
                "local sd-server HTTP request exceeded its deadline".to_owned(),
            ));
        }
        Ok(())
    }

    fn retry_controlled_io(
        error: &std::io::Error,
        cancellation: Option<&CancellationToken>,
        deadline: Option<Instant>,
    ) -> bool {
        (cancellation.is_some() || deadline.is_some())
            && matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            )
    }

    fn read_line_cancellable<R: BufRead>(
        reader: &mut R,
        line: &mut String,
        cancellation: Option<&CancellationToken>,
        deadline: Option<Instant>,
    ) -> Result<usize> {
        loop {
            check_http_control(cancellation, deadline)?;
            match reader.read_line(line) {
                Ok(read) => return Ok(read),
                Err(error) if retry_controlled_io(&error, cancellation, deadline) => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn write_all_cancellable<W: Write>(
        writer: &mut W,
        mut bytes: &[u8],
        cancellation: Option<&CancellationToken>,
        deadline: Option<Instant>,
    ) -> Result<()> {
        while !bytes.is_empty() {
            check_http_control(cancellation, deadline)?;
            match writer.write(bytes) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "local sd-server request stopped accepting bytes",
                    )
                    .into())
                }
                Ok(written) => bytes = &bytes[written..],
                Err(error) if retry_controlled_io(&error, cancellation, deadline) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn flush_cancellable<W: Write>(
        writer: &mut W,
        cancellation: Option<&CancellationToken>,
        deadline: Option<Instant>,
    ) -> Result<()> {
        loop {
            check_http_control(cancellation, deadline)?;
            match writer.flush() {
                Ok(()) => return Ok(()),
                Err(error) if retry_controlled_io(&error, cancellation, deadline) => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn read_exact_cancellable<R: Read>(
        reader: &mut R,
        mut bytes: &mut [u8],
        cancellation: Option<&CancellationToken>,
        deadline: Option<Instant>,
    ) -> Result<()> {
        while !bytes.is_empty() {
            check_http_control(cancellation, deadline)?;
            match reader.read(bytes) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "local sd-server response ended early",
                    )
                    .into())
                }
                Ok(read) => bytes = &mut bytes[read..],
                Err(error) if retry_controlled_io(&error, cancellation, deadline) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn read_to_end_cancellable<R: Read>(
        reader: &mut R,
        body_limit: usize,
        cancellation: Option<&CancellationToken>,
        deadline: Option<Instant>,
    ) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        let mut chunk = [0_u8; 16 * 1024];
        loop {
            check_http_control(cancellation, deadline)?;
            match reader.read(&mut chunk) {
                Ok(0) => return Ok(body),
                Ok(read) => {
                    body.extend_from_slice(&chunk[..read]);
                    if body.len() > body_limit {
                        return Ok(body);
                    }
                }
                Err(error) if retry_controlled_io(&error, cancellation, deadline) => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn response_snippet(body: &[u8]) -> String {
        String::from_utf8_lossy(&body[..body.len().min(4096)])
            .chars()
            .map(|character| {
                if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
                    '\u{fffd}'
                } else {
                    character
                }
            })
            .collect()
    }

    #[cfg(test)]
    mod recovery_tests {
        use std::sync::mpsc;

        use super::*;

        #[derive(Default)]
        struct RecoveryProbe {
            operations: usize,
            restarts: usize,
        }

        #[test]
        fn readiness_deadline_closes_a_stalled_probe_and_allows_retry() {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake sd-server");
            let address = listener.local_addr().expect("fake sd-server address");
            let (first_request_tx, first_request_rx) = mpsc::channel();
            let server = thread::spawn(move || {
                let (mut first, _) = listener.accept().expect("accept stalled readiness probe");
                let mut request = [0_u8; 4096];
                let read = first
                    .read(&mut request)
                    .expect("read first readiness probe");
                assert!(String::from_utf8_lossy(&request[..read]).starts_with("GET /v1/models"));
                first_request_tx.send(()).expect("signal first probe");

                let mut closed = [0_u8; 1];
                assert_eq!(first.read(&mut closed).expect("observe probe close"), 0);

                let (mut second, _) = listener.accept().expect("accept readiness retry");
                let read = second.read(&mut request).expect("read readiness retry");
                assert!(String::from_utf8_lossy(&request[..read]).starts_with("GET /v1/models"));
                let body =
                    br#"{"data":[{"id":"sd-cpp-local","object":"model","owned_by":"local"}]}"#;
                write!(
                    second,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .expect("write readiness headers");
                second.write_all(body).expect("write readiness body");
            });

            let started = Instant::now();
            assert!(!server_ready_with_timeout(
                address,
                Duration::from_millis(100)
            ));
            first_request_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("first readiness probe arrived");
            assert!(started.elapsed() < Duration::from_secs(1));
            assert!(server_ready_with_timeout(address, Duration::from_secs(1)));
            server.join().expect("fake sd-server exits");
        }

        #[test]
        fn confirmed_worker_exit_retries_the_request_exactly_once() {
            let mut probe = RecoveryProbe::default();
            let value = retry_once_after_worker_exit(
                &mut probe,
                |probe| {
                    probe.operations += 1;
                    if probe.operations == 1 {
                        Err(EngineError::StableDiffusionCpp(
                            "worker closed without a response".to_owned(),
                        ))
                    } else {
                        Ok("ready")
                    }
                },
                |probe, _| {
                    probe.restarts += 1;
                    Ok(true)
                },
            )
            .expect("confirmed exit restarts and reruns readiness request");

            assert_eq!(value, "ready");
            assert_eq!(probe.operations, 2);
            assert_eq!(probe.restarts, 1);
        }

        #[test]
        fn live_worker_error_is_not_retried() {
            let mut probe = RecoveryProbe::default();
            let error = retry_once_after_worker_exit(
                &mut probe,
                |probe| {
                    probe.operations += 1;
                    Err::<(), _>(EngineError::StableDiffusionCpp(
                        "sd-server returned HTTP 500".to_owned(),
                    ))
                },
                |probe, _| {
                    probe.restarts += 1;
                    Ok(false)
                },
            )
            .expect_err("a live worker error must be returned");

            assert!(error.to_string().contains("HTTP 500"));
            assert_eq!(probe.operations, 1);
            assert_eq!(probe.restarts, 1);
        }

        #[test]
        fn failed_retry_does_not_start_a_recovery_loop() {
            let mut probe = RecoveryProbe::default();
            let error = retry_once_after_worker_exit(
                &mut probe,
                |probe| {
                    probe.operations += 1;
                    Err::<(), _>(EngineError::StableDiffusionCpp(format!(
                        "worker failure {}",
                        probe.operations
                    )))
                },
                |probe, _| {
                    probe.restarts += 1;
                    Ok(true)
                },
            )
            .expect_err("the single retry also fails");

            assert!(error.to_string().contains("worker failure 2"));
            assert_eq!(probe.operations, 2);
            assert_eq!(probe.restarts, 1);
        }

        #[test]
        fn healthy_apple_metal_path_remains_full_metal() {
            let placement = recovery_device_placement(
                StableDiffusionDevicePlacement::Preferred,
                Some("metal"),
                "generation completed normally",
            );

            assert_eq!(placement, StableDiffusionDevicePlacement::Preferred);
            assert_eq!(
                stable_diffusion_backend_for_placement(Some("metal".to_owned()), placement)
                    .as_deref(),
                Some("metal")
            );
        }

        #[test]
        fn apple_metal_prompt_encoder_assertion_selects_cpu_text_encoder() {
            let placement = recovery_device_placement(
                StableDiffusionDevicePlacement::Preferred,
                Some("metal"),
                "GGML_ASSERT(!hidden_states.empty()) failed at src/conditioning/conditioner.hpp:1972",
            );

            assert_eq!(
                placement,
                StableDiffusionDevicePlacement::AppleMetalCpuTextEncoder
            );
            assert_eq!(
                stable_diffusion_backend_for_placement(Some("metal".to_owned()), placement)
                    .as_deref(),
                Some(APPLE_METAL_CPU_TEXT_ENCODER_BACKEND)
            );
        }

        #[test]
        fn prompt_encoder_assertion_does_not_override_other_or_explicit_placements() {
            let failure =
                "assertion !hidden_states.empty() failed in src/conditioning/conditioner.cpp";
            for backend in ["cuda", "cpu", "te=metal,diffusion=metal,vae=metal"] {
                assert_eq!(
                    recovery_device_placement(
                        StableDiffusionDevicePlacement::Preferred,
                        Some(backend),
                        failure,
                    ),
                    StableDiffusionDevicePlacement::Preferred,
                    "backend {backend} must remain authoritative"
                );
            }
        }

        #[test]
        fn unrelated_apple_metal_worker_exit_does_not_select_split_placement() {
            let placement = recovery_device_placement(
                StableDiffusionDevicePlacement::Preferred,
                Some("metal"),
                "sd-server exited after receiving SIGTERM",
            );

            assert_eq!(placement, StableDiffusionDevicePlacement::Preferred);
        }
    }
}

#[cfg(test)]
mod stable_diffusion_tests {
    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use base64::{engine::general_purpose, Engine as _};

    use super::*;

    #[test]
    fn stable_diffusion_cpp_backend_uses_one_native_request_and_emits_every_image() {
        let root =
            std::env::temp_dir().join(format!("mayhem-engine-sd-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let model = root.join("model.safetensors");
        fs::write(&model, stable_empty_safetensors()).unwrap();
        let expected = json!({
            "prompt": "a red square <sd_cpp_extra_args>{\"sample_params\":{\"flow_shift\":3.0}}</sd_cpp_extra_args>",
            "negative_prompt": "blur",
            "batch_size": 2,
            "width": 64,
            "height": 64,
            "steps": 2,
            "cfg_scale": 1.25,
            "seed": 7,
            "sampler_name": "euler",
            "scheduler": "discrete",
        });
        let first = b"\x89PNG\r\n\x1a\nfirst".to_vec();
        let second = b"\x89PNG\r\n\x1a\nsecond".to_vec();
        let (address, server) = serve_sdapi_once(expected, vec![first.clone(), second.clone()]);
        let mut backend = StableDiffusionCppBackend::with_ready_server(
            LoadConfig::stable_diffusion_checkpoint(&model),
            address,
        )
        .unwrap();
        let mut request = ImageGenerationRequest::new("a red square");
        request.negative_prompt = Some("blur".to_owned());
        request.image_count = 2;
        request.width = 64;
        request.height = 64;
        request.steps = 2;
        request.guidance_scale = 1.25;
        request.flow_shift = Some(3.0);
        request.seed = Some(7);
        request.sampling_method = Some("euler".to_owned());
        request.scheduler = Some("discrete".to_owned());
        let mut artifacts = Vec::new();
        backend
            .generate_image(
                request,
                &mut |chunk| {
                    artifacts.push(chunk);
                    Ok(())
                },
                &CancellationToken::new(),
            )
            .unwrap();

        server.join().unwrap();
        assert_eq!(artifacts.len(), 2);
        assert_eq!(artifacts[0].artifact_id, "image-1");
        assert_eq!(artifacts[0].content_type, "image/png");
        assert_eq!(artifacts[0].bytes, first);
        assert_eq!(artifacts[1].artifact_id, "image-2");
        assert_eq!(artifacts[1].bytes, second);
        assert!(artifacts[0].final_chunk);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stable_diffusion_cpp_backend_wires_split_components_and_signed_offsets() {
        let root = std::env::temp_dir().join(format!(
            "mayhem-engine-sd-components-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let diffusion = root.join("diffusion.gguf");
        let llm = root.join("text-encoder.gguf");
        let vae = root.join("vae.safetensors");
        fs::write(&diffusion, b"GGUF").unwrap();
        fs::write(&llm, b"GGUF").unwrap();
        fs::write(&vae, stable_empty_safetensors()).unwrap();

        let mut config = LoadConfig::stable_diffusion_checkpoint(&diffusion);
        config.stable_diffusion_llm = Some(ModelArtifact::stable_diffusion_checkpoint(&llm));
        config.stable_diffusion_cpp = StableDiffusionCppConfig {
            separate_diffusion_model: true,
            guidance_scale_offset: 1,
            steps_offset: -1,
        };
        let address = "127.0.0.1:18371".parse().unwrap();
        let missing_vae =
            StableDiffusionCppBackend::with_ready_server(config.clone(), address).unwrap_err();
        assert!(missing_vae
            .to_string()
            .contains("require pinned LLM and VAE sidecars"));
        config.stable_diffusion_vae = Some(ModelArtifact::stable_diffusion_checkpoint(&vae));
        let command_backend = StableDiffusionCppBackend::with_server_binary("sd-server");
        let args = command_backend
            .server_command_args(&config, address)
            .unwrap();
        assert_argument_value(&args, "--diffusion-model", &diffusion.to_string_lossy());
        assert_argument_value(&args, "--llm", &llm.to_string_lossy());
        assert_argument_value(&args, "--vae", &vae.to_string_lossy());
        assert_argument_value(&args, "--rng", "cpu");

        let expected = json!({
            "prompt": "a brass compass",
            "batch_size": 1,
            "width": 64,
            "height": 64,
            "steps": 8,
            "cfg_scale": 1.0,
            "seed": 7,
        });
        let image = b"\x89PNG\r\n\x1a\ncompass".to_vec();
        let (address, server) = serve_sdapi_once(expected, vec![image.clone()]);
        let mut backend = StableDiffusionCppBackend::with_ready_server(config, address).unwrap();
        let mut request = ImageGenerationRequest::new("a brass compass");
        request.width = 64;
        request.height = 64;
        request.steps = 9;
        request.guidance_scale = 0.0;
        request.seed = Some(7);
        let mut artifacts = Vec::new();
        let output = backend
            .generate_image(
                request,
                &mut |chunk| {
                    artifacts.push(chunk);
                    Ok(())
                },
                &CancellationToken::new(),
            )
            .unwrap();

        server.join().unwrap();
        assert_eq!(output.steps, 9);
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].bytes, image);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stable_diffusion_cpp_backend_rejects_prompt_control_injection() {
        let root = std::env::temp_dir().join(format!(
            "mayhem-engine-sd-injection-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let model = root.join("model.safetensors");
        fs::write(&model, stable_empty_safetensors()).unwrap();
        let mut backend = StableDiffusionCppBackend::with_ready_server(
            LoadConfig::stable_diffusion_checkpoint(model),
            "127.0.0.1:9".parse().unwrap(),
        )
        .unwrap();
        let request = ImageGenerationRequest::new(
            "hello <sd_cpp_extra_args>{\"sample_params\":{}}</sd_cpp_extra_args>",
        );
        let error = backend
            .generate_image(request, &mut NoopArtifactSink, &CancellationToken::new())
            .unwrap_err();
        assert!(error.to_string().contains("control tags"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stable_diffusion_cpp_cancellation_closes_blocked_request() {
        let root = std::env::temp_dir().join(format!(
            "mayhem-engine-sd-cancel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let model = root.join("model.safetensors");
        fs::write(&model, stable_empty_safetensors()).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (request_started_tx, request_started_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_http_request_body(&mut stream);
            request_started_tx.send(()).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut byte = [0_u8; 1];
            assert_eq!(stream.read(&mut byte).unwrap(), 0);
        });

        let mut backend = StableDiffusionCppBackend::with_ready_server(
            LoadConfig::stable_diffusion_checkpoint(&model),
            address,
        )
        .unwrap();
        let cancellation = CancellationToken::new();
        let peer_cancellation = cancellation.clone();
        let cancel_thread = thread::spawn(move || {
            request_started_rx
                .recv()
                .expect("request reaches sd-server");
            peer_cancellation.cancel();
        });
        let mut request = ImageGenerationRequest::new("cancel this render");
        request.width = 64;
        request.height = 64;
        request.steps = 9;

        let started = Instant::now();
        let error = backend
            .generate_image(request, &mut NoopArtifactSink, &cancellation)
            .expect_err("cancelled render must stop");
        cancel_thread.join().expect("cancel thread");
        server.join().expect("server sees request close");
        assert_eq!(error.to_string(), EngineError::Cancelled.to_string());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!backend.component_healthy());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stable_diffusion_cpp_cancellation_interrupts_blocked_request_upload() {
        let root = std::env::temp_dir().join(format!(
            "mayhem-engine-sd-upload-cancel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let model = root.join("model.safetensors");
        fs::write(&model, stable_empty_safetensors()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            accepted_tx.send(()).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            thread::sleep(Duration::from_millis(100));
            let mut buffered = [0_u8; 16 * 1024];
            loop {
                if stream.read(&mut buffered).unwrap() == 0 {
                    break;
                }
            }
        });

        let cancellation = CancellationToken::new();
        let peer_cancellation = cancellation.clone();
        let cancel_thread = thread::spawn(move || {
            accepted_rx.recv().expect("sd-server accepts request");
            thread::sleep(Duration::from_millis(50));
            peer_cancellation.cancel();
        });
        let mut backend = StableDiffusionCppBackend::with_ready_server(
            LoadConfig::stable_diffusion_checkpoint(&model),
            address,
        )
        .unwrap();
        let mut request = ImageGenerationRequest::new("x".repeat(16 * 1024 * 1024));
        request.width = 64;
        request.height = 64;
        request.steps = 9;

        let started = Instant::now();
        let error = backend
            .generate_image(request, &mut NoopArtifactSink, &cancellation)
            .expect_err("cancelled request upload must stop");
        cancel_thread.join().expect("cancel thread");
        server.join().expect("server sees request close");
        assert_eq!(error.to_string(), EngineError::Cancelled.to_string());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!backend.component_healthy());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stable_diffusion_cpp_backend_real_model_smoke_when_enabled() {
        if std::env::var_os("MAYHEM_RUN_STABLE_DIFFUSION_CPP_REAL").is_none() {
            return;
        }
        let model = std::env::var_os("MAYHEM_STABLE_DIFFUSION_MODEL")
            .map(PathBuf::from)
            .expect("MAYHEM_STABLE_DIFFUSION_MODEL must point to a checkpoint");

        let mut backend = StableDiffusionCppBackend::new().unwrap();
        backend
            .load(LoadConfig::stable_diffusion_checkpoint(model))
            .unwrap();
        let mut request = ImageGenerationRequest::new("a blue glass sphere on a white table");
        request.image_count = 1;
        request.width = 512;
        request.height = 512;
        request.steps = 1;
        request.guidance_scale = 1.0;
        request.seed = Some(11);

        let mut artifacts = Vec::new();
        backend
            .generate_image(
                request,
                &mut |chunk| {
                    artifacts.push(chunk);
                    Ok(())
                },
                &CancellationToken::new(),
            )
            .unwrap();

        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].content_type, "image/png");
        assert!(artifacts[0].bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(artifacts[0].bytes.len() > 1024);
    }

    fn serve_sdapi_once(
        expected_body: Value,
        images: Vec<Vec<u8>>,
    ) -> (SocketAddr, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let body = read_http_request_body(&mut stream);
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap(),
                expected_body
            );
            let response = serde_json::to_vec(&json!({
                "images": images
                    .iter()
                    .map(|image| general_purpose::STANDARD.encode(image))
                    .collect::<Vec<_>>(),
                "parameters": expected_body,
                "info": "{}",
            }))
            .unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.len()
            )
            .unwrap();
            stream.write_all(&response).unwrap();
            stream.flush().unwrap();
        });
        (address, server)
    }

    fn read_http_request_body(stream: &mut TcpStream) -> Vec<u8> {
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        assert_eq!(request_line, "POST /sdapi/v1/txt2img HTTP/1.1\r\n");
        let mut content_length = None;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = Some(value.trim().parse::<usize>().unwrap());
                }
            }
        }
        let mut body = vec![0_u8; content_length.expect("request has Content-Length")];
        reader.read_exact(&mut body).unwrap();
        body
    }

    fn assert_argument_value(args: &[String], name: &str, expected: &str) {
        let index = args.iter().position(|arg| arg == name).unwrap();
        assert_eq!(args.get(index + 1).map(String::as_str), Some(expected));
    }

    fn stable_empty_safetensors() -> Vec<u8> {
        let header = b"{}";
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(header);
        bytes
    }
}

#[cfg(all(test, unix))]
mod audio_backend_tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn whisper_cpp_backend_transcribes_with_cli_output() {
        let root =
            std::env::temp_dir().join(format!("mayhem-engine-whisper-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let model = root.join("ggml-tiny.en.bin");
        fs::write(&model, b"ggml-whisper-fixture").unwrap();
        let whisper_cli = root.join("whisper-cli");
        fs::write(
            &whisper_cli,
            r#"#!/bin/sh
out=""
lang=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-of" ]; then
    shift
    out="$1"
  elif [ "$1" = "-l" ]; then
    shift
    lang="$1"
  fi
  shift
done
[ "$lang" = "en" ] || exit 24
printf 'hello mayhem\n' > "$out.txt"
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&whisper_cli).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&whisper_cli, permissions).unwrap();

        let mut backend = WhisperCppBackend::with_binary(&whisper_cli);
        backend.load(LoadConfig::whisper_ggml(&model)).unwrap();
        let output = backend
            .transcribe(
                AudioTranscriptionRequest {
                    audio: tiny_wav_bytes(32_000),
                    content_type: Some("audio/wav".to_owned()),
                    language: Some("en".to_owned()),
                    prompt: None,
                },
                &CancellationToken::new(),
            )
            .unwrap();

        assert_eq!(output.text, "hello mayhem");
        assert_eq!(output.audio_seconds, 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn piper_backend_emits_wav_artifact() {
        let root =
            std::env::temp_dir().join(format!("mayhem-engine-piper-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        let model = root.join("voice.onnx");
        let config = root.join("config").join("voice-config.json");
        let fixture = root.join("fixture.wav");
        fs::write(&model, b"onnx fixture").unwrap();
        fs::write(&config, br#"{"audio":{"sample_rate":16000}}"#).unwrap();
        fs::write(&fixture, tiny_wav_bytes(16_000)).unwrap();
        let piper = root.join("piper");
        fs::write(
            &piper,
            format!(
                r#"#!/bin/sh
out=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-f" ]; then
    shift
    out="$1"
  fi
  shift
done
cp "{}" "$out"
"#,
                fixture.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&piper).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&piper, permissions).unwrap();

        let mut backend = PiperBackend::with_binary(&piper);
        let mut load_config = LoadConfig::piper_voice(&model);
        load_config.piper_config = Some(ModelArtifact::piper_config(&config));
        backend.load(load_config).unwrap();
        let mut artifacts = Vec::new();
        let output = backend
            .synthesize_speech(
                SpeechRequest {
                    input: "hello".to_owned(),
                    voice: Some("launch".to_owned()),
                    response_format: Some("wav".to_owned()),
                    speed: Some(1.0),
                },
                &mut |chunk| {
                    artifacts.push(chunk);
                    Ok(())
                },
                &CancellationToken::new(),
            )
            .unwrap();

        assert_eq!(output.audio_seconds, 1);
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].artifact_id, "speech-1");
        assert_eq!(artifacts[0].content_type, "audio/wav");
        assert!(artifacts[0].bytes.starts_with(b"RIFF"));
        assert!(artifacts[0].final_chunk);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn whisper_cpp_backend_real_model_smoke_when_enabled() {
        if std::env::var_os("MAYHEM_RUN_AUDIO_REAL").is_none() {
            return;
        }
        let binary = std::env::var_os("MAYHEM_WHISPER_CPP_BIN")
            .map(PathBuf::from)
            .expect("MAYHEM_WHISPER_CPP_BIN must point to whisper-cli");
        let model = std::env::var_os("MAYHEM_WHISPER_MODEL")
            .map(PathBuf::from)
            .expect("MAYHEM_WHISPER_MODEL must point to ggml-tiny.en.bin");
        let audio = std::env::var_os("MAYHEM_WHISPER_AUDIO")
            .map(PathBuf::from)
            .expect("MAYHEM_WHISPER_AUDIO must point to a real wav file");

        let mut backend = WhisperCppBackend::with_binary(binary);
        backend.load(LoadConfig::whisper_ggml(model)).unwrap();
        let output = backend
            .transcribe(
                AudioTranscriptionRequest {
                    audio: fs::read(audio).unwrap(),
                    content_type: Some("audio/wav".to_owned()),
                    language: Some("en".to_owned()),
                    prompt: None,
                },
                &CancellationToken::new(),
            )
            .unwrap();

        assert!(
            output.text.to_ascii_lowercase().contains("hello")
                && output.text.to_ascii_lowercase().contains("world"),
            "unexpected transcript: {:?}",
            output.text
        );
        assert!(output.audio_seconds >= 1);
    }

    #[test]
    fn piper_backend_real_model_smoke_when_enabled() {
        if std::env::var_os("MAYHEM_RUN_AUDIO_REAL").is_none() {
            return;
        }
        let binary = std::env::var_os("MAYHEM_PIPER_BIN")
            .map(PathBuf::from)
            .expect("MAYHEM_PIPER_BIN must point to piper");
        let model = std::env::var_os("MAYHEM_PIPER_MODEL")
            .map(PathBuf::from)
            .expect("MAYHEM_PIPER_MODEL must point to a Piper .onnx voice");

        let mut backend = PiperBackend::with_binary(binary);
        backend.load(LoadConfig::piper_voice(model)).unwrap();
        let mut artifacts = Vec::new();
        let output = backend
            .synthesize_speech(
                SpeechRequest {
                    input: "hello mayhem".to_owned(),
                    voice: None,
                    response_format: Some("wav".to_owned()),
                    speed: Some(1.0),
                },
                &mut |chunk| {
                    artifacts.push(chunk);
                    Ok(())
                },
                &CancellationToken::new(),
            )
            .unwrap();

        assert!(output.audio_seconds >= 1);
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].content_type, "audio/wav");
        assert!(artifacts[0].bytes.starts_with(b"RIFF"));
        assert!(artifacts[0].bytes.len() > 1024);
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
}

#[cfg(feature = "llama-cpp")]
mod llama_cpp_backend {
    use base64::{engine::general_purpose, Engine as _};
    use encoding_rs::UTF_8;
    use llama_cpp_2::context::params::{KvCacheType, LlamaContextParams, LlamaPoolingType};
    use llama_cpp_2::llama_backend::LlamaBackend;
    use llama_cpp_2::llama_batch::LlamaBatch;
    use llama_cpp_2::model::params::LlamaModelParams;
    use llama_cpp_2::model::{AddBos, LlamaModel};
    use llama_cpp_2::mtmd::{
        mtmd_default_marker, MtmdBitmap, MtmdContext, MtmdContextParams, MtmdInputChunkType,
        MtmdInputText,
    };
    use llama_cpp_2::sampling::LlamaSampler;
    use llama_cpp_2::token::LlamaToken;

    use super::{
        tool_call_json_schema, validate_load_config, verify_artifact, ArtifactFormat,
        CancellationToken, EmbeddingOutput, EmbeddingRequest, EngineBackend, EngineError,
        FinishReason, GenerateOutput, GenerateRequest, GenerateSpecialityTarget, GrammarSpec,
        LoadConfig, LoadedModelInfo, MediaInput, Result, TokenChunk, TokenSink, Tokenization,
        UsageCounters, DEFAULT_SEED, MTMD_MEDIA_MARKER,
    };
    use std::collections::VecDeque;
    use std::ffi::CString;
    use std::io::{Read, Write};
    use std::num::NonZeroU32;
    use std::ops::Range;
    use std::path::{Path, PathBuf};
    use std::process::{Command, ExitStatus, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};
    use wait_timeout::ChildExt;

    const LLAMA_VIDEO_MAX_FRAMES: u32 = 64;
    const LLAMA_VIDEO_MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
    const LLAMA_VIDEO_MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;
    const LLAMA_VIDEO_MAX_DIMENSION: u32 = 4096;
    const LLAMA_VIDEO_DECODE_TIMEOUT: Duration = Duration::from_secs(30);
    const LLAMA_VIDEO_STDERR_LIMIT: usize = 16 * 1024;

    #[derive(Debug)]
    pub struct LlamaCppBackend {
        backend: LlamaBackend,
        model: Option<LlamaModel>,
        mtmd: Option<MtmdContext>,
        mtmd_image_token_budget: Option<i32>,
        loaded: Option<LoadedModelInfo>,
        config: Option<LoadConfig>,
        media_python: PathBuf,
    }

    impl LlamaCppBackend {
        pub fn new() -> Result<Self> {
            let python = std::env::var_os("MAYHEM_LLAMA_MEDIA_PYTHON")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("python3"));
            Self::with_media_python(python)
        }

        pub fn with_media_python(python: impl AsRef<Path>) -> Result<Self> {
            let mut backend = LlamaBackend::init()?;
            if std::env::var_os("MAYHEM_LLAMA_LOGS").is_none() {
                backend.void_logs();
            }
            Ok(Self {
                backend,
                model: None,
                mtmd: None,
                mtmd_image_token_budget: None,
                loaded: None,
                config: None,
                media_python: python.as_ref().to_path_buf(),
            })
        }

        fn model(&self) -> Result<&LlamaModel> {
            self.model.as_ref().ok_or(EngineError::NotLoaded)
        }

        fn config(&self) -> Result<&LoadConfig> {
            self.config.as_ref().ok_or(EngineError::NotLoaded)
        }
    }

    fn llama_kv_cache_type(value: &str) -> Result<KvCacheType> {
        match value {
            "f32" => Ok(KvCacheType::F32),
            "f16" => Ok(KvCacheType::F16),
            "q4_0" => Ok(KvCacheType::Q4_0),
            "q4_1" => Ok(KvCacheType::Q4_1),
            "q5_0" => Ok(KvCacheType::Q5_0),
            "q5_1" => Ok(KvCacheType::Q5_1),
            "q8_0" => Ok(KvCacheType::Q8_0),
            "iq4_nl" => Ok(KvCacheType::IQ4_NL),
            other => Err(EngineError::InvalidConfig(format!(
                "unsupported llama.cpp KV-cache dtype {other}"
            ))),
        }
    }

    fn llama_context_params(
        config: &LoadConfig,
        ctx_size: NonZeroU32,
    ) -> Result<LlamaContextParams> {
        // llama.cpp's CLI defaults to the compact SWA cache; its low-level API default does not.
        let mut params = LlamaContextParams::default()
            .with_n_ctx(Some(ctx_size))
            .with_n_batch(config.batch_size)
            .with_n_ubatch(config.ubatch_size)
            .with_n_seq_max(1)
            .with_swa_full(false)
            .with_no_perf(true);
        if let Some(threads) = config.threads {
            params = params.with_n_threads(threads).with_n_threads_batch(threads);
        }
        if let Some(dtype) = config.kv_cache_dtype.as_deref() {
            let dtype = llama_kv_cache_type(dtype)?;
            params = params.with_type_k(dtype).with_type_v(dtype);
        }
        Ok(params)
    }

    fn llama_prompt_batch_ranges(token_count: usize, n_batch: u32) -> Result<Vec<Range<usize>>> {
        let batch_size = usize::try_from(n_batch).map_err(|error| {
            EngineError::InvalidConfig(format!("llama.cpp batch size overflow: {error}"))
        })?;
        if batch_size == 0 {
            return Err(EngineError::InvalidConfig(
                "llama.cpp batch size must be greater than zero".to_owned(),
            ));
        }
        if token_count == 0 {
            return Err(EngineError::InvalidConfig(
                "llama.cpp prompt tokenization produced no tokens".to_owned(),
            ));
        }
        Ok((0..token_count)
            .step_by(batch_size)
            .map(|start| start..start.saturating_add(batch_size).min(token_count))
            .collect())
    }

    impl EngineBackend for LlamaCppBackend {
        fn backend_id(&self) -> &'static str {
            "llama.cpp"
        }

        fn load(&mut self, config: LoadConfig) -> Result<LoadedModelInfo> {
            validate_load_config(&config)?;
            if config.artifact.format != ArtifactFormat::Gguf {
                return Err(EngineError::InvalidConfig(format!(
                    "llama.cpp backend requires GGUF artifacts, got {:?}",
                    config.artifact.format
                )));
            }
            verify_artifact(&config.artifact)?;

            let mut model_params = LlamaModelParams::default()
                .with_use_mmap(config.use_mmap)
                .with_use_mlock(config.use_mlock);
            if let Some(gpu_layers) = config.gpu_layers {
                model_params = model_params.with_n_gpu_layers(gpu_layers);
            }

            let model =
                LlamaModel::load_from_file(&self.backend, &config.artifact.path, &model_params)?;
            let mtmd = if let Some(projector) = &config.vision_projector {
                verify_artifact(projector)?;
                let projector_path = projector.path.to_str().ok_or_else(|| {
                    EngineError::InvalidConfig(format!(
                        "vision projector path {} is not valid UTF-8",
                        projector.path.display()
                    ))
                })?;
                let mut params = MtmdContextParams::default();
                params.use_gpu = config.gpu_layers.unwrap_or(0) > 0;
                params.print_timings = false;
                if let Some(threads) = config.threads {
                    params.n_threads = threads;
                }
                params.media_marker = CString::new(mtmd_default_marker()).map_err(|err| {
                    EngineError::InvalidConfig(format!("invalid mtmd media marker: {err}"))
                })?;
                let mtmd = MtmdContext::init_from_file(projector_path, &model, &params).map_err(
                    |err| {
                        EngineError::InvalidConfig(format!(
                            "initializing llama.cpp vision projector {} failed: {err}",
                            projector.path.display()
                        ))
                    },
                )?;
                if !mtmd.support_vision() && !mtmd.support_audio() {
                    return Err(EngineError::InvalidConfig(format!(
                        "multimodal projector {} advertises neither vision nor audio support",
                        projector.path.display()
                    )));
                }
                Some(mtmd)
            } else {
                None
            };
            let info = LoadedModelInfo {
                backend: self.backend_id().to_owned(),
                artifact: config.artifact.clone(),
                ctx_size: config.ctx_size,
                n_ctx_train: model.n_ctx_train(),
                n_vocab: model.n_vocab(),
            };

            self.model = Some(model);
            self.mtmd = mtmd;
            self.mtmd_image_token_budget = None;
            self.loaded = Some(info.clone());
            self.config = Some(config);
            Ok(info)
        }

        fn tokenize(&self, text: &str) -> Result<Tokenization> {
            let tokens = self.model()?.str_to_token(text, AddBos::Always)?;
            Ok(Tokenization {
                token_ids: tokens.into_iter().map(|token| token.0).collect(),
            })
        }

        fn generate(
            &mut self,
            request: GenerateRequest,
            sink: &mut dyn TokenSink,
            cancellation: &CancellationToken,
        ) -> Result<GenerateOutput> {
            cancellation.check()?;
            request.validate_sampling()?;
            let mut request = request;
            apply_llama_speciality_parameters(&mut request)?;
            let reasoning_budget = llama_reasoning_budget(&request)?;
            if request.max_new_tokens == 0 {
                return Ok(GenerateOutput {
                    text: String::new(),
                    usage: UsageCounters::default(),
                    finish_reason: FinishReason::Length,
                });
            }
            if !request.media.is_empty() {
                return self.generate_multimodal(request, sink, cancellation);
            }

            let config = self.config()?.clone();
            let model = self.model()?;
            let ctx_size = NonZeroU32::new(config.ctx_size).ok_or_else(|| {
                EngineError::InvalidConfig("ctx_size must be greater than zero".to_owned())
            })?;
            let ctx_params = llama_context_params(&config, ctx_size)?;
            let mut ctx = model.new_context(&self.backend, ctx_params)?;
            let prompt_tokens = model.str_to_token(&request.prompt, AddBos::Always)?;
            if prompt_tokens.len() >= ctx.n_ctx() as usize {
                return Err(EngineError::PromptTooLong {
                    prompt_tokens: prompt_tokens.len(),
                    ctx_size: ctx.n_ctx(),
                });
            }

            let batch_ranges = llama_prompt_batch_ranges(prompt_tokens.len(), ctx.n_batch())?;
            let batch_capacity = batch_ranges
                .iter()
                .map(|range| range.len())
                .max()
                .unwrap_or(1);
            let mut batch = LlamaBatch::new(batch_capacity, 1);
            let last_prompt_index = prompt_tokens.len().saturating_sub(1);
            for range in batch_ranges {
                cancellation.check()?;
                batch.clear();
                for index in range {
                    batch.add(
                        prompt_tokens[index],
                        i32::try_from(index).map_err(|err| {
                            EngineError::InvalidConfig(format!("prompt position overflow: {err}"))
                        })?,
                        &[0],
                        index == last_prompt_index,
                    )?;
                }
                ctx.decode(&mut batch)?;
                cancellation.check()?;
            }

            let mut sampler = make_sampler(model, &request)?;
            let mut decoder = UTF_8.new_decoder();
            let mut stop_stream = StopSequenceStream::new(&request.stop);
            let mut completion_tokens = 0_u32;
            let mut finish_reason = FinishReason::Length;
            let mut reasoning_text = String::new();
            let mut forced_reasoning_tokens = VecDeque::new();
            let mut next_pos = i32::try_from(prompt_tokens.len()).map_err(|err| {
                EngineError::InvalidConfig(format!("prompt token count overflow: {err}"))
            })?;

            while completion_tokens < request.max_new_tokens {
                cancellation.check()?;
                if forced_reasoning_tokens.is_empty()
                    && reasoning_budget.is_some_and(|budget| completion_tokens >= budget)
                    && !llama_reasoning_has_closed(&reasoning_text)
                {
                    forced_reasoning_tokens
                        .extend(model.str_to_token("</think>\n\n", AddBos::Never)?);
                }
                let token = if let Some(token) = forced_reasoning_tokens.pop_front() {
                    sampler.accept(token);
                    token
                } else {
                    sampler.sample(&ctx, batch.n_tokens() - 1)
                };
                if model.is_eog_token(token) && !request.ignore_eos {
                    stop_stream.finish(sink)?;
                    finish_reason = FinishReason::Stop;
                    break;
                }

                let text = model.token_to_piece(token, &mut decoder, true, None)?;
                reasoning_text.push_str(&text);
                let stopped = stop_stream.push(
                    TokenChunk {
                        index: completion_tokens,
                        token_id: token.0,
                        text,
                    },
                    sink,
                )?;
                completion_tokens = completion_tokens.saturating_add(1);
                if stopped {
                    finish_reason = FinishReason::Stop;
                    break;
                }

                if completion_tokens >= request.max_new_tokens {
                    break;
                }
                if next_pos >= i32::try_from(ctx.n_ctx()).unwrap_or(i32::MAX) {
                    break;
                }

                batch.clear();
                batch.add(token, next_pos, &[0], true)?;
                next_pos = next_pos.saturating_add(1);
                ctx.decode(&mut batch)?;
                cancellation.check()?;
            }
            cancellation.check()?;
            stop_stream.finish(sink)?;

            let mut usage = UsageCounters::new(prompt_tokens.len() as u32, completion_tokens);
            usage.reasoning_tokens =
                llama_reasoning_tokens(model, &request, &stop_stream.output, completion_tokens);
            Ok(GenerateOutput {
                text: stop_stream.output,
                usage,
                finish_reason,
            })
        }

        fn embed(
            &mut self,
            request: EmbeddingRequest,
            cancellation: &CancellationToken,
        ) -> Result<EmbeddingOutput> {
            cancellation.check()?;
            if request.inputs.is_empty() {
                return Err(EngineError::InvalidConfig(
                    "embedding request must include at least one input".to_owned(),
                ));
            }
            if request.dimensions == Some(0) {
                return Err(EngineError::InvalidConfig(
                    "embedding dimensions must be greater than zero".to_owned(),
                ));
            }

            let config = self.config()?.clone();
            let model = self.model()?;
            let ctx_size = NonZeroU32::new(config.ctx_size).ok_or_else(|| {
                EngineError::InvalidConfig("ctx_size must be greater than zero".to_owned())
            })?;
            let mut embeddings = Vec::with_capacity(request.inputs.len());
            let mut prompt_tokens = 0_u32;

            for input in &request.inputs {
                cancellation.check()?;
                let input_tokens = model.str_to_token(input, AddBos::Always)?;
                if input_tokens.len() >= config.ctx_size as usize {
                    return Err(EngineError::PromptTooLong {
                        prompt_tokens: input_tokens.len(),
                        ctx_size: config.ctx_size,
                    });
                }
                prompt_tokens = prompt_tokens
                    .saturating_add(u32::try_from(input_tokens.len()).unwrap_or(u32::MAX));

                let ctx_params = llama_context_params(&config, ctx_size)?
                    .with_pooling_type(LlamaPoolingType::Mean)
                    .with_embeddings(true);

                let mut ctx = model.new_context(&self.backend, ctx_params)?;
                let mut batch = LlamaBatch::new(input_tokens.len().max(1), 1);
                let last_prompt_index = input_tokens.len().saturating_sub(1);
                for (index, token) in input_tokens.iter().enumerate() {
                    batch.add(
                        *token,
                        i32::try_from(index).map_err(|err| {
                            EngineError::InvalidConfig(format!(
                                "embedding position overflow: {err}"
                            ))
                        })?,
                        &[0],
                        index == last_prompt_index,
                    )?;
                }
                cancellation.check()?;
                ctx.encode(&mut batch)?;
                cancellation.check()?;
                let mut embedding = ctx.embeddings_seq_ith(0)?.to_vec();
                if let Some(dimensions) = request.dimensions {
                    if dimensions > embedding.len() {
                        return Err(EngineError::InvalidConfig(format!(
                            "requested embedding dimensions {dimensions} exceed model dimensions {}",
                            embedding.len()
                        )));
                    }
                    embedding.truncate(dimensions);
                }
                normalize_embedding(&mut embedding);
                embeddings.push(embedding);
            }

            Ok(EmbeddingOutput {
                embeddings,
                usage: UsageCounters::new(prompt_tokens, 0),
            })
        }
    }

    impl LlamaCppBackend {
        fn generate_multimodal(
            &mut self,
            request: GenerateRequest,
            sink: &mut dyn TokenSink,
            cancellation: &CancellationToken,
        ) -> Result<GenerateOutput> {
            cancellation.check()?;
            let image_token_budget = llama_mtmd_image_token_budget(&request)?;
            let reasoning_budget = llama_reasoning_budget(&request)?;
            self.ensure_mtmd_image_token_budget(image_token_budget)?;
            let config = self.config()?.clone();
            let model = self.model()?;
            let mtmd = self.mtmd.as_ref().ok_or_else(|| {
                EngineError::InvalidConfig(
                    "llama.cpp received media input but no admin-approved mmproj sidecar is loaded"
                        .to_owned(),
                )
            })?;
            let ctx_size = NonZeroU32::new(config.ctx_size).ok_or_else(|| {
                EngineError::InvalidConfig("ctx_size must be greater than zero".to_owned())
            })?;
            let ctx_params = llama_context_params(&config, ctx_size)?;
            let mut ctx = model.new_context(&self.backend, ctx_params)?;
            let bitmaps = media_bitmaps(mtmd, &request.media, &self.media_python, cancellation)?;
            cancellation.check()?;
            let bitmap_refs = bitmaps.iter().collect::<Vec<_>>();
            let marker_count = request.prompt.matches(MTMD_MEDIA_MARKER).count();
            if marker_count != bitmap_refs.len() {
                return Err(EngineError::InvalidConfig(format!(
                    "multimodal prompt must contain exactly {} {} marker(s), found {}",
                    bitmap_refs.len(),
                    MTMD_MEDIA_MARKER,
                    marker_count
                )));
            }
            let chunks = mtmd
                .tokenize(
                    MtmdInputText {
                        text: request.prompt.clone(),
                        add_special: true,
                        parse_special: true,
                    },
                    &bitmap_refs,
                )
                .map_err(|err| {
                    EngineError::InvalidConfig(format!("llama.cpp mtmd tokenization failed: {err}"))
                })?;
            cancellation.check()?;
            let prompt_tokens = chunks.total_tokens();
            let prompt_positions = chunks.total_positions();
            if prompt_positions >= i32::try_from(ctx.n_ctx()).unwrap_or(i32::MAX) {
                return Err(EngineError::PromptTooLong {
                    prompt_tokens,
                    ctx_size: ctx.n_ctx(),
                });
            }
            let mut next_pos = chunks
                .eval_chunks(
                    mtmd,
                    &ctx,
                    0,
                    0,
                    i32::try_from(config.batch_size).unwrap_or(i32::MAX),
                    true,
                )
                .map_err(|err| {
                    EngineError::InvalidConfig(format!("llama.cpp mtmd prompt eval failed: {err}"))
                })?;
            cancellation.check()?;

            let mut sampler = make_sampler(model, &request)?;
            let mut decoder = UTF_8.new_decoder();
            let mut stop_stream = StopSequenceStream::new(&request.stop);
            let mut completion_tokens = 0_u32;
            let mut finish_reason = FinishReason::Length;
            let mut reasoning_text = String::new();
            let mut forced_reasoning_tokens = VecDeque::new();
            let mut batch = LlamaBatch::new(1, 1);

            while completion_tokens < request.max_new_tokens {
                cancellation.check()?;
                if forced_reasoning_tokens.is_empty()
                    && reasoning_budget.is_some_and(|budget| completion_tokens >= budget)
                    && !llama_reasoning_has_closed(&reasoning_text)
                {
                    forced_reasoning_tokens
                        .extend(model.str_to_token("</think>\n\n", AddBos::Never)?);
                }
                let token = if let Some(token) = forced_reasoning_tokens.pop_front() {
                    sampler.accept(token);
                    token
                } else {
                    sampler.sample(&ctx, -1)
                };
                if model.is_eog_token(token) && !request.ignore_eos {
                    stop_stream.finish(sink)?;
                    finish_reason = FinishReason::Stop;
                    break;
                }

                let text = model.token_to_piece(token, &mut decoder, true, None)?;
                reasoning_text.push_str(&text);
                let stopped = stop_stream.push(
                    TokenChunk {
                        index: completion_tokens,
                        token_id: token.0,
                        text,
                    },
                    sink,
                )?;
                completion_tokens = completion_tokens.saturating_add(1);
                if stopped {
                    finish_reason = FinishReason::Stop;
                    break;
                }

                if completion_tokens >= request.max_new_tokens {
                    break;
                }
                if next_pos >= i32::try_from(ctx.n_ctx()).unwrap_or(i32::MAX) {
                    break;
                }

                batch.clear();
                batch.add(token, next_pos, &[0], true)?;
                next_pos = next_pos.saturating_add(1);
                ctx.decode(&mut batch)?;
                cancellation.check()?;
            }
            cancellation.check()?;
            stop_stream.finish(sink)?;

            let mut usage = UsageCounters::new(prompt_tokens as u32, completion_tokens);
            usage.reasoning_tokens =
                llama_reasoning_tokens(model, &request, &stop_stream.output, completion_tokens);
            for index in 0..chunks.len() {
                let Some(chunk) = chunks.get(index) else {
                    continue;
                };
                let tokens = u32::try_from(chunk.n_tokens()).unwrap_or(u32::MAX);
                match chunk.chunk_type() {
                    MtmdInputChunkType::Image => {
                        usage.vision_tokens = usage.vision_tokens.saturating_add(tokens);
                    }
                    MtmdInputChunkType::Audio => {
                        usage.audio_tokens = usage.audio_tokens.saturating_add(tokens);
                    }
                    MtmdInputChunkType::Text => {}
                }
            }
            Ok(GenerateOutput {
                text: stop_stream.output,
                usage,
                finish_reason,
            })
        }

        fn ensure_mtmd_image_token_budget(&mut self, budget: Option<i32>) -> Result<()> {
            if self.mtmd_image_token_budget == budget {
                return Ok(());
            }
            let config = self.config()?.clone();
            let projector = config.vision_projector.as_ref().ok_or_else(|| {
                EngineError::InvalidConfig(
                    "llama.cpp received a visual-token budget without an mmproj sidecar".to_owned(),
                )
            })?;
            verify_artifact(projector)?;
            let projector_path = projector.path.to_str().ok_or_else(|| {
                EngineError::InvalidConfig(format!(
                    "multimodal projector path {} is not valid UTF-8",
                    projector.path.display()
                ))
            })?;
            self.mtmd.take();
            let model = self.model()?;
            let mut params = MtmdContextParams::default();
            params.use_gpu = config.gpu_layers.unwrap_or(0) > 0;
            params.print_timings = false;
            if let Some(threads) = config.threads {
                params.n_threads = threads;
            }
            params.media_marker = CString::new(mtmd_default_marker()).map_err(|err| {
                EngineError::InvalidConfig(format!("invalid mtmd media marker: {err}"))
            })?;
            if let Some(budget) = budget {
                params.image_min_tokens = budget;
                params.image_max_tokens = budget;
            }
            let mtmd =
                MtmdContext::init_from_file(projector_path, model, &params).map_err(|err| {
                    EngineError::InvalidConfig(format!(
                        "initializing llama.cpp multimodal projector {} failed: {err}",
                        projector.path.display()
                    ))
                })?;
            if !mtmd.support_vision() && !mtmd.support_audio() {
                return Err(EngineError::InvalidConfig(format!(
                    "multimodal projector {} advertises neither vision nor audio support",
                    projector.path.display()
                )));
            }
            self.mtmd = Some(mtmd);
            self.mtmd_image_token_budget = budget;
            Ok(())
        }
    }

    fn llama_mtmd_image_token_budget(request: &GenerateRequest) -> Result<Option<i32>> {
        let mut budget = None;
        for speciality in &request.speciality_parameters {
            if speciality.target != GenerateSpecialityTarget::BackendParameter
                || speciality.native_path != "mtmd.image_token_budget"
            {
                continue;
            }
            let value = speciality.value.as_i64().ok_or_else(|| {
                EngineError::InvalidConfig(format!(
                    "llama.cpp image-token budget speciality {} must map to an integer",
                    speciality.name
                ))
            })?;
            let value = i32::try_from(value).map_err(|_| {
                EngineError::InvalidConfig("llama.cpp image-token budget exceeds i32".to_owned())
            })?;
            if !matches!(value, 70 | 140 | 280 | 560 | 1120) {
                return Err(EngineError::InvalidConfig(format!(
                    "unsupported llama.cpp image-token budget {value}"
                )));
            }
            if budget.replace(value).is_some() {
                return Err(EngineError::InvalidConfig(
                    "llama.cpp received duplicate image-token budget specialities".to_owned(),
                ));
            }
        }
        Ok(budget)
    }

    fn apply_llama_speciality_parameters(request: &mut GenerateRequest) -> Result<()> {
        for speciality in &request.speciality_parameters {
            match speciality.target {
                GenerateSpecialityTarget::PromptSuffix => {
                    let suffix = speciality.value.as_str().ok_or_else(|| {
                        EngineError::InvalidConfig(format!(
                            "llama.cpp prompt-suffix speciality {} must map to a string",
                            speciality.name
                        ))
                    })?;
                    request.prompt.push_str(suffix);
                }
                GenerateSpecialityTarget::ChatTemplateKwarg => {
                    let enabled = speciality.value.as_bool().ok_or_else(|| {
                        EngineError::InvalidConfig(format!(
                            "llama.cpp chat-template speciality {} ({}) must map to a boolean",
                            speciality.name, speciality.native_path
                        ))
                    })?;
                    match speciality.native_path.as_str() {
                        "enable_thinking" => {
                            let prompt_enables_thinking =
                                llama_prompt_enables_thinking(&request.prompt);
                            if enabled != prompt_enables_thinking {
                                return Err(EngineError::InvalidConfig(format!(
                                    "llama.cpp prompt did not apply enable_thinking={} for speciality {}",
                                    enabled, speciality.name
                                )));
                            }
                        }
                        "preserve_thinking" => {
                            validate_llama_thinking_history(request, enabled, &speciality.name)?;
                        }
                        _ => {
                            return Err(EngineError::InvalidConfig(format!(
                                "llama.cpp artifact cannot apply chat-template speciality {} ({}) through this backend",
                                speciality.name, speciality.native_path
                            )));
                        }
                    }
                }
                GenerateSpecialityTarget::SamplingParameter => {
                    return Err(EngineError::InvalidConfig(format!(
                        "llama.cpp artifact does not support dynamic sampling speciality {} ({})",
                        speciality.name, speciality.native_path
                    )));
                }
                GenerateSpecialityTarget::BackendParameter => {
                    if speciality.native_path != "mtmd.image_token_budget"
                        || !matches!(speciality.value.as_i64(), Some(70 | 140 | 280 | 560 | 1120))
                    {
                        return Err(EngineError::InvalidConfig(format!(
                            "llama.cpp artifact does not support backend speciality {} ({}) at value {}",
                            speciality.name, speciality.native_path, speciality.value
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    fn llama_prompt_enables_thinking(prompt: &str) -> bool {
        let trimmed_prompt = prompt.trim_end();
        trimmed_prompt.ends_with("<think>")
            || trimmed_prompt.ends_with("<|think|>")
            || prompt.starts_with("<|turn>system\n<|think|>\n")
    }

    fn validate_llama_thinking_history(
        request: &GenerateRequest,
        preserve: bool,
        speciality_name: &str,
    ) -> Result<()> {
        let fragments = llama_prior_reasoning_fragments(request);
        let prompt_preserves = fragments
            .iter()
            .all(|fragment| request.prompt.contains(fragment));
        let prompt_strips = fragments
            .iter()
            .all(|fragment| !request.prompt.contains(fragment));
        if !fragments.is_empty()
            && ((preserve && !prompt_preserves) || (!preserve && !prompt_strips))
        {
            return Err(EngineError::InvalidConfig(format!(
                "llama.cpp prompt did not apply preserve_thinking={preserve} for speciality {speciality_name}"
            )));
        }
        Ok(())
    }

    fn llama_prior_reasoning_fragments(request: &GenerateRequest) -> Vec<String> {
        let mut fragments = Vec::new();
        for message in &request.messages {
            if message.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
                continue;
            }
            for field in ["reasoning_content", "reasoning"] {
                if let Some(value) = message
                    .get(field)
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    fragments.push(value.to_owned());
                }
            }
            let Some(content) = message.get("content").and_then(serde_json::Value::as_str) else {
                continue;
            };
            for (open, close) in [("<think>", "</think>"), ("<|think|>", "<|/think|>")] {
                let mut rest = content;
                while let Some(start) = rest.find(open) {
                    let after_open = &rest[start + open.len()..];
                    let Some(end) = after_open.find(close) else {
                        break;
                    };
                    let value = after_open[..end].trim();
                    if !value.is_empty() {
                        fragments.push(value.to_owned());
                    }
                    rest = &after_open[end + close.len()..];
                }
            }
        }
        fragments
    }

    fn llama_reasoning_budget(request: &GenerateRequest) -> Result<Option<u32>> {
        let mut budget = None;
        for speciality in &request.speciality_parameters {
            let name = speciality.name.to_ascii_lowercase();
            let native_path = speciality.native_path.to_ascii_lowercase();
            let relevant = (name.contains("reason")
                || name.contains("think")
                || native_path.contains("reason")
                || native_path.contains("think"))
                && !["preserve", "history", "retain"]
                    .iter()
                    .any(|marker| name.contains(marker) || native_path.contains(marker));
            let Some(limit) = speciality.max_reasoning_tokens else {
                continue;
            };
            if !relevant {
                continue;
            }
            let enabled = !matches!(
                speciality.level.to_ascii_lowercase().as_str(),
                "none" | "off" | "disabled"
            ) && speciality.value != serde_json::Value::Bool(false)
                && speciality.value.as_u64() != Some(0);
            if !enabled {
                continue;
            }
            if budget.replace(limit).is_some() {
                return Err(EngineError::InvalidConfig(
                    "llama.cpp received duplicate reasoning-budget specialities".to_owned(),
                ));
            }
        }
        Ok(budget)
    }

    fn llama_reasoning_has_closed(text: &str) -> bool {
        text.contains("</think>") || text.contains("<|channel|>")
    }

    fn llama_reasoning_tokens(
        model: &LlamaModel,
        request: &GenerateRequest,
        text: &str,
        completion_tokens: u32,
    ) -> u32 {
        let enabled = request.speciality_parameters.iter().any(|speciality| {
            let name = speciality.name.to_ascii_lowercase();
            let native_path = speciality.native_path.to_ascii_lowercase();
            let relevant = name.contains("reason")
                || name.contains("think")
                || native_path.contains("reason")
                || native_path.contains("think");
            let disabled_level = matches!(
                speciality.level.to_ascii_lowercase().as_str(),
                "none" | "off" | "disabled"
            );
            let disabled_value = speciality.value == serde_json::Value::Bool(false)
                || speciality.value.as_u64() == Some(0)
                || speciality.value.as_str().is_some_and(|value| {
                    matches!(
                        value.to_ascii_lowercase().as_str(),
                        "none" | "off" | "disabled" | "false"
                    )
                });
            relevant && !disabled_level && !disabled_value
        });
        if !enabled {
            return 0;
        }
        let close = ["</think>", "<channel|>"]
            .into_iter()
            .filter_map(|marker| text.find(marker).map(|index| (index, marker)))
            .min_by_key(|(index, _)| *index);
        let Some((index, marker)) = close else {
            return completion_tokens;
        };
        let prefix = &text[..index];
        let attributed = model
            .str_to_token(&format!("{prefix}{marker}"), AddBos::Never)
            .map(|tokens| u32::try_from(tokens.len()).unwrap_or(u32::MAX))
            .unwrap_or(completion_tokens);
        let attributed = attributed.min(completion_tokens);
        llama_reasoning_budget(request)
            .ok()
            .flatten()
            .map_or(attributed, |budget| attributed.min(budget))
    }

    fn media_bitmaps(
        mtmd: &MtmdContext,
        media: &[MediaInput],
        media_python: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Vec<MtmdBitmap>> {
        let mut bitmaps = Vec::new();
        for input in media {
            match input.kind.as_str() {
                "image" if !mtmd.support_vision() => {
                    return Err(EngineError::InvalidConfig(
                        "llama.cpp mmproj does not advertise image support".to_owned(),
                    ));
                }
                "audio" if !mtmd.support_audio() => {
                    return Err(EngineError::InvalidConfig(
                        "llama.cpp mmproj does not advertise audio support".to_owned(),
                    ));
                }
                "image" | "audio" => {
                    if !input.frames.is_empty() {
                        return Err(EngineError::InvalidConfig(format!(
                            "llama.cpp {} media must not contain video frames",
                            input.kind
                        )));
                    }
                    let bytes = media_input_bytes(input)?;
                    bitmaps.push(MtmdBitmap::from_buffer(mtmd, &bytes, false).map_err(|err| {
                        EngineError::InvalidConfig(format!(
                            "llama.cpp mtmd {} decode failed: {err}",
                            input.kind
                        ))
                    })?);
                }
                "video" => {
                    if !mtmd.support_vision() {
                        return Err(EngineError::InvalidConfig(
                            "llama.cpp mmproj does not advertise video-frame vision support"
                                .to_owned(),
                        ));
                    }
                    if !input.frames.is_empty() && (input.data.is_some() || input.url.is_some()) {
                        return Err(EngineError::InvalidConfig(
                            "llama.cpp video must use decoded frames or a container, not both"
                                .to_owned(),
                        ));
                    }
                    if input.frames.is_empty() {
                        let frames =
                            decode_llama_video_container(input, media_python, cancellation)?;
                        for (width, height, pixels) in frames {
                            bitmaps.push(
                                MtmdBitmap::from_image_data(width, height, &pixels).map_err(
                                    |err| {
                                        EngineError::InvalidConfig(format!(
                                        "llama.cpp mtmd decoded video-frame import failed: {err}"
                                    ))
                                    },
                                )?,
                            );
                        }
                    } else {
                        if input.num_frames.is_some_and(|declared| {
                            usize::try_from(declared).ok() != Some(input.frames.len())
                        }) {
                            return Err(EngineError::InvalidConfig(
                                "llama.cpp video frame count does not match decoded frames"
                                    .to_owned(),
                            ));
                        }
                        for frame in &input.frames {
                            let bytes = decode_data_url(frame)?;
                            bitmaps.push(MtmdBitmap::from_buffer(mtmd, &bytes, false).map_err(
                                |err| {
                                    EngineError::InvalidConfig(format!(
                                        "llama.cpp mtmd video-frame decode failed: {err}"
                                    ))
                                },
                            )?);
                        }
                    }
                }
                other => {
                    return Err(EngineError::InvalidConfig(format!(
                        "llama.cpp mtmd does not support media kind {other}"
                    )));
                }
            }
        }
        Ok(bitmaps)
    }

    fn decode_llama_video_container(
        input: &MediaInput,
        media_python: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Vec<(u32, u32, Vec<u8>)>> {
        let requested = input.num_frames.ok_or_else(|| {
            EngineError::InvalidConfig(
                "llama.cpp video container requires an explicit num_frames bound".to_owned(),
            )
        })?;
        if !(1..=LLAMA_VIDEO_MAX_FRAMES).contains(&requested) {
            return Err(EngineError::InvalidConfig(format!(
                "llama.cpp video num_frames must be between 1 and {LLAMA_VIDEO_MAX_FRAMES}"
            )));
        }
        let fps = input.fps.unwrap_or(1.0);
        if !fps.is_finite() || fps <= 0.0 {
            return Err(EngineError::InvalidConfig(
                "llama.cpp video fps must be a positive finite number".to_owned(),
            ));
        }
        let encoded_limit = LLAMA_VIDEO_MAX_INPUT_BYTES
            .saturating_mul(4)
            .saturating_div(3)
            .saturating_add(1024);
        if input
            .data
            .as_ref()
            .is_some_and(|value| value.len() > encoded_limit)
            || input
                .url
                .as_ref()
                .is_some_and(|value| value.len() > encoded_limit)
        {
            return Err(EngineError::InvalidConfig(
                "llama.cpp video container exceeds the bounded input size".to_owned(),
            ));
        }
        let payload = media_input_bytes(input)?;
        if payload.is_empty() || payload.len() > LLAMA_VIDEO_MAX_INPUT_BYTES {
            return Err(EngineError::InvalidConfig(
                "llama.cpp video container exceeds the bounded input size".to_owned(),
            ));
        }
        cancellation.check()?;

        let mut child = Command::new(media_python)
            .args([
                "-I",
                "-B",
                "-c",
                include_str!("llama_video_decode.py"),
                &requested.to_string(),
                &fps.to_string(),
                &LLAMA_VIDEO_MAX_INPUT_BYTES.to_string(),
                &LLAMA_VIDEO_MAX_DECODED_BYTES.to_string(),
                &LLAMA_VIDEO_MAX_DIMENSION.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                EngineError::InvalidConfig(format!(
                    "starting the managed llama.cpp video decoder {} failed: {error}",
                    media_python.display()
                ))
            })?;
        let mut stdin = child.stdin.take().ok_or_else(|| {
            EngineError::InvalidConfig("opening llama.cpp video decoder stdin failed".to_owned())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            EngineError::InvalidConfig("opening llama.cpp video decoder stdout failed".to_owned())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            EngineError::InvalidConfig("opening llama.cpp video decoder stderr failed".to_owned())
        })?;
        let input_writer = thread::spawn(move || stdin.write_all(&payload));
        let output_limit =
            LLAMA_VIDEO_MAX_DECODED_BYTES.saturating_add(12 * LLAMA_VIDEO_MAX_FRAMES as usize);
        let stdout_reader = thread::spawn(move || read_bounded(stdout, output_limit));
        let stderr_reader = thread::spawn(move || read_bounded(stderr, LLAMA_VIDEO_STDERR_LIMIT));

        enum WaitOutcome {
            Completed(ExitStatus),
            Cancelled,
            TimedOut,
        }
        let started = Instant::now();
        let outcome = loop {
            if cancellation.is_cancelled() {
                break WaitOutcome::Cancelled;
            }
            let elapsed = started.elapsed();
            if elapsed >= LLAMA_VIDEO_DECODE_TIMEOUT {
                break WaitOutcome::TimedOut;
            }
            let remaining = LLAMA_VIDEO_DECODE_TIMEOUT.saturating_sub(elapsed);
            match child
                .wait_timeout(remaining.min(Duration::from_millis(50)))
                .map_err(|error| {
                    EngineError::InvalidConfig(format!(
                        "waiting for the managed llama.cpp video decoder failed: {error}"
                    ))
                })? {
                Some(status) => break WaitOutcome::Completed(status),
                None => continue,
            }
        };
        if !matches!(&outcome, WaitOutcome::Completed(_)) {
            let _ = child.kill();
            let _ = child.wait();
        }
        let input_result = input_writer.join().map_err(|_| {
            EngineError::InvalidConfig("llama.cpp video decoder input thread panicked".to_owned())
        })?;
        let output = stdout_reader.join().map_err(|_| {
            EngineError::InvalidConfig("llama.cpp video decoder output thread panicked".to_owned())
        })??;
        let stderr = stderr_reader.join().map_err(|_| {
            EngineError::InvalidConfig("llama.cpp video decoder stderr thread panicked".to_owned())
        })??;

        match outcome {
            WaitOutcome::Cancelled => return Err(EngineError::Cancelled),
            WaitOutcome::TimedOut => {
                return Err(EngineError::InvalidConfig(format!(
                    "llama.cpp video decode exceeded {} seconds",
                    LLAMA_VIDEO_DECODE_TIMEOUT.as_secs()
                )))
            }
            WaitOutcome::Completed(status) => {
                if output.len() > output_limit {
                    return Err(EngineError::InvalidConfig(
                        "llama.cpp video decoder exceeded the bounded output size".to_owned(),
                    ));
                }
                if stderr.len() > LLAMA_VIDEO_STDERR_LIMIT {
                    return Err(EngineError::InvalidConfig(
                        "llama.cpp video decoder exceeded the bounded diagnostic size".to_owned(),
                    ));
                }
                if !status.success() {
                    return Err(EngineError::InvalidConfig(format!(
                        "llama.cpp video container decode failed: {}",
                        bounded_diagnostic(&stderr)
                    )));
                }
                if let Err(error) = input_result {
                    return Err(EngineError::InvalidConfig(format!(
                        "writing the llama.cpp video container to the decoder failed: {error}"
                    )));
                }
            }
        }
        cancellation.check()?;
        parse_llama_video_frames(&output, requested)
    }

    fn read_bounded(mut reader: impl Read, limit: usize) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        reader
            .by_ref()
            .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn bounded_diagnostic(bytes: &[u8]) -> String {
        let text = String::from_utf8_lossy(bytes);
        let text = text
            .chars()
            .map(|character| {
                if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
                    '\u{fffd}'
                } else {
                    character
                }
            })
            .collect::<String>();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            "decoder exited without diagnostics".to_owned()
        } else {
            trimmed.to_owned()
        }
    }

    fn parse_llama_video_frames(bytes: &[u8], expected: u32) -> Result<Vec<(u32, u32, Vec<u8>)>> {
        let mut cursor = 0usize;
        let mut frames = Vec::new();
        while cursor < bytes.len() {
            if frames.len() >= LLAMA_VIDEO_MAX_FRAMES as usize
                || bytes.len().saturating_sub(cursor) < 12
            {
                return Err(EngineError::InvalidConfig(
                    "llama.cpp video decoder returned a malformed frame stream".to_owned(),
                ));
            }
            let width = u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().unwrap());
            let height = u32::from_be_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap());
            let length =
                u32::from_be_bytes(bytes[cursor + 8..cursor + 12].try_into().unwrap()) as usize;
            cursor += 12;
            if width == 0
                || height == 0
                || width > LLAMA_VIDEO_MAX_DIMENSION
                || height > LLAMA_VIDEO_MAX_DIMENSION
            {
                return Err(EngineError::InvalidConfig(
                    "llama.cpp video decoder returned invalid frame dimensions".to_owned(),
                ));
            }
            let expected_length = usize::try_from(width)
                .ok()
                .and_then(|width| {
                    usize::try_from(height)
                        .ok()
                        .and_then(|height| width.checked_mul(height))
                })
                .and_then(|pixels| pixels.checked_mul(3))
                .ok_or_else(|| {
                    EngineError::InvalidConfig(
                        "llama.cpp video decoder frame dimensions overflow".to_owned(),
                    )
                })?;
            if length != expected_length
                || length > LLAMA_VIDEO_MAX_DECODED_BYTES
                || cursor.saturating_add(length) > bytes.len()
            {
                return Err(EngineError::InvalidConfig(
                    "llama.cpp video decoder returned an invalid RGB frame".to_owned(),
                ));
            }
            frames.push((width, height, bytes[cursor..cursor + length].to_vec()));
            cursor += length;
        }
        if frames.len() != expected as usize {
            return Err(EngineError::InvalidConfig(format!(
                "llama.cpp video decoder returned {} frames, expected {expected}",
                frames.len()
            )));
        }
        let total = frames.iter().try_fold(0usize, |total, (_, _, pixels)| {
            total.checked_add(pixels.len())
        });
        if total.is_none_or(|total| total > LLAMA_VIDEO_MAX_DECODED_BYTES) {
            return Err(EngineError::InvalidConfig(
                "llama.cpp video decoder exceeded the bounded decoded size".to_owned(),
            ));
        }
        Ok(frames)
    }

    fn media_input_bytes(input: &MediaInput) -> Result<Vec<u8>> {
        if let Some(data) = &input.data {
            return general_purpose::STANDARD.decode(data).map_err(|err| {
                EngineError::InvalidConfig(format!("media data is not base64: {err}"))
            });
        }
        let Some(url) = &input.url else {
            return Err(EngineError::InvalidConfig(
                "media input must include a data URL or base64 data".to_owned(),
            ));
        };
        decode_data_url(url)
    }

    fn decode_data_url(url: &str) -> Result<Vec<u8>> {
        let Some(rest) = url.strip_prefix("data:") else {
            return Err(EngineError::InvalidConfig(
                "remote media URLs are not fetched by provider engines; pass image data URLs"
                    .to_owned(),
            ));
        };
        let Some((metadata, payload)) = rest.split_once(',') else {
            return Err(EngineError::InvalidConfig(
                "media data URL is missing a comma separator".to_owned(),
            ));
        };
        if !metadata
            .split(';')
            .any(|part| part.eq_ignore_ascii_case("base64"))
        {
            return Err(EngineError::InvalidConfig(
                "media data URL must be base64 encoded".to_owned(),
            ));
        }
        general_purpose::STANDARD.decode(payload).map_err(|err| {
            EngineError::InvalidConfig(format!("media data URL payload is not base64: {err}"))
        })
    }

    fn normalize_embedding(embedding: &mut [f32]) {
        let norm = embedding
            .iter()
            .map(|value| f64::from(*value) * f64::from(*value))
            .sum::<f64>()
            .sqrt();
        if norm <= f64::EPSILON || !norm.is_finite() {
            return;
        }
        for value in embedding {
            *value = (f64::from(*value) / norm) as f32;
        }
    }

    fn make_sampler(model: &LlamaModel, request: &GenerateRequest) -> Result<LlamaSampler> {
        let mut samplers = Vec::new();
        if let Some(grammar) = &request.grammar {
            samplers.push(make_grammar_sampler(model, grammar)?);
        }

        let repeat_penalty = request.repeat_penalty.unwrap_or(1.0);
        let frequency_penalty = request.frequency_penalty.unwrap_or(0.0);
        let presence_penalty = request.presence_penalty.unwrap_or(0.0);
        if (repeat_penalty - 1.0).abs() > f32::EPSILON
            || frequency_penalty.abs() > f32::EPSILON
            || presence_penalty.abs() > f32::EPSILON
        {
            samplers.push(LlamaSampler::penalties(
                -1,
                repeat_penalty,
                frequency_penalty,
                presence_penalty,
            ));
        }
        if let Some(top_k) = request.top_k {
            if top_k > 0 {
                samplers.push(LlamaSampler::top_k(top_k));
            }
        }

        if let Some(top_p) = request.top_p {
            if top_p > 0.0 && top_p < 1.0 {
                samplers.push(LlamaSampler::top_p(top_p, 1));
            }
        }
        if let Some(min_p) = request.min_p {
            if min_p > 0.0 {
                samplers.push(LlamaSampler::min_p(min_p, 1));
            }
        }
        if let Some(temperature) = request.temperature {
            if temperature > 0.0 {
                samplers.push(LlamaSampler::temp(temperature));
            }
        }

        if request.temperature.unwrap_or(0.0) <= 0.0 {
            samplers.push(LlamaSampler::greedy());
        } else {
            samplers.push(LlamaSampler::dist(request.seed.unwrap_or(DEFAULT_SEED)));
        }

        Ok(LlamaSampler::chain_simple(samplers))
    }

    struct StopSequenceStream {
        stops: Vec<String>,
        pending: VecDeque<TokenChunk>,
        pending_text: String,
        output: String,
        stopped: bool,
    }

    impl StopSequenceStream {
        fn new(stops: &[String]) -> Self {
            Self {
                stops: stops.to_vec(),
                pending: VecDeque::new(),
                pending_text: String::new(),
                output: String::new(),
                stopped: false,
            }
        }

        fn push(&mut self, chunk: TokenChunk, sink: &mut dyn TokenSink) -> Result<bool> {
            self.pending_text.push_str(&chunk.text);
            self.pending.push_back(chunk);
            if let Some(stop_at) = self
                .stops
                .iter()
                .filter_map(|stop| self.pending_text.find(stop))
                .min()
            {
                self.flush_through(stop_at, sink)?;
                self.stopped = true;
                return Ok(true);
            }
            let held_suffix = longest_stop_prefix_suffix(&self.pending_text, &self.stops);
            let safe_bytes = self.pending_text.len().saturating_sub(held_suffix);
            self.flush_complete_chunks(safe_bytes, sink)?;
            Ok(false)
        }

        fn finish(&mut self, sink: &mut dyn TokenSink) -> Result<()> {
            if self.stopped {
                return Ok(());
            }
            while let Some(chunk) = self.pending.pop_front() {
                self.output.push_str(&chunk.text);
                sink.on_token(chunk)?;
            }
            self.pending_text.clear();
            Ok(())
        }

        fn flush_complete_chunks(
            &mut self,
            mut safe_bytes: usize,
            sink: &mut dyn TokenSink,
        ) -> Result<()> {
            while self
                .pending
                .front()
                .is_some_and(|chunk| chunk.text.len() <= safe_bytes)
            {
                let chunk = self.pending.pop_front().expect("front chunk exists");
                safe_bytes = safe_bytes.saturating_sub(chunk.text.len());
                self.pending_text.drain(..chunk.text.len());
                self.output.push_str(&chunk.text);
                sink.on_token(chunk)?;
            }
            Ok(())
        }

        fn flush_through(
            &mut self,
            mut allowed_bytes: usize,
            sink: &mut dyn TokenSink,
        ) -> Result<()> {
            while let Some(mut chunk) = self.pending.pop_front() {
                let take = allowed_bytes.min(chunk.text.len());
                let take = floor_char_boundary(&chunk.text, take);
                chunk.text.truncate(take);
                allowed_bytes = allowed_bytes.saturating_sub(take);
                self.output.push_str(&chunk.text);
                sink.on_token(chunk)?;
            }
            self.pending_text.clear();
            Ok(())
        }
    }

    fn longest_stop_prefix_suffix(text: &str, stops: &[String]) -> usize {
        text.char_indices()
            .map(|(offset, _)| &text[offset..])
            .chain(std::iter::once(""))
            .filter(|suffix| {
                !suffix.is_empty() && stops.iter().any(|stop| stop.starts_with(suffix))
            })
            .map(str::len)
            .max()
            .unwrap_or(0)
    }

    fn floor_char_boundary(text: &str, mut index: usize) -> usize {
        index = index.min(text.len());
        while index > 0 && !text.is_char_boundary(index) {
            index -= 1;
        }
        index
    }

    fn make_grammar_sampler(model: &LlamaModel, spec: &GrammarSpec) -> Result<LlamaSampler> {
        match spec {
            GrammarSpec::Gbnf { grammar, root } => Ok(LlamaSampler::grammar(model, grammar, root)?),
            GrammarSpec::JsonSchema { schema } => {
                let schema = serde_json::to_string(schema)?;
                Ok(LlamaSampler::llguidance(model, "json_schema", &schema)?)
            }
            GrammarSpec::ToolCall { tools } => {
                let schema = tool_call_json_schema(tools)?;
                let schema = serde_json::to_string(&schema)?;
                Ok(LlamaSampler::llguidance(model, "json_schema", &schema)?)
            }
        }
    }

    #[allow(dead_code)]
    fn token_ids(tokens: &[LlamaToken]) -> Vec<i32> {
        tokens.iter().map(|token| token.0).collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::GenerateSpecialityParameter;

        #[test]
        fn context_params_use_standard_compact_swa_cache() {
            let config = LoadConfig::gguf("model.gguf");
            let ctx_size = NonZeroU32::new(131_072).expect("context is nonzero");

            let params = llama_context_params(&config, ctx_size).expect("context params");

            assert!(!params.swa_full());
        }

        #[test]
        fn context_params_apply_explicit_quantized_kv_cache() {
            let mut config = LoadConfig::gguf("model.gguf");
            config.kv_cache_dtype = Some("q4_0".to_owned());
            config.kv_cache_bits = Some(4);
            config.kv_cache_group_size = Some(32);
            config.kv_cache_quantized_start_tokens = Some(0);
            let ctx_size = NonZeroU32::new(262_144).expect("context is nonzero");

            let params = llama_context_params(&config, ctx_size).expect("context params");

            assert_eq!(params.type_k(), KvCacheType::Q4_0);
            assert_eq!(params.type_v(), KvCacheType::Q4_0);
            assert!(!params.swa_full());
        }

        #[test]
        fn reasoning_effort_carries_the_signed_budget_into_llama_generation() {
            let mut request = GenerateRequest::new("<|im_start|>assistant\n<think>\n");
            request.speciality_parameters = vec![GenerateSpecialityParameter {
                name: "reasoning_effort".to_owned(),
                level: "low".to_owned(),
                target: GenerateSpecialityTarget::ChatTemplateKwarg,
                native_path: "enable_thinking".to_owned(),
                value: serde_json::Value::Bool(true),
                max_reasoning_tokens: Some(512),
            }];

            apply_llama_speciality_parameters(&mut request).expect("thinking prompt matches");
            assert_eq!(llama_reasoning_budget(&request).unwrap(), Some(512));
            assert!(llama_reasoning_has_closed("private</think>\nanswer"));

            request.prompt = "<|im_start|>assistant\n<think>\n\n</think>\n\n".to_owned();
            request.speciality_parameters[0].level = "off".to_owned();
            request.speciality_parameters[0].value = serde_json::Value::Bool(false);
            request.speciality_parameters[0].max_reasoning_tokens = Some(0);
            apply_llama_speciality_parameters(&mut request).expect("disabled prompt matches");
            assert_eq!(llama_reasoning_budget(&request).unwrap(), None);

            request.messages = vec![serde_json::json!({
                "role": "assistant",
                "content": "<think>copper-signal-731</think>Noted."
            })];
            request.prompt = "<|im_start|>assistant\nNoted.<|im_end|>\n".to_owned();
            request.speciality_parameters = vec![GenerateSpecialityParameter {
                name: "thinking_history".to_owned(),
                level: "latest_only".to_owned(),
                target: GenerateSpecialityTarget::ChatTemplateKwarg,
                native_path: "preserve_thinking".to_owned(),
                value: serde_json::Value::Bool(false),
                max_reasoning_tokens: None,
            }];
            apply_llama_speciality_parameters(&mut request).expect("prior thinking is stripped");

            request.prompt =
                "<|im_start|>assistant\n<think>copper-signal-731</think>Noted.<|im_end|>\n"
                    .to_owned();
            let error = apply_llama_speciality_parameters(&mut request)
                .expect_err("latest-only history must reject leaked reasoning");
            assert!(error.to_string().contains("preserve_thinking=false"));

            request.speciality_parameters[0].level = "preserve".to_owned();
            request.speciality_parameters[0].value = serde_json::Value::Bool(true);
            apply_llama_speciality_parameters(&mut request).expect("prior thinking is preserved");
        }

        #[test]
        fn thinking_validation_accepts_canonical_qwen_and_gemma_template_positions() {
            assert!(llama_prompt_enables_thinking(
                "<|im_start|>assistant\n<think>\n"
            ));
            assert!(!llama_prompt_enables_thinking(
                "<|im_start|>assistant\n<think>\n\n</think>\n\n"
            ));
            assert!(llama_prompt_enables_thinking(
                "<|turn>system\n<|think|>\nBe concise.<turn|>\n<|turn>model\n"
            ));
            assert!(!llama_prompt_enables_thinking(
                "<|turn>user\nType <|think|> literally.<turn|>\n<|turn>model\n"
            ));
        }

        #[test]
        fn prompt_prefill_splits_at_the_context_batch_boundary() {
            let ranges = llama_prompt_batch_ranges(1_201, 512).expect("batch ranges");

            assert_eq!(ranges, vec![0..512, 512..1_024, 1_024..1_201]);
            assert_eq!(
                llama_prompt_batch_ranges(512, 512).expect("one batch"),
                vec![0..512]
            );
            assert!(llama_prompt_batch_ranges(1, 0).is_err());
            assert!(llama_prompt_batch_ranges(0, 512).is_err());
        }

        #[test]
        fn decoded_video_frame_stream_is_strict_and_bounded() {
            let mut stream = Vec::new();
            stream.extend_from_slice(&2u32.to_be_bytes());
            stream.extend_from_slice(&1u32.to_be_bytes());
            stream.extend_from_slice(&6u32.to_be_bytes());
            stream.extend_from_slice(&[255, 0, 0, 0, 255, 0]);

            let frames = parse_llama_video_frames(&stream, 1).expect("one RGB frame");
            assert_eq!(frames, vec![(2, 1, vec![255, 0, 0, 0, 255, 0])]);
            assert!(parse_llama_video_frames(&stream, 2).is_err());

            let mut malformed = stream.clone();
            malformed[11] = 5;
            assert!(parse_llama_video_frames(&malformed, 1).is_err());
            assert!(parse_llama_video_frames(&[0; 11], 1).is_err());
        }

        #[test]
        fn managed_video_decoder_stays_offline_and_bounded() {
            let decoder = include_str!("llama_video_decode.py");
            assert!(decoder.contains("import av"));
            assert!(decoder.contains("sys.stdin.buffer.read(max_input_bytes + 1)"));
            assert!(decoder.contains("requested < 1 or requested > 64"));
            assert!(decoder.contains("decoded_bytes > max_decoded_bytes"));
            assert!(!decoder.contains("requests"));
            assert!(!decoder.contains("urllib"));
            assert!(!decoder.contains("http://"));
            assert!(!decoder.contains("https://"));
        }
    }
}

#[cfg(feature = "mlx")]
mod mlx_backend {
    use super::{
        attach_worker_containment, engine_worker_command, validate_load_config, verify_artifact,
        ArtifactFormat, CancellationToken, EngineBackend, EngineError, FinishReason,
        GenerateOutput, GenerateRequest, LoadConfig, LoadedModelInfo, Result, TokenChunk,
        TokenSink, Tokenization, UsageCounters, WorkerContainment,
    };
    use serde::de::DeserializeOwned;
    use serde::Deserialize;
    use serde_json::{json, Value};
    use std::cell::{Cell, RefCell};
    use std::env;
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, ChildStdout, Stdio};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    const WORKER: &str = include_str!("mlx_worker.py");
    const PYTHON_ENV: &str = "MAYHEM_MLX_PYTHON";

    pub struct MlxBackend {
        python: PathBuf,
        worker: RefCell<Option<MlxWorker>>,
        loaded: Option<LoadedModelInfo>,
        next_id: Cell<u64>,
        memory_limit_bytes: Cell<Option<u64>>,
        cache_root: RefCell<Option<PathBuf>>,
    }

    impl MlxBackend {
        pub fn new() -> Result<Self> {
            let python = env::var_os(PYTHON_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("python3"));
            Self::with_python(python)
        }

        pub fn with_python(python: impl Into<PathBuf>) -> Result<Self> {
            Ok(Self {
                python: python.into(),
                worker: RefCell::new(None),
                loaded: None,
                next_id: Cell::new(1),
                memory_limit_bytes: Cell::new(None),
                cache_root: RefCell::new(None),
            })
        }

        fn call<T>(&self, op: &str, payload: Value) -> Result<T>
        where
            T: DeserializeOwned,
        {
            self.call_streaming(op, payload, &mut |_| Ok(()), None)
        }

        fn call_streaming<T>(
            &self,
            op: &str,
            payload: Value,
            sink: &mut dyn FnMut(TokenChunk) -> Result<()>,
            cancellation: Option<&CancellationToken>,
        ) -> Result<T>
        where
            T: DeserializeOwned,
        {
            let id = self.next_id.get();
            self.next_id.set(id.saturating_add(1));
            let mut worker = self.worker.borrow_mut();
            if worker.is_none() {
                *worker = Some(MlxWorker::spawn(
                    &self.python,
                    self.memory_limit_bytes.get(),
                    self.cache_root.borrow().as_deref(),
                )?);
            }
            let worker = worker
                .as_mut()
                .ok_or_else(|| EngineError::Mlx("failed to start MLX backend worker".to_owned()))?;
            worker.send(id, op, payload)?;
            let mut cancel_sent = false;
            let mut sink_error = None;

            loop {
                if cancellation.is_some_and(CancellationToken::is_cancelled) && !cancel_sent {
                    worker.send(id, "cancel", json!({ "request_id": id }))?;
                    cancel_sent = true;
                }
                let Some(message) = worker.read_message(Duration::from_millis(25))? else {
                    continue;
                };
                if message.id != id {
                    return Err(EngineError::Mlx(format!(
                        "worker response id {} did not match request id {id}",
                        message.id
                    )));
                }

                if message.kind == "token" {
                    let chunk = message.chunk.ok_or_else(|| {
                        EngineError::Mlx("worker token message missing chunk".to_owned())
                    })?;
                    if !cancel_sent && sink_error.is_none() {
                        if let Err(error) = sink(chunk) {
                            worker.send(id, "cancel", json!({ "request_id": id }))?;
                            cancel_sent = true;
                            sink_error = Some(error);
                        }
                    }
                    continue;
                }

                if let Some(error) = sink_error {
                    return Err(error);
                }
                if cancel_sent || message.cancelled {
                    return Err(EngineError::Cancelled);
                }

                if message.ok.unwrap_or(false) {
                    let result = message.result.unwrap_or(Value::Null);
                    return Ok(serde_json::from_value(result)?);
                }

                return Err(EngineError::Mlx(
                    message
                        .error
                        .unwrap_or_else(|| "worker returned an unknown error".to_owned()),
                ));
            }
        }
    }

    impl EngineBackend for MlxBackend {
        fn backend_id(&self) -> &'static str {
            "mlx"
        }

        fn load(&mut self, config: LoadConfig) -> Result<LoadedModelInfo> {
            validate_load_config(&config)?;
            if config.artifact.format != ArtifactFormat::MlxSafetensors {
                return Err(EngineError::InvalidConfig(format!(
                    "MLX backend requires MLX safetensors artifacts, got {:?}",
                    config.artifact.format
                )));
            }
            verify_artifact(&config.artifact)?;
            self.memory_limit_bytes.set(config.memory_limit_bytes);
            self.cache_root.replace(config.backend_cache_dir.clone());

            let model_path = mlx_model_path(&config.artifact.path)?;
            let info: WorkerLoadInfo = self.call(
                "load",
                json!({
                    "path": model_path,
                    "ctx_size": config.ctx_size,
                    "multimodal": config.mlx_runtime.multimodal,
                    "kv_cache_bits": config.kv_cache_bits,
                    "kv_cache_group_size": config.kv_cache_group_size,
                    "kv_cache_quantized_start_tokens": config.kv_cache_quantized_start_tokens,
                }),
            )?;
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

        fn component_healthy(&mut self) -> bool {
            self.loaded.is_none()
                || self
                    .worker
                    .get_mut()
                    .as_mut()
                    .is_some_and(|worker| matches!(worker.child.try_wait(), Ok(None)))
        }

        fn process_ids(&self) -> Vec<u32> {
            self.worker
                .borrow()
                .as_ref()
                .map(|worker| vec![worker.child.id()])
                .unwrap_or_default()
        }

        fn tokenize(&self, text: &str) -> Result<Tokenization> {
            self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
            self.call("tokenize", json!({ "text": text }))
        }

        fn generate(
            &mut self,
            request: GenerateRequest,
            sink: &mut dyn TokenSink,
            cancellation: &CancellationToken,
        ) -> Result<GenerateOutput> {
            cancellation.check()?;
            request.validate_sampling()?;
            if request.max_new_tokens == 0 {
                return Ok(GenerateOutput {
                    text: String::new(),
                    usage: UsageCounters::default(),
                    finish_reason: FinishReason::Length,
                });
            }
            self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;

            self.call_streaming(
                "generate",
                serde_json::to_value(request)?,
                &mut |chunk| sink.on_token(chunk),
                Some(cancellation),
            )
        }
    }

    #[derive(Debug, Deserialize)]
    struct WorkerLoadInfo {
        n_ctx_train: u32,
        n_vocab: i32,
    }

    #[derive(Debug, Deserialize)]
    struct WorkerMessage {
        id: u64,
        #[serde(default = "default_message_kind", rename = "type")]
        kind: String,
        #[serde(default)]
        ok: Option<bool>,
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        chunk: Option<TokenChunk>,
        #[serde(default)]
        cancelled: bool,
    }

    struct MlxWorker {
        child: Child,
        _containment: WorkerContainment,
        stdin: ChildStdin,
        stdout_rx: Option<Receiver<WorkerRead>>,
        reader: Option<JoinHandle<()>>,
    }

    impl MlxWorker {
        fn spawn(
            python: &Path,
            memory_limit_bytes: Option<u64>,
            cache_root: Option<&Path>,
        ) -> Result<Self> {
            let mut command = engine_worker_command(python, memory_limit_bytes);
            configure_mlx_worker_environment(&mut command, python, cache_root)?;
            command
                .arg("-u")
                .arg("-c")
                .arg(WORKER)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit());
            let mut child = command.spawn().map_err(|err| {
                EngineError::Mlx(format!(
                    "spawning MLX Python worker with {} failed: {err}",
                    python.display()
                ))
            })?;
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| EngineError::Mlx("opening worker stdin failed".to_owned()))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| EngineError::Mlx("opening worker stdout failed".to_owned()))?;
            let containment =
                attach_worker_containment(&child, memory_limit_bytes).map_err(|err| {
                    EngineError::Mlx(format!("applying worker containment failed: {err}"))
                })?;
            let (stdout_tx, stdout_rx) = mpsc::sync_channel(super::WORKER_STDOUT_QUEUE_CAPACITY);
            let reader = thread::spawn(move || read_mlx_worker_stdout(stdout, stdout_tx));
            Ok(Self {
                child,
                _containment: containment,
                stdin,
                stdout_rx: Some(stdout_rx),
                reader: Some(reader),
            })
        }

        fn send(&mut self, id: u64, op: &str, payload: Value) -> Result<()> {
            let message = json!({
                "id": id,
                "op": op,
                "payload": payload,
            });
            serde_json::to_writer(&mut self.stdin, &message)?;
            self.stdin.write_all(b"\n")?;
            self.stdin.flush()?;
            Ok(())
        }

        fn read_message(&mut self, wait: Duration) -> Result<Option<WorkerMessage>> {
            let read = match self
                .stdout_rx
                .as_ref()
                .ok_or_else(|| {
                    EngineError::Mlx("MLX backend worker stdout reader is closed".to_owned())
                })?
                .recv_timeout(wait)
            {
                Ok(read) => read,
                Err(RecvTimeoutError::Timeout) => return Ok(None),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(EngineError::Mlx(
                        "MLX backend worker stdout reader stopped".to_owned(),
                    ))
                }
            };
            let line = match read {
                WorkerRead::Line(line) => line,
                WorkerRead::Eof => {
                    return Err(EngineError::Mlx(
                        "MLX backend worker exited before replying".to_owned(),
                    ))
                }
                WorkerRead::Error(error) => return Err(EngineError::Mlx(error)),
            };
            Ok(Some(serde_json::from_str(line.trim_end())?))
        }
    }

    enum WorkerRead {
        Line(String),
        Eof,
        Error(String),
    }

    fn read_mlx_worker_stdout(stdout: ChildStdout, sender: mpsc::SyncSender<WorkerRead>) {
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
                        "reading MLX backend worker stdout failed: {error}"
                    )));
                    return;
                }
            }
        }
    }

    fn configure_mlx_worker_environment(
        command: &mut std::process::Command,
        python: &Path,
        cache_root: Option<&Path>,
    ) -> Result<()> {
        let cache_root = cache_root
            .map(Path::to_path_buf)
            .or_else(|| {
                env::var_os("MAYHEM_HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join("cache/mlx"))
            })
            .or_else(|| {
                env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".mayhem/cache/mlx"))
            })
            .unwrap_or_else(|| env::temp_dir().join("mayhem-mlx-cache"));
        for (name, default_path) in [
            ("XDG_CACHE_HOME", cache_root.join("xdg")),
            ("HF_HOME", cache_root.join("huggingface")),
            ("HF_HUB_CACHE", cache_root.join("huggingface/hub")),
            ("TRANSFORMERS_CACHE", cache_root.join("transformers")),
            ("MLX_METAL_CACHE_DIR", cache_root.join("metal")),
        ] {
            let path = env::var_os(name).map(PathBuf::from).unwrap_or(default_path);
            fs::create_dir_all(&path).map_err(|err| {
                EngineError::Mlx(format!(
                    "creating MLX cache directory {} failed: {err}",
                    path.display()
                ))
            })?;
            command.env(name, path);
        }
        if let Some(python_bin) = python.parent() {
            let mut paths = vec![python_bin.to_path_buf()];
            if let Some(current) = env::var_os("PATH") {
                paths.extend(env::split_paths(&current));
            }
            if let Ok(path) = env::join_paths(paths) {
                command.env("PATH", path);
            }
        }
        Ok(())
    }

    impl Drop for MlxWorker {
        fn drop(&mut self) {
            let _ = self.send(0, "shutdown", Value::Null);
            self.stdout_rx.take();
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }

    fn mlx_model_path(path: &Path) -> Result<PathBuf> {
        if path.is_dir() {
            return Ok(path.to_path_buf());
        }
        path.parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| EngineError::InvalidConfig("MLX weights path has no parent".to_owned()))
    }

    fn default_message_kind() -> String {
        "response".to_owned()
    }

    #[cfg(all(test, unix))]
    mod tests {
        use super::*;
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

        #[test]
        fn mlx_cancellation_keeps_worker_aligned_for_next_request() {
            let root = env::temp_dir().join(format!(
                "mayhem-mlx-cancel-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            let python = root.join("bin/python");
            let model = root.join("checkpoint/model.safetensors");
            fs::create_dir_all(python.parent().expect("python parent")).expect("python dir");
            fs::create_dir_all(model.parent().expect("model parent")).expect("model dir");
            fs::write(
                &python,
                r#"#!/bin/sh
read load_request
printf '%s\n' '{"id":1,"type":"response","ok":true,"result":{"n_ctx_train":4096,"n_vocab":32000}}'
read first_generate
read first_cancel
printf '%s\n' '{"id":2,"type":"response","cancelled":true}'
read second_generate
printf '%s\n' '{"id":3,"type":"token","chunk":{"index":0,"token_id":11,"text":"second"}}'
printf '%s\n' '{"id":3,"type":"response","ok":true,"result":{"text":"second","usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2},"finish_reason":"stop"}}'
"#,
            )
            .expect("fake worker");
            fs::write(&model, safetensors_fixture()).expect("model fixture");
            let mut permissions = fs::metadata(&python).expect("metadata").permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&python, permissions).expect("chmod fake worker");

            let mut backend = MlxBackend::with_python(&python).expect("backend");
            let mut config = LoadConfig::mlx_safetensors(&model);
            config.ctx_size = 4096;
            config.backend_cache_dir = Some(root.join("cache"));
            backend.load(config).expect("load");

            let cancellation = CancellationToken::new();
            let peer_cancellation = cancellation.clone();
            let cancel_thread = thread::spawn(move || {
                thread::sleep(Duration::from_millis(100));
                peer_cancellation.cancel();
            });
            let started = Instant::now();
            let error = backend
                .generate(
                    GenerateRequest::new("first"),
                    &mut |_| Ok(()),
                    &cancellation,
                )
                .expect_err("first request cancels");
            cancel_thread.join().expect("cancel thread");
            assert_eq!(error.to_string(), EngineError::Cancelled.to_string());
            assert!(started.elapsed() < Duration::from_secs(2));
            assert!(backend.component_healthy());

            let mut chunks = Vec::new();
            let output = backend
                .generate(
                    GenerateRequest::new("second"),
                    &mut |chunk| {
                        chunks.push(chunk);
                        Ok(())
                    },
                    &CancellationToken::new(),
                )
                .expect("next request remains aligned");
            assert_eq!(output.text, "second");
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].text, "second");

            drop(backend);
            let _ = fs::remove_dir_all(root);
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn mlx_worker_streams_each_generated_token_once_and_keeps_terminal_text() {
            let test = r#"
import ast
import sys
import types
from collections import deque

tree = ast.parse(sys.stdin.read(), "mlx_worker.py")
nodes = [
    node
    for node in tree.body
    if (
        isinstance(node, ast.ClassDef)
        and node.name in {"StopSequenceStream", "MlxGenerationEvents"}
    ) or (
        isinstance(node, ast.FunctionDef)
        and node.name in {
            "tool_call_schema",
            "structured_logits_processor",
            "reasoning_enabled",
        }
    )
]
sent = []
namespace = {"deque": deque, "send": sent.append, "tokenizer": object()}
exec(compile(ast.Module(body=nodes, type_ignores=[]), "mlx_worker.py", "exec"), namespace)

structured = types.ModuleType("mlx_vlm.structured")
structured.build_json_schema_logits_processor = lambda tokenizer, schema: ("schema", schema)
structured.ThinkingAwareLogitsProcessor = (
    lambda constrained, tokenizer, enable_thinking: ("thinking", constrained)
)
mlx_vlm = types.ModuleType("mlx_vlm")
mlx_vlm.__path__ = []
sys.modules["mlx_vlm"] = mlx_vlm
sys.modules["mlx_vlm.structured"] = structured

thinking = [{"name": "thinking_mode", "native_path": "enable_thinking", "value": True}]
tool_processor = namespace["structured_logits_processor"]({
    "grammar": {"kind": "tool_call", "tools": [{"name": "lookup"}]},
    "speciality_parameters": thinking,
})
assert tool_processor[0] == "schema"
assert tool_processor[1]["properties"]["tool"]["enum"] == ["lookup"]
json_processor = namespace["structured_logits_processor"]({
    "grammar": {"kind": "json_schema", "schema": {"type": "object"}},
    "speciality_parameters": thinking,
})
assert json_processor[0] == "thinking"

class Response:
    def __init__(self, text, token, generated, finish_reason=None):
        self.text = text
        self.token = token
        self.generation_tokens = generated
        self.finish_reason = finish_reason

events = namespace["MlxGenerationEvents"](7, [])
assert events.push(Response("", 101, 1)) == (True, False, None)
assert sent == []
assert events.push(Response("A", 102, 2)) == (True, False, None)
assert [item["chunk"]["token_id"] for item in sent] == [101]
assert events.push(Response("B", 102, 2, "length")) == (False, False, "length")
events.finish()
assert [item["chunk"]["token_id"] for item in sent] == [101, 102]
assert "".join(item["chunk"]["text"] for item in sent) == "AB"
assert events.stream.output == "AB"
assert events.completion_tokens == 2

sent.clear()
events = namespace["MlxGenerationEvents"](8, [])
events.push(Response("", 201, 1))
assert events.push(Response("tail", 202, 2, "stop")) == (True, False, "stop")
events.finish()
assert [item["chunk"]["token_id"] for item in sent] == [201, 202]
assert "".join(item["chunk"]["text"] for item in sent) == "tail"

sent.clear()
events = namespace["MlxGenerationEvents"](9, ["<STOP>"])
events.push(Response("hello<", 301, 1))
assert events.push(Response("STOP>ignored", 302, 2)) == (True, True, None)
events.finish()
assert [item["chunk"]["token_id"] for item in sent] == [301, 302]
assert events.stream.output == "hello"

events = namespace["MlxGenerationEvents"](10, [])
events.push(Response("", 401, 1))
try:
    events.push(Response("", 401, 1))
except RuntimeError as error:
    assert "before the terminal event" in str(error)
else:
    raise AssertionError("non-terminal duplicate generation counter was accepted")
print("ok")
"#;
            let mut child = std::process::Command::new("python3")
                .arg("-c")
                .arg(test)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("start Python stream-contract test");
            child
                .stdin
                .take()
                .expect("Python stdin")
                .write_all(WORKER.as_bytes())
                .expect("write embedded MLX worker");
            let output = child.wait_with_output().expect("wait for Python test");
            assert!(
                output.status.success(),
                "MLX stream-contract test failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ok");
        }

        fn safetensors_fixture() -> Vec<u8> {
            let header = br#"{"weight":{"dtype":"F32","shape":[1],"data_offsets":[0,4]}}"#;
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&(header.len() as u64).to_le_bytes());
            bytes.extend_from_slice(header);
            bytes.extend_from_slice(&[0_u8; 4]);
            bytes
        }
    }
}

#[cfg(feature = "vllm")]
mod vllm_backend {
    use super::{
        attach_worker_containment, engine_worker_command, select_runtime_compatible_cuda_home,
        validate_load_config, verify_artifact, vllm_safetensors_payload_path, ArtifactFormat,
        CancellationToken, EngineBackend, EngineError, FinishReason, GenerateOutput,
        GenerateRequest, LoadConfig, LoadedModelInfo, Result, TokenChunk, TokenSink, Tokenization,
        UsageCounters, WorkerContainment,
    };
    use serde::de::DeserializeOwned;
    use serde::Deserialize;
    use serde_json::{json, Value};
    use std::cell::{Cell, RefCell};
    use std::env;
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, ChildStdout, Stdio};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    const WORKER: &str = include_str!("vllm_worker.py");
    const PYTHON_ENV: &str = "MAYHEM_VLLM_PYTHON";
    const REQUEST_TIMEOUT_ENV: &str = "MAYHEM_VLLM_REQUEST_TIMEOUT_SECS";
    const CACHE_DIR_ENV: &str = "MAYHEM_VLLM_CACHE_DIR";
    const CUDA_HOME_ENV: &str = "MAYHEM_VLLM_CUDA_HOME";
    const BUILD_JOBS_ENV: &str = "MAYHEM_VLLM_BUILD_JOBS";
    const DEFAULT_BUILD_JOBS: usize = 2;
    const MEMORY_UTILIZATION_BACKOFF_STEP_PCT: u32 = 5;
    const DEFAULT_REQUEST_TIMEOUT: Option<Duration> = None;

    pub struct VllmBackend {
        python: PathBuf,
        worker: RefCell<Option<VllmWorker>>,
        loaded: Option<LoadedModelInfo>,
        next_id: Cell<u64>,
        memory_limit_bytes: Cell<Option<u64>>,
        cache_root: RefCell<Option<PathBuf>>,
    }

    impl VllmBackend {
        pub fn new() -> Result<Self> {
            let python = env::var_os(PYTHON_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("python3"));
            Self::with_python(python)
        }

        pub fn with_python(python: impl Into<PathBuf>) -> Result<Self> {
            Ok(Self {
                python: python.into(),
                worker: RefCell::new(None),
                loaded: None,
                next_id: Cell::new(1),
                memory_limit_bytes: Cell::new(None),
                cache_root: RefCell::new(None),
            })
        }

        fn call<T>(&self, op: &str, payload: Value) -> Result<T>
        where
            T: DeserializeOwned,
        {
            self.call_streaming(op, payload, &mut |_| Ok(()), None)
        }

        fn call_streaming<T>(
            &self,
            op: &str,
            payload: Value,
            sink: &mut dyn FnMut(TokenChunk) -> Result<()>,
            cancellation: Option<&CancellationToken>,
        ) -> Result<T>
        where
            T: DeserializeOwned,
        {
            let id = self.next_id.get();
            self.next_id.set(id.saturating_add(1));
            let mut worker = self.worker.borrow_mut();
            if worker.is_none() {
                *worker = Some(VllmWorker::spawn(
                    &self.python,
                    self.memory_limit_bytes.get(),
                    self.cache_root.borrow().as_deref(),
                )?);
            }
            let worker = worker.as_mut().ok_or_else(|| {
                EngineError::Vllm("failed to start vLLM backend worker".to_owned())
            })?;
            worker.send(id, op, payload)?;
            let mut sink_error = None;
            let mut cancel_sent = false;
            let mut last_progress = Instant::now();

            loop {
                let message = if op == "load" {
                    worker.read_load_message().map(Some)
                } else {
                    if cancellation.is_some_and(CancellationToken::is_cancelled) && !cancel_sent {
                        worker.send(id, "cancel", json!({ "request_id": id }))?;
                        cancel_sent = true;
                    }
                    if let Some(timeout) = worker.request_timeout {
                        if last_progress.elapsed() >= timeout {
                            worker.terminate();
                            return Err(sink_error.unwrap_or_else(|| {
                                EngineError::Vllm(format!(
                                    "vLLM backend worker stalled for {}s without a response",
                                    timeout.as_secs()
                                ))
                            }));
                        }
                    }
                    worker.read_message_poll(Duration::from_millis(25))
                };
                let message = match message {
                    Ok(Some(message)) => message,
                    Ok(None) => continue,
                    Err(err) => return Err(sink_error.unwrap_or(err)),
                };
                last_progress = Instant::now();
                if message.id < id {
                    continue;
                }
                if message.id > id {
                    return Err(EngineError::Vllm(format!(
                        "worker response id {} did not match request id {id}",
                        message.id
                    )));
                }

                if message.kind == "token" {
                    let chunk = message.chunk.ok_or_else(|| {
                        EngineError::Vllm("worker token message missing chunk".to_owned())
                    })?;
                    if sink_error.is_none() {
                        if let Err(err) = sink(chunk) {
                            worker.send(id, "cancel", json!({ "request_id": id }))?;
                            cancel_sent = true;
                            sink_error = Some(err);
                        }
                    }
                    continue;
                }

                if let Some(err) = sink_error {
                    return Err(err);
                }
                if cancel_sent || message.cancelled {
                    return Err(EngineError::Cancelled);
                }

                if message.ok.unwrap_or(false) {
                    let result = message.result.unwrap_or(Value::Null);
                    return Ok(serde_json::from_value(result)?);
                }

                return Err(EngineError::Vllm(
                    message
                        .error
                        .unwrap_or_else(|| "worker returned an unknown error".to_owned()),
                ));
            }
        }

        fn reset_worker(&self) {
            self.worker.borrow_mut().take();
        }
    }

    impl EngineBackend for VllmBackend {
        fn backend_id(&self) -> &'static str {
            "vllm"
        }

        fn load(&mut self, config: LoadConfig) -> Result<LoadedModelInfo> {
            validate_load_config(&config)?;
            if config.artifact.format != ArtifactFormat::VllmSafetensors {
                return Err(EngineError::InvalidConfig(format!(
                    "vLLM backend requires vLLM safetensors artifacts, got {:?}",
                    config.artifact.format
                )));
            }
            verify_artifact(&config.artifact)?;
            self.memory_limit_bytes.set(config.memory_limit_bytes);
            self.cache_root.replace(config.backend_cache_dir.clone());

            let model_path = vllm_model_path(&config.artifact.path)?;
            let attempts = vllm_memory_utilization_attempts(
                config.vllm_gpu_memory_utilization_pct,
                config.vllm_gpu_memory_utilization_floor_pct,
            )?;
            let mut info = None;
            for (index, utilization_pct) in attempts.iter().enumerate() {
                let mut attempt_config = config.clone();
                attempt_config.vllm_gpu_memory_utilization_pct = *utilization_pct;
                match self
                    .call::<WorkerLoadInfo>("load", vllm_load_payload(&attempt_config, &model_path))
                {
                    Ok(loaded) => {
                        info = Some(loaded);
                        break;
                    }
                    Err(err) if is_vllm_oom_error(&err) && index + 1 < attempts.len() => {
                        self.reset_worker();
                    }
                    Err(err) => return Err(err),
                }
            }
            let info = info.ok_or_else(|| {
                EngineError::Vllm("vLLM load exhausted memory-utilization attempts".to_owned())
            })?;
            let loaded = LoadedModelInfo {
                backend: self.backend_id().to_owned(),
                artifact: config.artifact,
                ctx_size: config.ctx_size,
                n_ctx_train: if info.n_ctx_train == 0 {
                    config.ctx_size
                } else {
                    info.n_ctx_train
                },
                n_vocab: info.n_vocab,
            };
            self.loaded = Some(loaded.clone());
            Ok(loaded)
        }

        fn component_healthy(&mut self) -> bool {
            self.loaded.is_none()
                || self
                    .worker
                    .get_mut()
                    .as_mut()
                    .is_some_and(|worker| matches!(worker.child.try_wait(), Ok(None)))
        }

        fn process_ids(&self) -> Vec<u32> {
            self.worker
                .borrow()
                .as_ref()
                .map(|worker| vec![worker.child.id()])
                .unwrap_or_default()
        }

        fn tokenize(&self, text: &str) -> Result<Tokenization> {
            self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
            self.call("tokenize", json!({ "text": text }))
        }

        fn generate(
            &mut self,
            request: GenerateRequest,
            sink: &mut dyn TokenSink,
            cancellation: &CancellationToken,
        ) -> Result<GenerateOutput> {
            cancellation.check()?;
            request.validate_sampling()?;
            if request.max_new_tokens == 0 {
                return Ok(GenerateOutput {
                    text: String::new(),
                    usage: UsageCounters::default(),
                    finish_reason: FinishReason::Length,
                });
            }
            self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;

            self.call_streaming(
                "generate",
                serde_json::to_value(request)?,
                &mut |chunk| sink.on_token(chunk),
                Some(cancellation),
            )
        }
    }

    #[derive(Debug, Deserialize)]
    struct WorkerLoadInfo {
        #[serde(default)]
        n_ctx_train: u32,
        #[serde(default)]
        n_vocab: i32,
    }

    #[derive(Debug, Deserialize)]
    struct WorkerMessage {
        id: u64,
        #[serde(default = "default_message_kind", rename = "type")]
        kind: String,
        #[serde(default)]
        ok: Option<bool>,
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        chunk: Option<TokenChunk>,
        #[serde(default)]
        cancelled: bool,
    }

    struct VllmWorker {
        child: Child,
        _containment: WorkerContainment,
        stdin: ChildStdin,
        stdout_rx: Option<Receiver<WorkerRead>>,
        reader: Option<JoinHandle<()>>,
        request_timeout: Option<Duration>,
        terminated: bool,
    }

    impl VllmWorker {
        fn spawn(
            python: &Path,
            memory_limit_bytes: Option<u64>,
            cache_root: Option<&Path>,
        ) -> Result<Self> {
            Self::spawn_with_timeout(python, request_timeout()?, memory_limit_bytes, cache_root)
        }

        fn spawn_with_timeout(
            python: &Path,
            request_timeout: Option<Duration>,
            memory_limit_bytes: Option<u64>,
            cache_root: Option<&Path>,
        ) -> Result<Self> {
            let mut command = engine_worker_command(python, memory_limit_bytes);
            configure_vllm_worker_environment(&mut command, python, cache_root)?;
            command
                .arg("-u")
                .arg("-c")
                .arg(WORKER)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit());
            let mut child = command.spawn().map_err(|err| {
                EngineError::Vllm(format!(
                    "spawning vLLM Python worker with {} failed: {err}",
                    python.display()
                ))
            })?;
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| EngineError::Vllm("opening worker stdin failed".to_owned()))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| EngineError::Vllm("opening worker stdout failed".to_owned()))?;
            let containment =
                attach_worker_containment(&child, memory_limit_bytes).map_err(|err| {
                    EngineError::Vllm(format!("applying worker containment failed: {err}"))
                })?;
            let (stdout_tx, stdout_rx) = mpsc::sync_channel(super::WORKER_STDOUT_QUEUE_CAPACITY);
            let reader = thread::spawn(move || read_worker_stdout(stdout, stdout_tx));
            Ok(Self {
                child,
                _containment: containment,
                stdin,
                stdout_rx: Some(stdout_rx),
                reader: Some(reader),
                request_timeout,
                terminated: false,
            })
        }

        fn send(&mut self, id: u64, op: &str, payload: Value) -> Result<()> {
            let message = json!({
                "id": id,
                "op": op,
                "payload": payload,
            });
            serde_json::to_writer(&mut self.stdin, &message)?;
            self.stdin.write_all(b"\n")?;
            self.stdin.flush()?;
            Ok(())
        }

        #[cfg(test)]
        fn read_message(&mut self) -> Result<WorkerMessage> {
            let read = match self.request_timeout {
                Some(request_timeout) => match self
                    .stdout_rx
                    .as_ref()
                    .ok_or_else(|| {
                        EngineError::Vllm("vLLM backend worker stdout reader is closed".to_owned())
                    })?
                    .recv_timeout(request_timeout)
                {
                    Ok(read) => read,
                    Err(RecvTimeoutError::Timeout) => {
                        self.terminate();
                        return Err(EngineError::Vllm(format!(
                            "vLLM backend worker stalled for {}s without a response",
                            request_timeout.as_secs()
                        )));
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        return Err(EngineError::Vllm(
                            "vLLM backend worker stdout reader stopped".to_owned(),
                        ));
                    }
                },
                None => self
                    .stdout_rx
                    .as_ref()
                    .ok_or_else(|| {
                        EngineError::Vllm("vLLM backend worker stdout reader is closed".to_owned())
                    })?
                    .recv()
                    .map_err(|_| {
                        EngineError::Vllm("vLLM backend worker stdout reader stopped".to_owned())
                    })?,
            };
            Self::decode_read(read)
        }

        fn read_message_poll(&mut self, wait: Duration) -> Result<Option<WorkerMessage>> {
            match self
                .stdout_rx
                .as_ref()
                .ok_or_else(|| {
                    EngineError::Vllm("vLLM backend worker stdout reader is closed".to_owned())
                })?
                .recv_timeout(wait)
            {
                Ok(read) => Self::decode_read(read).map(Some),
                Err(RecvTimeoutError::Timeout) => Ok(None),
                Err(RecvTimeoutError::Disconnected) => Err(EngineError::Vllm(
                    "vLLM backend worker stdout reader stopped".to_owned(),
                )),
            }
        }

        fn read_load_message(&mut self) -> Result<WorkerMessage> {
            let read = self
                .stdout_rx
                .as_ref()
                .ok_or_else(|| {
                    EngineError::Vllm("vLLM backend worker stdout reader is closed".to_owned())
                })?
                .recv()
                .map_err(|_| {
                    EngineError::Vllm("vLLM backend worker stdout reader stopped".to_owned())
                })?;
            Self::decode_read(read)
        }

        fn decode_read(read: WorkerRead) -> Result<WorkerMessage> {
            let line = match read {
                WorkerRead::Line(line) => line,
                WorkerRead::Eof => {
                    return Err(EngineError::Vllm(
                        "vLLM backend worker exited before replying".to_owned(),
                    ));
                }
                WorkerRead::Error(error) => return Err(EngineError::Vllm(error)),
            };
            Ok(serde_json::from_str(line.trim_end())?)
        }

        fn terminate(&mut self) {
            if self.terminated {
                return;
            }
            terminate_worker_process(&mut self.child);
            self.terminated = true;
        }
    }

    enum WorkerRead {
        Line(String),
        Eof,
        Error(String),
    }

    fn read_worker_stdout(stdout: ChildStdout, sender: mpsc::SyncSender<WorkerRead>) {
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
                Err(err) => {
                    let _ = sender.send(WorkerRead::Error(format!(
                        "reading vLLM backend worker stdout failed: {err}"
                    )));
                    return;
                }
            }
        }
    }

    fn request_timeout() -> Result<Option<Duration>> {
        match env::var(REQUEST_TIMEOUT_ENV) {
            Ok(value) => request_timeout_from(Some(&value)),
            Err(env::VarError::NotPresent) => Ok(DEFAULT_REQUEST_TIMEOUT),
            Err(err) => Err(EngineError::InvalidConfig(format!(
                "reading {REQUEST_TIMEOUT_ENV} failed: {err}"
            ))),
        }
    }

    fn request_timeout_from(value: Option<&str>) -> Result<Option<Duration>> {
        let Some(value) = value else {
            return Ok(DEFAULT_REQUEST_TIMEOUT);
        };
        let seconds = value.trim().parse::<u64>().map_err(|_| {
            EngineError::InvalidConfig(format!(
                "{REQUEST_TIMEOUT_ENV} must be a non-negative integer in seconds"
            ))
        })?;
        Ok((seconds > 0).then(|| Duration::from_secs(seconds)))
    }

    fn configure_vllm_worker_environment(
        command: &mut std::process::Command,
        python: &Path,
        configured_cache_root: Option<&Path>,
    ) -> Result<()> {
        let cache_root = vllm_cache_root(configured_cache_root);
        let cache_dirs = [
            ("XDG_CACHE_HOME", cache_root.join("xdg")),
            ("TRITON_CACHE_DIR", cache_root.join("triton")),
            ("VLLM_CACHE_ROOT", cache_root.join("vllm")),
            ("TORCHINDUCTOR_CACHE_DIR", cache_root.join("torchinductor")),
            ("CUDA_CACHE_PATH", cache_root.join("cuda")),
            ("FLASHINFER_CACHE_DIR", cache_root.join("flashinfer")),
            (
                "FLASHINFER_WORKSPACE_BASE",
                cache_root.join("flashinfer/workspace-base"),
            ),
            ("FLASHINFER_JIT_DIR", cache_root.join("flashinfer/jit")),
            (
                "FLASHINFER_WORKSPACE_DIR",
                cache_root.join("flashinfer/workspace"),
            ),
        ];
        for (name, default_path) in cache_dirs {
            let path = env::var_os(name).map(PathBuf::from).unwrap_or(default_path);
            fs::create_dir_all(&path).map_err(|err| {
                EngineError::Vllm(format!(
                    "creating vLLM cache directory {} failed: {err}",
                    path.display()
                ))
            })?;
            command.env(name, path);
        }
        command.env("VLLM_ENABLE_V1_MULTIPROCESSING", "0");
        command.env("PYTHONHASHSEED", "0");
        command.env("CUBLAS_WORKSPACE_CONFIG", ":4096:8");
        command.env("MAX_JOBS", vllm_build_jobs().to_string());

        let cuda_home = resolve_vllm_cuda_home(python);
        let mut path_prefixes = Vec::new();
        if let Some(cuda_home) = &cuda_home {
            let nvcc = cuda_home.join("bin/nvcc");
            path_prefixes.push(cuda_home.join("bin"));
            command.env("CUDA_HOME", cuda_home);
            command.env("CUDA_PATH", cuda_home);
            if env::var_os("FLASHINFER_NVCC").is_none() {
                command.env("FLASHINFER_NVCC", &nvcc);
            }
            let cuda_lib = if cuda_home.join("lib").is_dir() {
                cuda_home.join("lib")
            } else {
                cuda_home.join("lib64")
            };
            command.env(
                "LD_LIBRARY_PATH",
                prepend_env_path(&cuda_lib, env::var_os("LD_LIBRARY_PATH")),
            );
        }
        if let Some(python_bin) = python.parent() {
            path_prefixes.push(python_bin.to_path_buf());
        }
        command.env(
            "PATH",
            prepend_env_paths(&path_prefixes, env::var_os("PATH")),
        );
        Ok(())
    }

    fn vllm_cache_root(configured: Option<&Path>) -> PathBuf {
        env::var_os(CACHE_DIR_ENV)
            .map(PathBuf::from)
            .or_else(|| configured.map(Path::to_path_buf))
            .or_else(|| {
                env::var_os("MAYHEM_HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join("cache/vllm"))
            })
            .or_else(|| {
                env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".mayhem/cache/vllm"))
            })
            .unwrap_or_else(|| env::temp_dir().join("mayhem-vllm-cache"))
    }

    fn vllm_build_jobs() -> usize {
        [BUILD_JOBS_ENV, "MAX_JOBS"]
            .into_iter()
            .find_map(|name| {
                env::var(name)
                    .ok()
                    .and_then(|value| parse_build_jobs(&value))
            })
            .unwrap_or(DEFAULT_BUILD_JOBS)
    }

    fn parse_build_jobs(value: &str) -> Option<usize> {
        value.parse::<usize>().ok().filter(|jobs| *jobs > 0)
    }

    pub(super) fn resolve_vllm_cuda_home(python: &Path) -> Option<PathBuf> {
        if let Some(explicit) = env::var_os(CUDA_HOME_ENV).map(PathBuf::from) {
            return cuda_home_with_nvcc(explicit);
        }
        if let Some(explicit) = env::var_os("CUDA_HOME").map(PathBuf::from) {
            return cuda_home_with_nvcc(explicit);
        }

        let mut candidates = cuda_home_with_nvcc(PathBuf::from("/usr/local/cuda"))
            .into_iter()
            .collect::<Vec<_>>();
        candidates.extend(usr_local_cuda_homes());
        candidates.extend(bundled_cuda_homes(python));
        select_runtime_compatible_cuda_home(python, candidates)
    }

    fn bundled_cuda_homes(python: &Path) -> Vec<PathBuf> {
        let Some(venv) = python.parent().and_then(Path::parent) else {
            return Vec::new();
        };
        let lib = venv.join("lib");
        let mut candidates = fs::read_dir(lib)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path().join("site-packages/nvidia"))
            .filter_map(|nvidia| fs::read_dir(nvidia).ok())
            .flatten()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter_map(cuda_home_with_nvcc)
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.reverse();
        candidates
    }

    fn usr_local_cuda_homes() -> Vec<PathBuf> {
        let mut candidates = fs::read_dir("/usr/local")
            .ok()
            .into_iter()
            .flatten()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("cuda-"))
            })
            .filter_map(cuda_home_with_nvcc)
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.reverse();
        candidates
    }

    fn cuda_home_with_nvcc(path: PathBuf) -> Option<PathBuf> {
        path.join("bin/nvcc").is_file().then_some(path)
    }

    fn prepend_env_path(path: &Path, current: Option<std::ffi::OsString>) -> std::ffi::OsString {
        prepend_env_paths(&[path.to_path_buf()], current)
    }

    fn prepend_env_paths(
        prefixes: &[PathBuf],
        current: Option<std::ffi::OsString>,
    ) -> std::ffi::OsString {
        let mut paths = prefixes.to_vec();
        if let Some(current) = current {
            paths.extend(env::split_paths(&current));
        }
        env::join_paths(paths).unwrap_or_else(|_| {
            prefixes
                .first()
                .map(|path| path.as_os_str().to_owned())
                .unwrap_or_default()
        })
    }

    fn terminate_worker_process(child: &mut Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            match child.try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) => thread::sleep(Duration::from_millis(50)),
                Err(_) => return true,
            }
        }
        false
    }

    impl Drop for VllmWorker {
        fn drop(&mut self) {
            let _ = self.send(0, "shutdown", Value::Null);
            self.stdout_rx.take();
            if wait_for_child_exit(&mut self.child, Duration::from_secs(3)) {
                self.terminated = true;
            } else {
                self.terminate();
            }
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }

    fn vllm_model_path(path: &Path) -> Result<PathBuf> {
        if path.is_dir() {
            return Ok(path.to_path_buf());
        }
        let payload = vllm_safetensors_payload_path(path)?;
        payload
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| EngineError::InvalidConfig("vLLM weights path has no parent".to_owned()))
    }

    fn vllm_load_payload(config: &LoadConfig, model_path: &Path) -> Value {
        let mut payload = json!({
            "path": model_path,
            "ctx_size": config.ctx_size,
            "max_batch_size": config.batch_size.max(1),
            "max_num_tokens": config.ubatch_size.max(1),
            "tensor_parallel": config.vllm_tensor_parallel.unwrap_or(1),
            "dtype": config.vllm_dtype,
        });
        if let Some(pct) = config.vllm_gpu_memory_utilization_pct {
            payload["gpu_memory_utilization"] = json!((pct as f64) / 100.0);
        }
        payload
    }

    fn vllm_memory_utilization_attempts(
        target_pct: Option<u32>,
        floor_pct: Option<u32>,
    ) -> Result<Vec<Option<u32>>> {
        let Some(target_pct) = target_pct else {
            if floor_pct.is_some() {
                return Err(EngineError::InvalidConfig(
                    "vLLM memory-utilization floor requires a target".to_owned(),
                ));
            }
            return Ok(vec![None]);
        };
        if target_pct == 0 || target_pct > 100 {
            return Err(EngineError::InvalidConfig(
                "vLLM memory-utilization target must be between 1 and 100".to_owned(),
            ));
        }
        let floor_pct = floor_pct.unwrap_or(target_pct);
        if floor_pct == 0 || floor_pct > target_pct {
            return Err(EngineError::InvalidConfig(
                "vLLM memory-utilization floor must be between 1 and the target".to_owned(),
            ));
        }

        let mut attempts = vec![Some(target_pct)];
        let mut current = target_pct;
        while current > floor_pct {
            current = current
                .saturating_sub(MEMORY_UTILIZATION_BACKOFF_STEP_PCT)
                .max(floor_pct);
            attempts.push(Some(current));
        }
        Ok(attempts)
    }

    fn is_vllm_oom_error(error: &EngineError) -> bool {
        let EngineError::Vllm(message) = error else {
            return false;
        };
        let message = message.to_ascii_lowercase();
        [
            "out of memory",
            "cuda oom",
            "cannot allocate memory",
            "not enough memory",
        ]
        .iter()
        .any(|needle| message.contains(needle))
    }

    fn default_message_kind() -> String {
        "response".to_owned()
    }

    #[cfg(test)]
    #[cfg(unix)]
    mod tests {
        use super::*;
        use std::collections::BTreeMap;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        #[test]
        fn vllm_worker_read_timeout_kills_silent_child() {
            let path =
                env::temp_dir().join(format!("mayhem-silent-vllm-worker-{}", std::process::id()));
            fs::write(&path, "#!/bin/sh\nexec sleep 20\n").expect("write fake worker");
            let mut perms = fs::metadata(&path).expect("metadata").permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&path, perms).expect("chmod fake worker");

            let mut worker =
                VllmWorker::spawn_with_timeout(&path, Some(Duration::from_secs(1)), None, None)
                    .expect("spawn");
            worker
                .send(1, "load", Value::Null)
                .expect("send request to fake worker");
            let start = Instant::now();
            let err = worker.read_message().expect_err("silent worker times out");
            assert!(start.elapsed() < Duration::from_secs(5));
            assert!(
                format!("{err}").contains("stalled for 1s without a response"),
                "{err}"
            );

            let _ = fs::remove_file(path);
        }

        #[test]
        fn vllm_load_wait_ignores_inference_timeout() {
            let path = env::temp_dir().join(format!(
                "mayhem-slow-loading-vllm-worker-{}",
                std::process::id()
            ));
            fs::write(
                &path,
                "#!/bin/sh\nread request\nsleep 1\nprintf '%s\\n' '{\"id\":1,\"type\":\"response\",\"ok\":true,\"result\":{}}'\n",
            )
            .expect("write fake worker");
            let mut perms = fs::metadata(&path).expect("metadata").permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&path, perms).expect("chmod fake worker");

            let mut worker =
                VllmWorker::spawn_with_timeout(&path, Some(Duration::from_millis(50)), None, None)
                    .expect("spawn");
            worker
                .send(1, "load", Value::Null)
                .expect("send load request to fake worker");
            let start = Instant::now();
            let message = worker
                .read_load_message()
                .expect("load waits beyond inference timeout");
            assert!(start.elapsed() >= Duration::from_millis(500));
            assert_eq!(message.id, 1);
            assert_eq!(message.ok, Some(true));

            let _ = fs::remove_file(path);
        }

        #[test]
        fn vllm_worker_has_no_implicit_response_deadline() {
            let path =
                env::temp_dir().join(format!("mayhem-delayed-vllm-worker-{}", std::process::id()));
            fs::write(
                &path,
                "#!/bin/sh\nread request\nsleep 1\nprintf '%s\\n' '{\"id\":1,\"type\":\"response\",\"ok\":true,\"result\":{}}'\n",
            )
            .expect("write fake worker");
            let mut perms = fs::metadata(&path).expect("metadata").permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&path, perms).expect("chmod fake worker");

            let mut worker =
                VllmWorker::spawn_with_timeout(&path, None, None, None).expect("spawn");
            worker
                .send(1, "generate", Value::Null)
                .expect("send generation request");
            let message = worker
                .read_message()
                .expect("default response wait has no time ceiling");
            assert_eq!(message.id, 1);
            assert_eq!(message.ok, Some(true));

            let _ = fs::remove_file(path);
        }

        #[test]
        fn vllm_worker_timeout_override_is_validated() {
            assert_eq!(request_timeout_from(None).unwrap(), None);
            assert_eq!(request_timeout_from(Some("0")).unwrap(), None);
            assert_eq!(
                request_timeout_from(Some("600")).unwrap(),
                Some(Duration::from_secs(600))
            );
            assert!(request_timeout_from(Some("later")).is_err());
        }

        #[test]
        fn vllm_worker_multimodal_path_is_local_only_and_processor_backed() {
            assert!(WORKER.contains("AutoProcessor.from_pretrained"));
            assert!(WORKER.contains("renderer.apply_chat_template"));
            assert!(WORKER.contains("multi_modal_data"));
            assert!(WORKER.contains("limit_mm_per_prompt"));
            assert!(WORKER.contains("remote media URLs are forbidden"));
            assert!(WORKER.contains("base64.b64decode"));
            assert!(WORKER.contains("num_frames must be between 1 and 64"));
            assert!(WORKER.contains("\"frames_indices\": frame_indices"));
            assert!(WORKER.contains("\"video_backend\": \"pyav\""));
            assert!(WORKER.contains("\"audio_tokens\""));
        }

        #[test]
        fn vllm_worker_enforces_seeded_batch_invariant_execution_when_supported() {
            assert!(WORKER.contains("VLLM_BATCH_INVARIANT"));
            assert!(WORKER.contains("capability >= (9, 0)"));
            assert!(WORKER.contains("model_uses_hybrid_attention"));
            assert!(WORKER.contains("linear\", \"mamba\", \"ssm\", \"gdn"));
            assert!(WORKER.contains("\"async_scheduling\": False"));
            assert!(WORKER.contains("\"use_fp64_gumbel\": True"));
            assert!(WORKER.contains("required deterministic engine option(s)"));
            assert!(WORKER.contains("model_uses_nvfp4"));
            assert!(WORKER.contains("kwargs[\"linear_backend\"] = \"cutlass\""));
            assert!(WORKER.contains("kwargs[\"moe_backend\"] = \"cutlass\""));
            assert!(WORKER.contains("\"kernel_policy\": kernel_policy"));
        }

        #[test]
        fn vllm_worker_environment_disables_nondeterministic_v1_scheduling() {
            let cache = env::temp_dir().join(format!(
                "mayhem-vllm-determinism-env-{}",
                std::process::id()
            ));
            let mut command = std::process::Command::new("python3");
            configure_vllm_worker_environment(&mut command, Path::new("python3"), Some(&cache))
                .unwrap();
            let environment = command
                .get_envs()
                .filter_map(|(name, value)| Some((name.to_str()?, value?.to_str()?)))
                .collect::<BTreeMap<_, _>>();

            assert_eq!(environment["VLLM_ENABLE_V1_MULTIPROCESSING"], "0");
            assert_eq!(environment["PYTHONHASHSEED"], "0");
            assert_eq!(environment["CUBLAS_WORKSPACE_CONFIG"], ":4096:8");
            let _ = fs::remove_dir_all(cache);
        }

        #[test]
        fn vllm_stream_sink_failure_drains_response_and_keeps_request_ids_aligned() {
            let root =
                env::temp_dir().join(format!("mayhem-vllm-stream-drain-{}", std::process::id()));
            let python = root.join("bin/python");
            let model = root.join("checkpoint/model.safetensors");
            fs::create_dir_all(python.parent().expect("python parent")).unwrap();
            fs::create_dir_all(model.parent().expect("model parent")).unwrap();
            fs::write(
                &python,
                r#"#!/bin/sh
read load_request
printf '%s\n' '{"id":1,"type":"response","ok":true,"result":{"n_ctx_train":4096,"n_vocab":32000}}'
read first_generate
printf '%s\n' '{"id":2,"type":"token","chunk":{"index":0,"token_id":10,"text":"first"}}'
printf '%s\n' '{"id":2,"type":"response","ok":true,"result":{"text":"first","usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2},"finish_reason":"stop"}}'
read second_generate
printf '%s\n' '{"id":3,"type":"token","chunk":{"index":0,"token_id":11,"text":"second"}}'
printf '%s\n' '{"id":3,"type":"response","ok":true,"result":{"text":"second","usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2},"finish_reason":"stop"}}'
"#,
            )
            .unwrap();
            fs::write(&model, safetensors_fixture()).unwrap();
            let mut perms = fs::metadata(&python).unwrap().permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&python, perms).unwrap();

            let mut backend = VllmBackend::with_python(&python).unwrap();
            let mut config = LoadConfig::vllm_safetensors(&model);
            config.ctx_size = 4096;
            config.backend_cache_dir = Some(root.join("cache"));
            backend.load(config).unwrap();

            let mut disconnected = |_chunk: TokenChunk| {
                Err(EngineError::InvalidConfig("client disconnected".to_owned()))
            };
            let err = backend
                .generate(
                    GenerateRequest::new("first"),
                    &mut disconnected,
                    &CancellationToken::new(),
                )
                .expect_err("first stream sink disconnects");
            assert!(err.to_string().contains("client disconnected"));

            let mut chunks = Vec::new();
            let output = backend
                .generate(
                    GenerateRequest::new("second"),
                    &mut |chunk| {
                        chunks.push(chunk);
                        Ok(())
                    },
                    &CancellationToken::new(),
                )
                .expect("next request remains aligned");
            assert_eq!(output.text, "second");
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].text, "second");

            drop(backend);
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn vllm_cancellation_aborts_request_and_keeps_worker_aligned() {
            let root = env::temp_dir().join(format!(
                "mayhem-vllm-cancel-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            let python = root.join("bin/python");
            let model = root.join("checkpoint/model.safetensors");
            fs::create_dir_all(python.parent().expect("python parent")).unwrap();
            fs::create_dir_all(model.parent().expect("model parent")).unwrap();
            fs::write(
                &python,
                r#"#!/bin/sh
read load_request
printf '%s\n' '{"id":1,"type":"response","ok":true,"result":{"n_ctx_train":4096,"n_vocab":32000}}'
read first_generate
read first_cancel
printf '%s\n' '{"id":2,"type":"response","cancelled":true}'
read second_generate
printf '%s\n' '{"id":3,"type":"token","chunk":{"index":0,"token_id":11,"text":"second"}}'
printf '%s\n' '{"id":3,"type":"response","ok":true,"result":{"text":"second","usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2},"finish_reason":"stop"}}'
"#,
            )
            .unwrap();
            fs::write(&model, safetensors_fixture()).unwrap();
            let mut permissions = fs::metadata(&python).unwrap().permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&python, permissions).unwrap();

            let mut backend = VllmBackend::with_python(&python).unwrap();
            let mut config = LoadConfig::vllm_safetensors(&model);
            config.ctx_size = 4096;
            config.backend_cache_dir = Some(root.join("cache"));
            backend.load(config).unwrap();

            let cancellation = CancellationToken::new();
            let peer_cancellation = cancellation.clone();
            let cancel_thread = thread::spawn(move || {
                thread::sleep(Duration::from_millis(100));
                peer_cancellation.cancel();
            });
            let started = Instant::now();
            let error = backend
                .generate(
                    GenerateRequest::new("first"),
                    &mut |_| Ok(()),
                    &cancellation,
                )
                .expect_err("first request cancels");
            cancel_thread.join().expect("cancel thread");
            assert_eq!(error.to_string(), EngineError::Cancelled.to_string());
            assert!(started.elapsed() < Duration::from_secs(2));
            assert!(backend.component_healthy());

            let mut chunks = Vec::new();
            let output = backend
                .generate(
                    GenerateRequest::new("second"),
                    &mut |chunk| {
                        chunks.push(chunk);
                        Ok(())
                    },
                    &CancellationToken::new(),
                )
                .expect("next request remains aligned");
            assert_eq!(output.text, "second");
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].text, "second");

            drop(backend);
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn vllm_load_payload_carries_capacity_knobs() {
            let mut config = LoadConfig::vllm_safetensors("/tmp/checkpoint");
            config.ctx_size = 1024;
            config.batch_size = 4;
            config.ubatch_size = 512;
            config.vllm_tensor_parallel = Some(2);
            config.vllm_dtype = Some("float16".to_owned());
            config.vllm_gpu_memory_utilization_pct = Some(45);

            let payload = vllm_load_payload(&config, Path::new("/tmp/checkpoint"));
            assert_eq!(payload["ctx_size"], json!(1024));
            assert_eq!(payload["max_batch_size"], json!(4));
            assert_eq!(payload["max_num_tokens"], json!(512));
            assert_eq!(payload["tensor_parallel"], json!(2));
            assert_eq!(payload["dtype"], json!("float16"));
            assert_eq!(payload["gpu_memory_utilization"], json!(0.45));

            config.ctx_size = 131_072;
            let payload = vllm_load_payload(&config, Path::new("/tmp/checkpoint"));
            assert_eq!(payload["ctx_size"], json!(131_072));
            assert_eq!(payload["max_num_tokens"], json!(512));
        }

        #[test]
        fn vllm_memory_utilization_backoff_stops_at_the_fit_floor() {
            assert_eq!(
                vllm_memory_utilization_attempts(Some(42), Some(31)).unwrap(),
                vec![Some(42), Some(37), Some(32), Some(31)]
            );
            assert_eq!(
                vllm_memory_utilization_attempts(Some(42), Some(42)).unwrap(),
                vec![Some(42)]
            );
            assert!(vllm_memory_utilization_attempts(Some(30), Some(31)).is_err());
        }

        #[test]
        fn vllm_load_retries_oom_without_changing_context() {
            let root =
                env::temp_dir().join(format!("mayhem-vllm-oom-retry-{}", std::process::id()));
            let python = root.join("bin/python");
            let state = root.join("first-attempt-seen");
            let requests = root.join("requests.jsonl");
            let model = root.join("checkpoint/model.safetensors");
            fs::create_dir_all(python.parent().expect("python parent")).unwrap();
            fs::create_dir_all(model.parent().expect("model parent")).unwrap();
            let script = r#"#!/bin/sh
read request
printf '%s\n' "$request" >> "__REQUESTS__"
if [ ! -f "__STATE__" ]; then
  : > "__STATE__"
  printf '%s\n' '{"id":1,"type":"response","ok":false,"error":"CUDA out of memory"}'
else
  printf '%s\n' '{"id":2,"type":"response","ok":true,"result":{"n_ctx_train":4096,"n_vocab":32000}}'
fi
"#
            .replace("__STATE__", &state.display().to_string())
            .replace("__REQUESTS__", &requests.display().to_string());
            fs::write(&python, script).unwrap();
            fs::write(&model, safetensors_fixture()).unwrap();
            let mut perms = fs::metadata(&python).unwrap().permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&python, perms).unwrap();

            let mut backend = VllmBackend::with_python(&python).unwrap();
            let mut config = LoadConfig::vllm_safetensors(&model);
            config.ctx_size = 4096;
            config.vllm_gpu_memory_utilization_pct = Some(42);
            config.vllm_gpu_memory_utilization_floor_pct = Some(32);
            config.backend_cache_dir = Some(root.join("cache"));
            let loaded = backend.load(config).expect("second load attempt succeeds");
            assert_eq!(loaded.ctx_size, 4096);

            let requests = fs::read_to_string(&requests).unwrap();
            let payloads = requests
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(payloads.len(), 2);
            assert_eq!(payloads[0]["payload"]["ctx_size"], json!(4096));
            assert_eq!(payloads[1]["payload"]["ctx_size"], json!(4096));
            assert_eq!(
                payloads[0]["payload"]["gpu_memory_utilization"],
                json!(0.42)
            );
            assert_eq!(
                payloads[1]["payload"]["gpu_memory_utilization"],
                json!(0.37)
            );

            drop(backend);
            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn bundled_cuda_home_follows_the_vllm_python_environment() {
            let root =
                env::temp_dir().join(format!("mayhem-vllm-cuda-home-{}", std::process::id()));
            let python = root.join("bin/python");
            let cuda = root.join("lib/python3.12/site-packages/nvidia/cu13");
            fs::create_dir_all(python.parent().unwrap()).unwrap();
            fs::create_dir_all(cuda.join("bin")).unwrap();
            fs::write(&python, b"").unwrap();
            fs::write(cuda.join("bin/nvcc"), b"").unwrap();

            assert_eq!(bundled_cuda_homes(&python), vec![cuda]);

            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn vllm_worker_path_contains_cuda_and_python_tools() {
            let cuda_bin = PathBuf::from("/runtime/cuda/bin");
            let python_bin = PathBuf::from("/runtime/vllm/bin");
            let current = env::join_paths([PathBuf::from("/usr/bin"), PathBuf::from("/bin")])
                .expect("test PATH");
            let joined = prepend_env_paths(&[cuda_bin.clone(), python_bin.clone()], Some(current));
            let paths = env::split_paths(&joined).collect::<Vec<_>>();
            assert_eq!(paths[0], cuda_bin);
            assert_eq!(paths[1], python_bin);
            assert_eq!(paths[2], PathBuf::from("/usr/bin"));
        }

        #[test]
        fn vllm_build_jobs_requires_a_positive_integer() {
            assert_eq!(parse_build_jobs("3"), Some(3));
            assert_eq!(parse_build_jobs("0"), None);
            assert_eq!(parse_build_jobs("many"), None);
        }

        fn safetensors_fixture() -> Vec<u8> {
            let header = br#"{"weight":{"dtype":"F32","shape":[1],"data_offsets":[0,4]}}"#;
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&(header.len() as u64).to_le_bytes());
            bytes.extend_from_slice(header);
            bytes.extend_from_slice(&[0_u8; 4]);
            bytes
        }
    }
}

#[cfg(feature = "trt-llm")]
mod trt_llm_backend {
    use super::{
        attach_worker_containment, engine_worker_command, select_runtime_compatible_cuda_home,
        validate_load_config, verify_artifact, ArtifactFormat, CancellationToken, EngineBackend,
        EngineError, FinishReason, GenerateOutput, GenerateRequest, LoadConfig, LoadedModelInfo,
        Result, TokenChunk, TokenSink, Tokenization, UsageCounters, WorkerContainment,
    };
    use serde::de::DeserializeOwned;
    use serde::Deserialize;
    use serde_json::{json, Value};
    use std::cell::{Cell, RefCell};
    use std::env;
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, ChildStdout, Stdio};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    const WORKER: &str = include_str!("trtllm_worker.py");
    const PYTHON_ENV: &str = "MAYHEM_TRTLLM_PYTHON";
    const REQUEST_TIMEOUT_ENV: &str = "MAYHEM_TRTLLM_REQUEST_TIMEOUT_SECS";
    const CACHE_DIR_ENV: &str = "MAYHEM_TRTLLM_CACHE_DIR";
    const CUDA_HOME_ENV: &str = "MAYHEM_TRTLLM_CUDA_HOME";
    const DEFAULT_REQUEST_TIMEOUT: Option<Duration> = None;

    pub struct TrtLlmBackend {
        python: PathBuf,
        worker: RefCell<Option<TrtLlmWorker>>,
        loaded: Option<LoadedModelInfo>,
        next_id: Cell<u64>,
        memory_limit_bytes: Cell<Option<u64>>,
        cache_root: RefCell<Option<PathBuf>>,
    }

    impl TrtLlmBackend {
        pub fn new() -> Result<Self> {
            let python = env::var_os(PYTHON_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("python3"));
            Self::with_python(python)
        }

        pub fn with_python(python: impl Into<PathBuf>) -> Result<Self> {
            Ok(Self {
                python: python.into(),
                worker: RefCell::new(None),
                loaded: None,
                next_id: Cell::new(1),
                memory_limit_bytes: Cell::new(None),
                cache_root: RefCell::new(None),
            })
        }

        fn call<T>(&self, op: &str, payload: Value) -> Result<T>
        where
            T: DeserializeOwned,
        {
            self.call_streaming(op, payload, &mut |_| Ok(()), None)
        }

        fn call_streaming<T>(
            &self,
            op: &str,
            payload: Value,
            sink: &mut dyn FnMut(TokenChunk) -> Result<()>,
            cancellation: Option<&CancellationToken>,
        ) -> Result<T>
        where
            T: DeserializeOwned,
        {
            let id = self.next_id.get();
            self.next_id.set(id.saturating_add(1));
            let mut worker = self.worker.borrow_mut();
            if worker.is_none() {
                *worker = Some(TrtLlmWorker::spawn(
                    &self.python,
                    self.memory_limit_bytes.get(),
                    self.cache_root.borrow().as_deref(),
                )?);
            }
            let worker = worker.as_mut().ok_or_else(|| {
                EngineError::TrtLlm("failed to start TensorRT-LLM backend worker".to_owned())
            })?;
            worker.send(id, op, payload)?;
            let mut last_progress = Instant::now();

            loop {
                let message = if op == "load" {
                    Some(worker.read_load_message()?)
                } else {
                    if cancellation.is_some_and(CancellationToken::is_cancelled) {
                        worker.terminate();
                        return Err(EngineError::Cancelled);
                    }
                    if let Some(timeout) = worker.request_timeout {
                        if last_progress.elapsed() >= timeout {
                            worker.terminate();
                            return Err(EngineError::TrtLlm(format!(
                                "TensorRT-LLM backend worker stalled for {}s without a response",
                                timeout.as_secs()
                            )));
                        }
                    }
                    worker.read_message_poll(Duration::from_millis(25))?
                };
                let Some(message) = message else {
                    continue;
                };
                last_progress = Instant::now();
                if message.id != id {
                    return Err(EngineError::TrtLlm(format!(
                        "worker response id {} did not match request id {id}",
                        message.id
                    )));
                }

                if message.kind == "token" {
                    let chunk = message.chunk.ok_or_else(|| {
                        EngineError::TrtLlm("worker token message missing chunk".to_owned())
                    })?;
                    if let Err(error) = sink(chunk) {
                        worker.terminate();
                        return Err(error);
                    }
                    continue;
                }

                if message.ok.unwrap_or(false) {
                    let result = message.result.unwrap_or(Value::Null);
                    return Ok(serde_json::from_value(result)?);
                }

                return Err(EngineError::TrtLlm(
                    message
                        .error
                        .unwrap_or_else(|| "worker returned an unknown error".to_owned()),
                ));
            }
        }

        pub fn generate_batch(
            &mut self,
            requests: &[GenerateRequest],
        ) -> Result<Vec<GenerateOutput>> {
            if requests.is_empty() {
                return Ok(Vec::new());
            }
            self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
            self.call("generate_batch", serde_json::to_value(requests)?)
        }
    }

    impl EngineBackend for TrtLlmBackend {
        fn backend_id(&self) -> &'static str {
            "trt-llm"
        }

        fn load(&mut self, config: LoadConfig) -> Result<LoadedModelInfo> {
            validate_load_config(&config)?;
            if config.artifact.format != ArtifactFormat::TensorRtLlmCheckpoint {
                return Err(EngineError::InvalidConfig(format!(
                    "TensorRT-LLM backend requires TensorRT-LLM checkpoint artifacts, got {:?}",
                    config.artifact.format
                )));
            }
            verify_artifact(&config.artifact)?;
            self.memory_limit_bytes.set(config.memory_limit_bytes);
            self.cache_root.replace(config.backend_cache_dir.clone());

            let model_path = trt_llm_model_path(&config.artifact.path)?;
            if config.trt_require_engine_dir {
                require_trt_engine_payload(config.trt_engine_dir.as_deref())?;
            }
            let info: WorkerLoadInfo = self.call("load", trt_load_payload(&config, &model_path))?;
            let loaded = LoadedModelInfo {
                backend: self.backend_id().to_owned(),
                artifact: config.artifact,
                ctx_size: config.ctx_size,
                n_ctx_train: if info.n_ctx_train == 0 {
                    config.ctx_size
                } else {
                    info.n_ctx_train
                },
                n_vocab: info.n_vocab,
            };
            self.loaded = Some(loaded.clone());
            Ok(loaded)
        }

        fn component_healthy(&mut self) -> bool {
            self.loaded.is_none()
                || self
                    .worker
                    .get_mut()
                    .as_mut()
                    .is_some_and(|worker| matches!(worker.child.try_wait(), Ok(None)))
        }

        fn process_ids(&self) -> Vec<u32> {
            self.worker
                .borrow()
                .as_ref()
                .map(|worker| vec![worker.child.id()])
                .unwrap_or_default()
        }

        fn tokenize(&self, text: &str) -> Result<Tokenization> {
            self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
            self.call("tokenize", json!({ "text": text }))
        }

        fn generate(
            &mut self,
            request: GenerateRequest,
            sink: &mut dyn TokenSink,
            cancellation: &CancellationToken,
        ) -> Result<GenerateOutput> {
            cancellation.check()?;
            request.validate_sampling()?;
            if request.max_new_tokens == 0 {
                return Ok(GenerateOutput {
                    text: String::new(),
                    usage: UsageCounters::default(),
                    finish_reason: FinishReason::Length,
                });
            }
            self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;

            self.call_streaming(
                "generate",
                serde_json::to_value(request)?,
                &mut |chunk| sink.on_token(chunk),
                Some(cancellation),
            )
        }
    }

    #[derive(Debug, Deserialize)]
    struct WorkerLoadInfo {
        #[serde(default)]
        n_ctx_train: u32,
        #[serde(default)]
        n_vocab: i32,
    }

    #[derive(Debug, Deserialize)]
    struct WorkerMessage {
        id: u64,
        #[serde(default = "default_message_kind", rename = "type")]
        kind: String,
        #[serde(default)]
        ok: Option<bool>,
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        chunk: Option<TokenChunk>,
    }

    struct TrtLlmWorker {
        child: Child,
        _containment: WorkerContainment,
        stdin: ChildStdin,
        stdout_rx: Option<Receiver<WorkerRead>>,
        reader: Option<JoinHandle<()>>,
        request_timeout: Option<Duration>,
        terminated: bool,
    }

    impl TrtLlmWorker {
        fn spawn(
            python: &Path,
            memory_limit_bytes: Option<u64>,
            cache_root: Option<&Path>,
        ) -> Result<Self> {
            Self::spawn_with_timeout(python, request_timeout()?, memory_limit_bytes, cache_root)
        }

        fn spawn_with_timeout(
            python: &Path,
            request_timeout: Option<Duration>,
            memory_limit_bytes: Option<u64>,
            cache_root: Option<&Path>,
        ) -> Result<Self> {
            let mut command = engine_worker_command(python, memory_limit_bytes);
            configure_trt_worker_environment(&mut command, python, cache_root)?;
            command
                .arg("-u")
                .arg("-c")
                .arg(WORKER)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit());
            let mut child = command.spawn().map_err(|err| {
                EngineError::TrtLlm(format!(
                    "spawning TensorRT-LLM Python worker with {} failed: {err}",
                    python.display()
                ))
            })?;
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| EngineError::TrtLlm("opening worker stdin failed".to_owned()))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| EngineError::TrtLlm("opening worker stdout failed".to_owned()))?;
            let containment =
                attach_worker_containment(&child, memory_limit_bytes).map_err(|err| {
                    EngineError::TrtLlm(format!("applying worker containment failed: {err}"))
                })?;
            let (stdout_tx, stdout_rx) = mpsc::sync_channel(super::WORKER_STDOUT_QUEUE_CAPACITY);
            let reader = thread::spawn(move || read_worker_stdout(stdout, stdout_tx));
            Ok(Self {
                child,
                _containment: containment,
                stdin,
                stdout_rx: Some(stdout_rx),
                reader: Some(reader),
                request_timeout,
                terminated: false,
            })
        }

        fn send(&mut self, id: u64, op: &str, payload: Value) -> Result<()> {
            let message = json!({
                "id": id,
                "op": op,
                "payload": payload,
            });
            serde_json::to_writer(&mut self.stdin, &message)?;
            self.stdin.write_all(b"\n")?;
            self.stdin.flush()?;
            Ok(())
        }

        #[cfg(test)]
        fn read_message(&mut self) -> Result<WorkerMessage> {
            let read = match self.request_timeout {
                Some(request_timeout) => match self
                    .stdout_rx
                    .as_ref()
                    .ok_or_else(|| {
                        EngineError::TrtLlm(
                            "TensorRT-LLM backend worker stdout reader is closed".to_owned(),
                        )
                    })?
                    .recv_timeout(request_timeout)
                {
                    Ok(read) => read,
                    Err(RecvTimeoutError::Timeout) => {
                        self.terminate();
                        return Err(EngineError::TrtLlm(format!(
                            "TensorRT-LLM backend worker stalled for {}s without a response",
                            request_timeout.as_secs()
                        )));
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        return Err(EngineError::TrtLlm(
                            "TensorRT-LLM backend worker stdout reader stopped".to_owned(),
                        ));
                    }
                },
                None => self
                    .stdout_rx
                    .as_ref()
                    .ok_or_else(|| {
                        EngineError::TrtLlm(
                            "TensorRT-LLM backend worker stdout reader is closed".to_owned(),
                        )
                    })?
                    .recv()
                    .map_err(|_| {
                        EngineError::TrtLlm(
                            "TensorRT-LLM backend worker stdout reader stopped".to_owned(),
                        )
                    })?,
            };
            Self::decode_read(read)
        }

        fn read_message_poll(&mut self, wait: Duration) -> Result<Option<WorkerMessage>> {
            match self
                .stdout_rx
                .as_ref()
                .ok_or_else(|| {
                    EngineError::TrtLlm(
                        "TensorRT-LLM backend worker stdout reader is closed".to_owned(),
                    )
                })?
                .recv_timeout(wait)
            {
                Ok(read) => Self::decode_read(read).map(Some),
                Err(RecvTimeoutError::Timeout) => Ok(None),
                Err(RecvTimeoutError::Disconnected) => Err(EngineError::TrtLlm(
                    "TensorRT-LLM backend worker stdout reader stopped".to_owned(),
                )),
            }
        }

        fn read_load_message(&mut self) -> Result<WorkerMessage> {
            let read = self
                .stdout_rx
                .as_ref()
                .ok_or_else(|| {
                    EngineError::TrtLlm(
                        "TensorRT-LLM backend worker stdout reader is closed".to_owned(),
                    )
                })?
                .recv()
                .map_err(|_| {
                    EngineError::TrtLlm(
                        "TensorRT-LLM backend worker stdout reader stopped".to_owned(),
                    )
                })?;
            Self::decode_read(read)
        }

        fn decode_read(read: WorkerRead) -> Result<WorkerMessage> {
            let line = match read {
                WorkerRead::Line(line) => line,
                WorkerRead::Eof => {
                    return Err(EngineError::TrtLlm(
                        "TensorRT-LLM backend worker exited before replying".to_owned(),
                    ));
                }
                WorkerRead::Error(error) => return Err(EngineError::TrtLlm(error)),
            };
            Ok(serde_json::from_str(line.trim_end())?)
        }

        fn terminate(&mut self) {
            if self.terminated {
                return;
            }
            terminate_worker_process(&mut self.child);
            self.terminated = true;
        }
    }

    enum WorkerRead {
        Line(String),
        Eof,
        Error(String),
    }

    fn read_worker_stdout(stdout: ChildStdout, sender: mpsc::SyncSender<WorkerRead>) {
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
                Err(err) => {
                    let _ = sender.send(WorkerRead::Error(format!(
                        "reading TensorRT-LLM backend worker stdout failed: {err}"
                    )));
                    return;
                }
            }
        }
    }

    fn configure_trt_worker_environment(
        command: &mut std::process::Command,
        python: &Path,
        configured_cache_root: Option<&Path>,
    ) -> Result<()> {
        let cache_root = env::var_os(CACHE_DIR_ENV)
            .map(PathBuf::from)
            .or_else(|| configured_cache_root.map(Path::to_path_buf))
            .or_else(|| {
                env::var_os("MAYHEM_HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join("cache/trt-llm"))
            })
            .or_else(|| {
                env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".mayhem/cache/trt-llm"))
            })
            .unwrap_or_else(|| env::temp_dir().join("mayhem-trt-llm-cache"));
        for (name, default_path) in [
            ("XDG_CACHE_HOME", cache_root.join("xdg")),
            ("HF_HOME", cache_root.join("huggingface")),
            ("HF_HUB_CACHE", cache_root.join("huggingface/hub")),
            ("TRANSFORMERS_CACHE", cache_root.join("transformers")),
            ("TORCHINDUCTOR_CACHE_DIR", cache_root.join("torchinductor")),
            ("CUDA_CACHE_PATH", cache_root.join("cuda")),
            ("TRTLLM_CACHE_DIR", cache_root.join("trtllm")),
        ] {
            let path = env::var_os(name).map(PathBuf::from).unwrap_or(default_path);
            fs::create_dir_all(&path).map_err(|err| {
                EngineError::TrtLlm(format!(
                    "creating TensorRT-LLM cache directory {} failed: {err}",
                    path.display()
                ))
            })?;
            command.env(name, path);
        }

        let cuda_home = resolve_trt_cuda_home(python).ok_or_else(|| {
            EngineError::TrtLlm(
                "TensorRT-LLM requires a CUDA toolkit containing bin/nvcc; install the toolkit or set MAYHEM_TRTLLM_CUDA_HOME"
                    .to_owned(),
            )
        })?;
        command.env("CUDA_HOME", &cuda_home);
        command.env("CUDA_PATH", &cuda_home);
        let cuda_lib = if cuda_home.join("lib").is_dir() {
            cuda_home.join("lib")
        } else {
            cuda_home.join("lib64")
        };
        command.env(
            "LD_LIBRARY_PATH",
            trt_prepend_env_paths(&[cuda_lib], env::var_os("LD_LIBRARY_PATH")),
        );
        let mut path_prefixes = vec![cuda_home.join("bin")];
        if let Some(python_bin) = python.parent() {
            path_prefixes.push(python_bin.to_path_buf());
        }
        command.env(
            "PATH",
            trt_prepend_env_paths(&path_prefixes, env::var_os("PATH")),
        );
        Ok(())
    }

    pub(super) fn resolve_trt_cuda_home(python: &Path) -> Option<PathBuf> {
        if let Some(explicit) = env::var_os(CUDA_HOME_ENV).map(PathBuf::from) {
            return trt_cuda_home_with_nvcc(explicit);
        }
        if let Some(explicit) = env::var_os("CUDA_HOME").map(PathBuf::from) {
            return trt_cuda_home_with_nvcc(explicit);
        }

        let mut candidates = trt_cuda_home_with_nvcc(PathBuf::from("/usr/local/cuda"))
            .into_iter()
            .collect::<Vec<_>>();
        candidates.extend(trt_usr_local_cuda_homes());
        candidates.extend(trt_bundled_cuda_homes(python));
        select_runtime_compatible_cuda_home(python, candidates)
    }

    fn trt_bundled_cuda_homes(python: &Path) -> Vec<PathBuf> {
        let Some(venv) = python.parent().and_then(Path::parent) else {
            return Vec::new();
        };
        let lib = venv.join("lib");
        let mut candidates = fs::read_dir(lib)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path().join("site-packages/nvidia"))
            .filter_map(|nvidia| fs::read_dir(nvidia).ok())
            .flatten()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter_map(trt_cuda_home_with_nvcc)
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.reverse();
        candidates
    }

    fn trt_usr_local_cuda_homes() -> Vec<PathBuf> {
        let mut candidates = fs::read_dir("/usr/local")
            .ok()
            .into_iter()
            .flatten()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("cuda-"))
            })
            .filter_map(trt_cuda_home_with_nvcc)
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.reverse();
        candidates
    }

    fn trt_cuda_home_with_nvcc(path: PathBuf) -> Option<PathBuf> {
        path.join("bin/nvcc").is_file().then_some(path)
    }

    fn trt_prepend_env_paths(
        prefixes: &[PathBuf],
        current: Option<std::ffi::OsString>,
    ) -> std::ffi::OsString {
        let mut paths = prefixes.to_vec();
        if let Some(current) = current {
            paths.extend(env::split_paths(&current));
        }
        env::join_paths(paths).unwrap_or_else(|_| {
            prefixes
                .first()
                .map(|path| path.as_os_str().to_owned())
                .unwrap_or_default()
        })
    }

    fn request_timeout() -> Result<Option<Duration>> {
        match env::var(REQUEST_TIMEOUT_ENV) {
            Ok(value) => request_timeout_from(Some(&value)),
            Err(env::VarError::NotPresent) => Ok(DEFAULT_REQUEST_TIMEOUT),
            Err(err) => Err(EngineError::InvalidConfig(format!(
                "reading {REQUEST_TIMEOUT_ENV} failed: {err}"
            ))),
        }
    }

    fn request_timeout_from(value: Option<&str>) -> Result<Option<Duration>> {
        let Some(value) = value else {
            return Ok(DEFAULT_REQUEST_TIMEOUT);
        };
        let seconds = value.trim().parse::<u64>().map_err(|_| {
            EngineError::InvalidConfig(format!(
                "{REQUEST_TIMEOUT_ENV} must be a non-negative integer in seconds"
            ))
        })?;
        Ok((seconds > 0).then(|| Duration::from_secs(seconds)))
    }

    fn terminate_worker_process(child: &mut Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            match child.try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) => thread::sleep(Duration::from_millis(50)),
                Err(_) => return true,
            }
        }
        false
    }

    impl Drop for TrtLlmWorker {
        fn drop(&mut self) {
            let _ = self.send(0, "shutdown", Value::Null);
            self.stdout_rx.take();
            if wait_for_child_exit(&mut self.child, Duration::from_secs(3)) {
                self.terminated = true;
            } else {
                self.terminate();
            }
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }

    fn trt_llm_model_path(path: &Path) -> Result<PathBuf> {
        if path.is_dir() {
            return Ok(path.to_path_buf());
        }
        path.parent().map(Path::to_path_buf).ok_or_else(|| {
            EngineError::InvalidConfig("TensorRT-LLM checkpoint path has no parent".to_owned())
        })
    }

    fn trt_load_payload(config: &LoadConfig, model_path: &Path) -> Value {
        json!({
            "path": model_path,
            "ctx_size": config.ctx_size,
            "max_batch_size": config.batch_size.max(1),
            "max_num_tokens": config.ubatch_size.max(config.ctx_size).max(1),
            "engine_dir": config.trt_engine_dir,
            "tensor_parallel": config.trt_tensor_parallel.unwrap_or(1),
            "kv_cache_dtype": config.trt_kv_cache_dtype,
            "require_engine_dir": config.trt_require_engine_dir,
        })
    }

    fn require_trt_engine_payload(engine_dir: Option<&Path>) -> Result<()> {
        let engine_dir = engine_dir.ok_or_else(|| {
            EngineError::InvalidConfig(
                "TensorRT-LLM loading requires a prebuilt engine directory; run the seal-time engine build first".to_owned(),
            )
        })?;
        if !engine_dir.is_dir() {
            return Err(EngineError::InvalidConfig(format!(
                "TensorRT-LLM engine directory {} is missing; run the seal-time engine build first",
                engine_dir.display()
            )));
        }
        if has_trt_engine_payload(engine_dir)? {
            return Ok(());
        }
        Err(EngineError::InvalidConfig(format!(
            "TensorRT-LLM engine directory {} contains no .engine or .plan payload; run the seal-time engine build first",
            engine_dir.display()
        )))
    }

    fn has_trt_engine_payload(engine_dir: &Path) -> Result<bool> {
        for entry in std::fs::read_dir(engine_dir)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|ext| matches!(ext.to_str(), Some("engine" | "plan")))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn default_message_kind() -> String {
        "response".to_owned()
    }

    #[cfg(test)]
    #[cfg(unix)]
    mod tests {
        use super::*;
        use crate::ModelArtifact;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        #[test]
        fn trt_worker_read_timeout_kills_silent_child() {
            let root =
                env::temp_dir().join(format!("mayhem-silent-trt-worker-{}", std::process::id()));
            let path = root.join("bin/python");
            let nvcc = root.join("lib/python3.12/site-packages/nvidia/cu13/bin/nvcc");
            fs::create_dir_all(path.parent().expect("python parent")).expect("python dir");
            fs::create_dir_all(nvcc.parent().expect("nvcc parent")).expect("CUDA dir");
            fs::write(&path, "#!/bin/sh\nexec sleep 20\n").expect("write fake worker");
            fs::write(&nvcc, "#!/bin/sh\nexit 0\n").expect("write fake nvcc");
            let mut perms = fs::metadata(&path).expect("metadata").permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&path, perms).expect("chmod fake worker");

            let mut worker =
                TrtLlmWorker::spawn_with_timeout(&path, Some(Duration::from_secs(1)), None, None)
                    .expect("spawn");
            worker
                .send(1, "load", Value::Null)
                .expect("send request to fake worker");
            let start = Instant::now();
            let err = worker.read_message().expect_err("silent worker times out");
            assert!(start.elapsed() < Duration::from_secs(5));
            assert!(
                format!("{err}").contains("stalled for 1s without a response"),
                "{err}"
            );

            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn trt_worker_timeout_override_is_validated() {
            assert_eq!(request_timeout_from(None).unwrap(), None);
            assert_eq!(request_timeout_from(Some("0")).unwrap(), None);
            assert_eq!(
                request_timeout_from(Some("600")).unwrap(),
                Some(Duration::from_secs(600))
            );
            assert!(request_timeout_from(Some("later")).is_err());
        }

        #[test]
        fn trt_worker_cancellation_terminates_inflight_generation() {
            let root = env::temp_dir().join(format!(
                "mayhem-cancel-trt-worker-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            let path = root.join("bin/python");
            let nvcc = root.join("lib/python3.12/site-packages/nvidia/cu13/bin/nvcc");
            fs::create_dir_all(path.parent().expect("python parent")).expect("python dir");
            fs::create_dir_all(nvcc.parent().expect("nvcc parent")).expect("CUDA dir");
            fs::write(&path, "#!/bin/sh\nread -r request\nexec sleep 30\n")
                .expect("write fake worker");
            fs::write(&nvcc, "#!/bin/sh\nexit 0\n").expect("write fake nvcc");
            let mut perms = fs::metadata(&path).expect("metadata").permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&path, perms).expect("chmod fake worker");

            let mut backend = TrtLlmBackend::with_python(&path).expect("backend");
            backend.loaded = Some(LoadedModelInfo {
                backend: "trt-llm".to_owned(),
                artifact: ModelArtifact::trt_llm_checkpoint(root.join("checkpoint")),
                ctx_size: 2048,
                n_ctx_train: 2048,
                n_vocab: 1,
            });
            let cancellation = CancellationToken::new();
            let cancel_from_peer = cancellation.clone();
            let cancel_thread = thread::spawn(move || {
                thread::sleep(Duration::from_millis(100));
                cancel_from_peer.cancel();
            });

            let started = Instant::now();
            let error = backend
                .call_streaming::<Value>(
                    "generate",
                    json!({}),
                    &mut |_| Ok(()),
                    Some(&cancellation),
                )
                .expect_err("cancelled generation must stop");
            cancel_thread.join().expect("cancel thread");
            assert_eq!(error.to_string(), EngineError::Cancelled.to_string());
            assert!(started.elapsed() < Duration::from_secs(2));
            assert!(!backend.component_healthy());

            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn trt_load_requires_prebuilt_engine_when_requested() {
            let root =
                env::temp_dir().join(format!("mayhem-trt-require-engine-{}", std::process::id()));
            let checkpoint = root.join("checkpoint");
            fs::create_dir_all(&checkpoint).expect("checkpoint dir");
            fs::write(checkpoint.join("config.json"), "{}").expect("checkpoint config");
            fs::write(checkpoint.join("model.safetensors"), safetensors_fixture())
                .expect("checkpoint weights");

            let mut backend = TrtLlmBackend::with_python("/does/not/matter").expect("backend");
            let mut config = LoadConfig::trt_llm_checkpoint(&checkpoint);
            config.trt_require_engine_dir = true;

            let err = backend
                .load(config.clone())
                .expect_err("missing engine dir must fail before worker spawn");
            assert!(
                format!("{err}").contains("prebuilt engine directory"),
                "{err}"
            );

            let engine_dir = root.join("engine");
            fs::create_dir_all(&engine_dir).expect("engine dir");
            config.trt_engine_dir = Some(engine_dir.clone());
            let err = backend
                .load(config.clone())
                .expect_err("empty engine dir must fail before worker spawn");
            assert!(
                format!("{err}").contains("contains no .engine or .plan"),
                "{err}"
            );

            fs::write(engine_dir.join("rank0.engine"), b"engine").expect("engine file");
            assert!(require_trt_engine_payload(Some(&engine_dir)).is_ok());

            let _ = fs::remove_dir_all(root);
        }

        #[test]
        fn trt_load_payload_carries_capacity_knobs() {
            let mut config = LoadConfig::trt_llm_checkpoint("/tmp/checkpoint");
            config.ctx_size = 1024;
            config.batch_size = 4;
            config.ubatch_size = 512;
            config.trt_engine_dir = Some(PathBuf::from("/tmp/engine"));
            config.trt_tensor_parallel = Some(2);
            config.trt_kv_cache_dtype = Some("fp8".to_owned());
            config.trt_require_engine_dir = true;

            let payload = trt_load_payload(&config, Path::new("/tmp/checkpoint"));
            assert_eq!(payload["ctx_size"], json!(1024));
            assert_eq!(payload["max_batch_size"], json!(4));
            assert_eq!(payload["max_num_tokens"], json!(1024));
            assert_eq!(payload["tensor_parallel"], json!(2));
            assert_eq!(payload["kv_cache_dtype"], json!("fp8"));
            assert_eq!(payload["require_engine_dir"], json!(true));

            config.ubatch_size = 4096;
            let payload = trt_load_payload(&config, Path::new("/tmp/checkpoint"));
            assert_eq!(payload["max_num_tokens"], json!(4096));
        }

        fn safetensors_fixture() -> Vec<u8> {
            let header = br#"{"weight":{"dtype":"F32","shape":[1],"data_offsets":[0,4]}}"#;
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&(header.len() as u64).to_le_bytes());
            bytes.extend_from_slice(header);
            bytes.extend_from_slice(&[0_u8; 4]);
            bytes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_worker_containment_uses_a_valid_minimum_working_set() {
        let maximum = 8 * 1024 * 1024;
        assert_eq!(
            windows_worker_working_set_bounds(maximum).unwrap(),
            (WINDOWS_MINIMUM_WORKING_SET_BYTES, maximum as usize)
        );
        assert!(
            windows_worker_working_set_bounds((WINDOWS_MINIMUM_WORKING_SET_BYTES - 1) as u64)
                .is_err()
        );
    }

    #[test]
    fn speciality_reasoning_detection_ignores_history_preservation_controls() {
        for (name, worker) in [
            ("mlx", include_str!("mlx_worker.py")),
            ("vllm", include_str!("vllm_worker.py")),
            ("trt-llm", include_str!("trtllm_worker.py")),
        ] {
            let history_guard = worker
                .find("(\"preserve\", \"history\", \"retain\")")
                .unwrap_or_else(|| panic!("{name} worker lacks the reasoning-history guard"));
            let value_check = worker[history_guard..]
                .find("value = item.get(\"value\")")
                .map(|offset| history_guard + offset)
                .unwrap_or_else(|| panic!("{name} worker lacks reasoning value handling"));
            assert!(
                history_guard < value_check,
                "{name} must ignore history controls before evaluating reasoning values"
            );
        }
    }

    #[test]
    fn tokenizer_workers_forward_tool_definitions_into_chat_templates() {
        for (name, worker) in [
            ("mlx", include_str!("mlx_worker.py")),
            ("vllm", include_str!("vllm_worker.py")),
            ("trt-llm", include_str!("trtllm_worker.py")),
        ] {
            assert!(
                worker.contains("payload.get(\"tools\")"),
                "{name} worker does not read template tools"
            );
            assert!(
                worker.contains("[\"tools\"] = template_tools"),
                "{name} worker does not pass tools to its chat template"
            );
        }
    }

    #[test]
    fn mlx_worker_multimodal_path_is_local_bounded_and_processor_backed() {
        let worker = include_str!("mlx_worker.py");
        assert!(worker.contains("processor_chat_messages"));
        assert!(worker.contains("processor.apply_chat_template"));
        assert!(worker.contains("remote media URLs are forbidden"));
        assert!(worker.contains("num_frames must be between 1 and 64"));
        assert!(worker.contains("np.transpose(np.stack"));
        assert!(worker.contains("av.open"));
        assert!(worker.contains("generation_kwargs[\"thinking_budget\"]"));
        assert!(worker.contains("generation_kwargs[\"enable_thinking\"]"));
        assert!(worker.contains("if images or videos"));
        assert!(worker.contains("if audios and not images and not videos"));
        assert!(worker.contains("build_json_schema_logits_processor"));
        assert!(worker.contains("ThinkingAwareLogitsProcessor"));
        assert!(worker.contains("class StopSequenceStream"));
        assert!(worker.contains("class MlxGenerationEvents"));
        assert!(worker.contains("len(self.pending) > 1"));
        assert!(worker.contains("effective_top_k"));
        assert!(worker.contains("snapshot_mlx_random_state"));
        assert!(worker.contains("restore_mlx_random_state"));
        assert!(worker.contains("install_qwen35_mixed_visual_support"));
        assert!(worker.contains("pixel_values_videos"));
        assert!(worker.contains("image_grid_thw"));
        assert!(worker.contains("video_grid_thw"));
        assert!(!worker.contains("advertise caps.tools=false"));
    }

    use std::io::Write;

    #[cfg(any(feature = "trt-llm", feature = "vllm"))]
    #[test]
    fn cuda_version_parser_reads_runtime_and_nvcc_forms() {
        assert_eq!(parse_cuda_major_minor("13.0"), Some((13, 0)));
        assert_eq!(
            parse_cuda_major_minor("Cuda compilation tools, release 13.2, V13.2.78"),
            Some((13, 2))
        );
        assert_eq!(parse_cuda_major_minor("None"), None);
    }

    #[cfg(all(any(feature = "trt-llm", feature = "vllm"), unix))]
    #[test]
    fn cuda_home_selection_matches_the_torch_runtime() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "mayhem-cuda-runtime-selection-{}",
            std::process::id()
        ));
        let python = root.join("venv/bin/python");
        let torch_version = root.join("venv/lib/python3.12/site-packages/torch/version.py");
        let cuda_13_2 = root.join("cuda-13.2");
        let cuda_13_0 = root.join("cuda-13.0");
        std::fs::create_dir_all(python.parent().unwrap()).unwrap();
        std::fs::create_dir_all(torch_version.parent().unwrap()).unwrap();
        std::fs::write(&python, b"").unwrap();
        std::fs::write(&torch_version, "cuda: Optional[str] = '13.0'\n").unwrap();

        for (cuda, version) in [(&cuda_13_2, "13.2"), (&cuda_13_0, "13.0")] {
            let nvcc = cuda.join("bin/nvcc");
            std::fs::create_dir_all(nvcc.parent().unwrap()).unwrap();
            std::fs::write(
                &nvcc,
                format!(
                    "#!/bin/sh\necho 'Cuda compilation tools, release {version}, V{version}'\n"
                ),
            )
            .unwrap();
            let mut permissions = std::fs::metadata(&nvcc).unwrap().permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(&nvcc, permissions).unwrap();
        }

        assert_eq!(
            select_runtime_compatible_cuda_home(&python, [cuda_13_2.clone(), cuda_13_0.clone()]),
            Some(cuda_13_0)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    struct EchoBackend;

    impl EngineBackend for EchoBackend {
        fn backend_id(&self) -> &'static str {
            "echo"
        }

        fn load(&mut self, config: LoadConfig) -> Result<LoadedModelInfo> {
            Ok(LoadedModelInfo {
                backend: self.backend_id().to_owned(),
                artifact: config.artifact,
                ctx_size: config.ctx_size,
                n_ctx_train: config.ctx_size,
                n_vocab: 0,
            })
        }

        fn tokenize(&self, text: &str) -> Result<Tokenization> {
            Ok(Tokenization {
                token_ids: text.bytes().map(i32::from).collect(),
            })
        }

        fn generate(
            &mut self,
            request: GenerateRequest,
            _sink: &mut dyn TokenSink,
            _cancellation: &CancellationToken,
        ) -> Result<GenerateOutput> {
            Ok(GenerateOutput {
                text: request.prompt,
                usage: UsageCounters::new(1, 1),
                finish_reason: FinishReason::Stop,
            })
        }
    }

    #[test]
    fn exposes_crate_name() {
        assert_eq!(CRATE_NAME, "mayhem-engine");
    }

    #[test]
    fn media_validation_allows_only_task_complete_music_without_a_prompt() {
        let request = |endpoint_family: &str, request: Value| MediaGenerationRequest {
            endpoint_family: endpoint_family.to_owned(),
            prompt: String::new(),
            request,
            duration_seconds: None,
            frame_count: None,
            step_count: None,
            response_format: None,
        };

        request(
            "mayhem_music_generations",
            json!({"lyrics": "[Verse]\nA lyrics-only song"}),
        )
        .validate()
        .expect("music accepts lyrics-only text2music");
        request(
            "mayhem_music_generations",
            json!({"caption": "A caption-only song"}),
        )
        .validate()
        .expect("music accepts the caption prompt alias");
        request(
            "mayhem_music_generations",
            json!({
                "task_type": "cover",
                "source_audio": {
                    "encoding": "base64",
                    "data": "UklGRg==",
                    "content_type": "audio/wav"
                }
            }),
        )
        .validate()
        .expect("music accepts a source-driven cover");

        for invalid in [
            request("mayhem_audio_generations", json!({"lyrics": "not music"})),
            request("mayhem_music_generations", json!({})),
            request(
                "mayhem_music_generations",
                json!({"task_type": "cover", "lyrics": "missing source"}),
            ),
            request(
                "mayhem_music_generations",
                json!({"task_type": "extract", "source_audio": "UklGRg=="}),
            ),
        ] {
            assert!(invalid.validate().is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn cancellable_child_command_kills_inflight_process() {
        use std::time::{Duration, Instant};

        let cancellation = CancellationToken::new();
        let peer_cancellation = cancellation.clone();
        let cancel_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            peer_cancellation.cancel();
        });
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "exec sleep 30"]);

        let started = Instant::now();
        let error = run_command_cancellable(&mut command, &cancellation, EngineError::Io)
            .expect_err("cancelled child must stop");
        cancel_thread.join().expect("cancel thread");
        assert_eq!(error.to_string(), EngineError::Cancelled.to_string());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn default_generate_with_artifacts_delegates_without_artifacts() {
        let mut backend = EchoBackend;
        let mut token_chunks = Vec::new();
        let mut artifact_chunks = Vec::new();

        let output = backend
            .generate_with_artifacts(
                GenerateRequest::new("hello"),
                &mut |chunk| {
                    token_chunks.push(chunk);
                    Ok(())
                },
                &mut |chunk| {
                    artifact_chunks.push(chunk);
                    Ok(())
                },
                &CancellationToken::new(),
            )
            .unwrap();

        assert_eq!(output.text, "hello");
        assert!(token_chunks.is_empty());
        assert!(artifact_chunks.is_empty());
    }

    #[test]
    fn tool_call_schema_restricts_names() {
        let schema = tool_call_json_schema(&[
            ToolSpec::new("lookup", json!({"type": "object"})),
            ToolSpec::new("quote", json!({"type": "object"})),
        ])
        .expect("schema");

        let names = &schema["properties"]["tool"]["enum"];
        assert!(names.as_array().expect("enum").contains(&json!("lookup")));
        assert!(names.as_array().expect("enum").contains(&json!("quote")));
        assert_eq!(schema["required"], json!(["tool", "arguments"]));
    }

    #[test]
    fn verifies_gguf_header() {
        let path = std::env::temp_dir().join(format!(
            "mayhem-engine-test-{}-{}.gguf",
            std::process::id(),
            "header"
        ));
        let mut file = File::create(&path).expect("temp gguf");
        file.write_all(b"GGUFtest").expect("write header");
        drop(file);

        let artifact = ModelArtifact::gguf(&path);
        verify_artifact(&artifact).expect("valid gguf header");
        std::fs::remove_file(path).expect("remove temp gguf");
    }

    #[test]
    fn verifies_mlx_safetensors_header_from_file_and_directory() {
        let dir = std::env::temp_dir().join(format!(
            "mayhem-engine-test-{}-{}",
            std::process::id(),
            "mlx-safetensors"
        ));
        std::fs::create_dir_all(&dir).expect("temp mlx dir");
        let path = dir.join("model.safetensors");
        let header = br#"{"__metadata__":{}}"#;
        let mut file = File::create(&path).expect("temp safetensors");
        file.write_all(&(header.len() as u64).to_le_bytes())
            .expect("write header length");
        file.write_all(header).expect("write header");
        file.write_all(b"weights").expect("write body");
        drop(file);

        verify_artifact(&ModelArtifact::mlx_safetensors(&path)).expect("valid safetensors file");
        verify_artifact(&ModelArtifact::mlx_safetensors(&dir)).expect("valid safetensors dir");
        std::fs::remove_dir_all(dir).expect("remove temp mlx dir");
    }

    #[test]
    fn mlx_multimodal_runtime_is_bound_to_mlx_artifacts() {
        let mut mlx = LoadConfig::mlx_safetensors("/tmp/model.safetensors");
        mlx.mlx_runtime.multimodal = true;
        validate_load_config(&mlx).expect("MLX artifact accepts MLX multimodal semantics");

        let mut gguf = LoadConfig::gguf("/tmp/model.gguf");
        gguf.mlx_runtime.multimodal = true;
        let error = validate_load_config(&gguf).expect_err("GGUF must reject MLX semantics");
        assert!(error
            .to_string()
            .contains("require an MLX safetensors artifact"));
    }

    #[test]
    fn verifies_trt_llm_checkpoint_payloads() {
        let dir = std::env::temp_dir().join(format!(
            "mayhem-engine-test-{}-{}",
            std::process::id(),
            "trt-llm"
        ));
        std::fs::create_dir_all(&dir).expect("temp trt dir");
        std::fs::write(dir.join("config.json"), br#"{"architecture":"test"}"#)
            .expect("write config");
        let path = dir.join("rank0.safetensors");
        let header = br#"{"__metadata__":{}}"#;
        let mut file = File::create(&path).expect("temp safetensors");
        file.write_all(&(header.len() as u64).to_le_bytes())
            .expect("write header length");
        file.write_all(header).expect("write header");
        file.write_all(b"weights").expect("write body");
        drop(file);

        verify_artifact(&ModelArtifact::trt_llm_checkpoint(&path))
            .expect("valid TensorRT-LLM safetensors file");
        verify_artifact(&ModelArtifact::trt_llm_checkpoint(&dir))
            .expect("valid TensorRT-LLM checkpoint dir");

        let mut config = LoadConfig::trt_llm_checkpoint(&dir);
        config.trt_engine_dir = Some(dir.join("engine-cache"));
        config.trt_tensor_parallel = Some(2);
        config.trt_kv_cache_dtype = Some("nvfp4".to_owned());
        assert_eq!(
            config.artifact.format,
            ArtifactFormat::TensorRtLlmCheckpoint
        );
        assert_eq!(config.trt_tensor_parallel, Some(2));
        std::fs::remove_dir_all(dir).expect("remove temp trt dir");
    }

    #[test]
    fn verifies_vllm_safetensors_payloads() {
        let dir = std::env::temp_dir().join(format!(
            "mayhem-engine-test-{}-{}",
            std::process::id(),
            "vllm"
        ));
        std::fs::create_dir_all(&dir).expect("temp vllm dir");
        std::fs::write(dir.join("config.json"), br#"{"architectures":["TestLM"]}"#)
            .expect("write config");
        let path = dir.join("model.safetensors");
        let header = br#"{"__metadata__":{}}"#;
        let mut file = File::create(&path).expect("temp safetensors");
        file.write_all(&(header.len() as u64).to_le_bytes())
            .expect("write header length");
        file.write_all(header).expect("write header");
        file.write_all(b"weights").expect("write body");
        drop(file);

        verify_artifact(&ModelArtifact::vllm_safetensors(&path))
            .expect("valid vLLM safetensors file");
        verify_artifact(&ModelArtifact::vllm_safetensors(&dir)).expect("valid vLLM checkpoint dir");

        let mut config = LoadConfig::vllm_safetensors(&dir);
        config.vllm_tensor_parallel = Some(2);
        config.vllm_dtype = Some("float16".to_owned());
        assert_eq!(config.artifact.format, ArtifactFormat::VllmSafetensors);
        assert_eq!(config.vllm_tensor_parallel, Some(2));
        assert_eq!(config.vllm_dtype.as_deref(), Some("float16"));
        std::fs::remove_dir_all(dir).expect("remove temp vllm dir");
    }

    #[cfg(feature = "llama-cpp")]
    #[test]
    fn converts_tool_schema_to_grammar() {
        let grammar = llama_cpp_2::json_schema_to_grammar(
            r#"{"type":"object","additionalProperties":false,"properties":{"ok":{"type":"boolean"}},"required":["ok"]}"#,
        )
        .expect("grammar");
        assert!(grammar.contains("root ::="));
    }

    #[test]
    fn tool_call_gbnf_restricts_names() {
        let grammar = tool_call_gbnf(&[
            ToolSpec::new("lookup", json!({"type": "object"})),
            ToolSpec::new("quote", json!({"type": "object"})),
        ])
        .expect("gbnf");

        assert!(grammar.contains("tool-name ::="));
        assert!(grammar.contains(r#""\"lookup\"""#));
        assert!(grammar.contains(r#""\"quote\"""#));
    }
}
