#![forbid(unsafe_code)]

use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const CRATE_NAME: &str = "mayhem-engine";
pub const DEFAULT_CONTEXT_SIZE: u32 = 2048;
pub const DEFAULT_BATCH_SIZE: u32 = 512;
pub const DEFAULT_UBATCH_SIZE: u32 = 512;
pub const DEFAULT_SEED: u32 = 0x4d415948;

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
    #[error("model has not been loaded")]
    NotLoaded,
    #[error("prompt has {prompt_tokens} tokens, leaving no room in ctx_size={ctx_size}")]
    PromptTooLong { prompt_tokens: usize, ctx_size: u32 },
    #[error("MLX backend error: {0}")]
    Mlx(String),
    #[error("TensorRT-LLM backend error: {0}")]
    TrtLlm(String),
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactFormat {
    Gguf,
    MlxSafetensors,
    TensorRtLlmCheckpoint,
}

impl ArtifactFormat {
    fn magic(&self) -> &'static [u8] {
        match self {
            Self::Gguf => b"GGUF",
            Self::MlxSafetensors => b"",
            Self::TensorRtLlmCheckpoint => b"",
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Gguf => "GGUF",
            Self::MlxSafetensors => "MLX safetensors",
            Self::TensorRtLlmCheckpoint => "TensorRT-LLM checkpoint",
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

    #[must_use]
    pub fn with_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.sha256 = Some(sha256.into());
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LoadConfig {
    pub artifact: ModelArtifact,
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
    #[serde(default = "default_true")]
    pub use_mmap: bool,
    #[serde(default)]
    pub use_mlock: bool,
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
}

impl Default for LoadConfig {
    fn default() -> Self {
        Self {
            artifact: ModelArtifact::gguf(PathBuf::new()),
            ctx_size: DEFAULT_CONTEXT_SIZE,
            batch_size: DEFAULT_BATCH_SIZE,
            ubatch_size: DEFAULT_UBATCH_SIZE,
            threads: None,
            gpu_layers: None,
            trt_engine_dir: None,
            trt_tensor_parallel: None,
            trt_kv_cache_dtype: None,
            use_mmap: true,
            use_mlock: false,
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
    #[serde(default = "default_max_new_tokens")]
    pub max_new_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grammar: Option<GrammarSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
}

impl GenerateRequest {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            max_new_tokens: default_max_new_tokens(),
            grammar: None,
            temperature: None,
            top_p: None,
            seed: None,
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
pub struct GenerateOutput {
    pub text: String,
    pub usage: UsageCounters,
    pub finish_reason: FinishReason,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct UsageCounters {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl UsageCounters {
    #[must_use]
    pub fn new(prompt_tokens: u32, completion_tokens: u32) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens.saturating_add(completion_tokens),
        }
    }
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

pub trait EngineBackend {
    fn backend_id(&self) -> &'static str;
    fn load(&mut self, config: LoadConfig) -> Result<LoadedModelInfo>;
    fn tokenize(&self, text: &str) -> Result<Tokenization>;
    fn generate(
        &mut self,
        request: GenerateRequest,
        sink: &mut dyn TokenSink,
    ) -> Result<GenerateOutput>;
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

#[cfg(feature = "llama-cpp")]
mod llama_cpp_backend {
    use encoding_rs::UTF_8;
    use llama_cpp_2::context::params::LlamaContextParams;
    use llama_cpp_2::llama_backend::LlamaBackend;
    use llama_cpp_2::llama_batch::LlamaBatch;
    use llama_cpp_2::model::params::LlamaModelParams;
    use llama_cpp_2::model::{AddBos, LlamaModel};
    use llama_cpp_2::sampling::LlamaSampler;
    use llama_cpp_2::token::LlamaToken;

    use super::{
        tool_call_json_schema, validate_load_config, verify_artifact, ArtifactFormat,
        EngineBackend, EngineError, FinishReason, GenerateOutput, GenerateRequest, GrammarSpec,
        LoadConfig, LoadedModelInfo, Result, TokenChunk, TokenSink, Tokenization, UsageCounters,
        DEFAULT_SEED,
    };
    use std::num::NonZeroU32;

    #[derive(Debug)]
    pub struct LlamaCppBackend {
        backend: LlamaBackend,
        model: Option<LlamaModel>,
        loaded: Option<LoadedModelInfo>,
        config: Option<LoadConfig>,
    }

    impl LlamaCppBackend {
        pub fn new() -> Result<Self> {
            let mut backend = LlamaBackend::init()?;
            backend.void_logs();
            Ok(Self {
                backend,
                model: None,
                loaded: None,
                config: None,
            })
        }

        fn model(&self) -> Result<&LlamaModel> {
            self.model.as_ref().ok_or(EngineError::NotLoaded)
        }

        fn config(&self) -> Result<&LoadConfig> {
            self.config.as_ref().ok_or(EngineError::NotLoaded)
        }
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
            let info = LoadedModelInfo {
                backend: self.backend_id().to_owned(),
                artifact: config.artifact.clone(),
                ctx_size: config.ctx_size,
                n_ctx_train: model.n_ctx_train(),
                n_vocab: model.n_vocab(),
            };

            self.model = Some(model);
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
        ) -> Result<GenerateOutput> {
            if request.max_new_tokens == 0 {
                return Ok(GenerateOutput {
                    text: String::new(),
                    usage: UsageCounters::default(),
                    finish_reason: FinishReason::Length,
                });
            }

            let config = self.config()?.clone();
            let model = self.model()?;
            let ctx_size = NonZeroU32::new(config.ctx_size).ok_or_else(|| {
                EngineError::InvalidConfig("ctx_size must be greater than zero".to_owned())
            })?;
            let mut ctx_params = LlamaContextParams::default()
                .with_n_ctx(Some(ctx_size))
                .with_n_batch(config.batch_size)
                .with_n_ubatch(config.ubatch_size)
                .with_n_seq_max(1)
                .with_no_perf(true);
            if let Some(threads) = config.threads {
                ctx_params = ctx_params
                    .with_n_threads(threads)
                    .with_n_threads_batch(threads);
            }

            let mut ctx = model.new_context(&self.backend, ctx_params)?;
            let prompt_tokens = model.str_to_token(&request.prompt, AddBos::Always)?;
            if prompt_tokens.len() >= ctx.n_ctx() as usize {
                return Err(EngineError::PromptTooLong {
                    prompt_tokens: prompt_tokens.len(),
                    ctx_size: ctx.n_ctx(),
                });
            }

            let mut batch = LlamaBatch::new(prompt_tokens.len().max(1), 1);
            let last_prompt_index = prompt_tokens.len().saturating_sub(1);
            for (index, token) in prompt_tokens.iter().enumerate() {
                batch.add(
                    *token,
                    i32::try_from(index).map_err(|err| {
                        EngineError::InvalidConfig(format!("prompt position overflow: {err}"))
                    })?,
                    &[0],
                    index == last_prompt_index,
                )?;
            }
            ctx.decode(&mut batch)?;

            let mut sampler = make_sampler(model, &request)?;
            let mut decoder = UTF_8.new_decoder();
            let mut output = String::new();
            let mut completion_tokens = 0_u32;
            let mut finish_reason = FinishReason::Length;
            let mut next_pos = i32::try_from(prompt_tokens.len()).map_err(|err| {
                EngineError::InvalidConfig(format!("prompt token count overflow: {err}"))
            })?;

            while completion_tokens < request.max_new_tokens {
                let token = sampler.sample(&ctx, batch.n_tokens() - 1);
                if model.is_eog_token(token) {
                    finish_reason = FinishReason::Stop;
                    break;
                }

                let text = model.token_to_piece(token, &mut decoder, true, None)?;
                output.push_str(&text);
                sink.on_token(TokenChunk {
                    index: completion_tokens,
                    token_id: token.0,
                    text,
                })?;
                completion_tokens = completion_tokens.saturating_add(1);

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
            }

            Ok(GenerateOutput {
                text: output,
                usage: UsageCounters::new(prompt_tokens.len() as u32, completion_tokens),
                finish_reason,
            })
        }
    }

    fn make_sampler(model: &LlamaModel, request: &GenerateRequest) -> Result<LlamaSampler> {
        let mut samplers = Vec::new();
        if let Some(grammar) = &request.grammar {
            samplers.push(make_grammar_sampler(model, grammar)?);
        }

        if let Some(temperature) = request.temperature {
            if temperature > 0.0 {
                samplers.push(LlamaSampler::temp(temperature));
            }
        }
        if let Some(top_p) = request.top_p {
            if top_p > 0.0 && top_p < 1.0 {
                samplers.push(LlamaSampler::top_p(top_p, 1));
            }
        }

        if request.temperature.unwrap_or(0.0) <= 0.0 {
            samplers.push(LlamaSampler::greedy());
        } else {
            samplers.push(LlamaSampler::dist(request.seed.unwrap_or(DEFAULT_SEED)));
        }

        Ok(LlamaSampler::chain_simple(samplers))
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
}

#[cfg(feature = "mlx")]
mod mlx_backend {
    use super::{
        validate_load_config, verify_artifact, ArtifactFormat, EngineBackend, EngineError,
        FinishReason, GenerateOutput, GenerateRequest, LoadConfig, LoadedModelInfo, Result,
        TokenChunk, TokenSink, Tokenization, UsageCounters,
    };
    use serde::de::DeserializeOwned;
    use serde::Deserialize;
    use serde_json::{json, Value};
    use std::cell::{Cell, RefCell};
    use std::env;
    use std::io::{BufRead, BufReader, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

    const WORKER: &str = include_str!("mlx_worker.py");
    const PYTHON_ENV: &str = "MAYHEM_MLX_PYTHON";

    pub struct MlxBackend {
        python: PathBuf,
        worker: RefCell<Option<MlxWorker>>,
        loaded: Option<LoadedModelInfo>,
        next_id: Cell<u64>,
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
            })
        }

        fn call<T>(&self, op: &str, payload: Value) -> Result<T>
        where
            T: DeserializeOwned,
        {
            self.call_streaming(op, payload, &mut |_| Ok(()))
        }

        fn call_streaming<T>(
            &self,
            op: &str,
            payload: Value,
            sink: &mut dyn FnMut(TokenChunk) -> Result<()>,
        ) -> Result<T>
        where
            T: DeserializeOwned,
        {
            let id = self.next_id.get();
            self.next_id.set(id.saturating_add(1));
            let mut worker = self.worker.borrow_mut();
            if worker.is_none() {
                *worker = Some(MlxWorker::spawn(&self.python)?);
            }
            let worker = worker
                .as_mut()
                .ok_or_else(|| EngineError::Mlx("failed to start MLX backend worker".to_owned()))?;
            worker.send(id, op, payload)?;

            loop {
                let message = worker.read_message()?;
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
                    sink(chunk)?;
                    continue;
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

            let model_path = mlx_model_path(&config.artifact.path)?;
            let info: WorkerLoadInfo = self.call(
                "load",
                json!({
                    "path": model_path,
                    "ctx_size": config.ctx_size,
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

        fn tokenize(&self, text: &str) -> Result<Tokenization> {
            self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
            self.call("tokenize", json!({ "text": text }))
        }

        fn generate(
            &mut self,
            request: GenerateRequest,
            sink: &mut dyn TokenSink,
        ) -> Result<GenerateOutput> {
            if request.max_new_tokens == 0 {
                return Ok(GenerateOutput {
                    text: String::new(),
                    usage: UsageCounters::default(),
                    finish_reason: FinishReason::Length,
                });
            }
            self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;

            self.call_streaming("generate", serde_json::to_value(request)?, &mut |chunk| {
                sink.on_token(chunk)
            })
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
    }

    struct MlxWorker {
        child: Child,
        stdin: ChildStdin,
        stdout: BufReader<ChildStdout>,
    }

    impl MlxWorker {
        fn spawn(python: &Path) -> Result<Self> {
            let mut child = Command::new(python)
                .arg("-u")
                .arg("-c")
                .arg(WORKER)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(|err| {
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
            Ok(Self {
                child,
                stdin,
                stdout: BufReader::new(stdout),
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

        fn read_message(&mut self) -> Result<WorkerMessage> {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line)?;
            if read == 0 {
                return Err(EngineError::Mlx(
                    "MLX backend worker exited before replying".to_owned(),
                ));
            }
            Ok(serde_json::from_str(line.trim_end())?)
        }
    }

    impl Drop for MlxWorker {
        fn drop(&mut self) {
            let _ = self.send(0, "shutdown", Value::Null);
            let _ = self.child.kill();
            let _ = self.child.wait();
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
}

#[cfg(feature = "trt-llm")]
mod trt_llm_backend {
    use super::{
        validate_load_config, verify_artifact, ArtifactFormat, EngineBackend, EngineError,
        FinishReason, GenerateOutput, GenerateRequest, LoadConfig, LoadedModelInfo, Result,
        TokenChunk, TokenSink, Tokenization, UsageCounters,
    };
    use serde::de::DeserializeOwned;
    use serde::Deserialize;
    use serde_json::{json, Value};
    use std::cell::{Cell, RefCell};
    use std::env;
    use std::io::{BufRead, BufReader, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    const WORKER: &str = include_str!("trtllm_worker.py");
    const PYTHON_ENV: &str = "MAYHEM_TRTLLM_PYTHON";
    const REQUEST_TIMEOUT_ENV: &str = "MAYHEM_TRTLLM_REQUEST_TIMEOUT_SECS";
    const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

    pub struct TrtLlmBackend {
        python: PathBuf,
        worker: RefCell<Option<TrtLlmWorker>>,
        loaded: Option<LoadedModelInfo>,
        next_id: Cell<u64>,
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
            })
        }

        fn call<T>(&self, op: &str, payload: Value) -> Result<T>
        where
            T: DeserializeOwned,
        {
            self.call_streaming(op, payload, &mut |_| Ok(()))
        }

        fn call_streaming<T>(
            &self,
            op: &str,
            payload: Value,
            sink: &mut dyn FnMut(TokenChunk) -> Result<()>,
        ) -> Result<T>
        where
            T: DeserializeOwned,
        {
            let id = self.next_id.get();
            self.next_id.set(id.saturating_add(1));
            let mut worker = self.worker.borrow_mut();
            if worker.is_none() {
                *worker = Some(TrtLlmWorker::spawn(&self.python)?);
            }
            let worker = worker.as_mut().ok_or_else(|| {
                EngineError::TrtLlm("failed to start TensorRT-LLM backend worker".to_owned())
            })?;
            worker.send(id, op, payload)?;

            loop {
                let message = worker.read_message()?;
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
                    sink(chunk)?;
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

            let model_path = trt_llm_model_path(&config.artifact.path)?;
            let info: WorkerLoadInfo = self.call(
                "load",
                json!({
                    "path": model_path,
                    "ctx_size": config.ctx_size,
                    "engine_dir": config.trt_engine_dir,
                    "tensor_parallel": config.trt_tensor_parallel.unwrap_or(1),
                    "kv_cache_dtype": config.trt_kv_cache_dtype,
                }),
            )?;
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

        fn tokenize(&self, text: &str) -> Result<Tokenization> {
            self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
            self.call("tokenize", json!({ "text": text }))
        }

        fn generate(
            &mut self,
            request: GenerateRequest,
            sink: &mut dyn TokenSink,
        ) -> Result<GenerateOutput> {
            if request.max_new_tokens == 0 {
                return Ok(GenerateOutput {
                    text: String::new(),
                    usage: UsageCounters::default(),
                    finish_reason: FinishReason::Length,
                });
            }
            self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;

            self.call_streaming("generate", serde_json::to_value(request)?, &mut |chunk| {
                sink.on_token(chunk)
            })
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
        stdin: ChildStdin,
        stdout_rx: Receiver<WorkerRead>,
        reader: Option<JoinHandle<()>>,
        request_timeout: Duration,
        terminated: bool,
    }

    impl TrtLlmWorker {
        fn spawn(python: &Path) -> Result<Self> {
            Self::spawn_with_timeout(python, request_timeout())
        }

        fn spawn_with_timeout(python: &Path, request_timeout: Duration) -> Result<Self> {
            let mut command = Command::new(python);
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
            let (stdout_tx, stdout_rx) = mpsc::channel();
            let reader = thread::spawn(move || read_worker_stdout(stdout, stdout_tx));
            Ok(Self {
                child,
                stdin,
                stdout_rx,
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

        fn read_message(&mut self) -> Result<WorkerMessage> {
            let line = match self.stdout_rx.recv_timeout(self.request_timeout) {
                Ok(WorkerRead::Line(line)) => line,
                Ok(WorkerRead::Eof) => {
                    return Err(EngineError::TrtLlm(
                        "TensorRT-LLM backend worker exited before replying".to_owned(),
                    ));
                }
                Ok(WorkerRead::Error(error)) => return Err(EngineError::TrtLlm(error)),
                Err(RecvTimeoutError::Timeout) => {
                    self.terminate();
                    return Err(EngineError::TrtLlm(format!(
                        "TensorRT-LLM backend worker timed out after {}s waiting for a response",
                        self.request_timeout.as_secs()
                    )));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(EngineError::TrtLlm(
                        "TensorRT-LLM backend worker stdout reader stopped".to_owned(),
                    ));
                }
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

    fn read_worker_stdout(stdout: ChildStdout, sender: mpsc::Sender<WorkerRead>) {
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

    fn request_timeout() -> Duration {
        env::var(REQUEST_TIMEOUT_ENV)
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|secs| *secs > 0)
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT)
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

    fn default_message_kind() -> String {
        "response".to_owned()
    }

    #[cfg(test)]
    #[cfg(unix)]
    mod tests {
        use super::*;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        #[test]
        fn trt_worker_read_timeout_kills_silent_child() {
            let path =
                env::temp_dir().join(format!("mayhem-silent-trt-worker-{}", std::process::id()));
            fs::write(&path, "#!/bin/sh\nexec sleep 20\n").expect("write fake worker");
            let mut perms = fs::metadata(&path).expect("metadata").permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&path, perms).expect("chmod fake worker");

            let mut worker =
                TrtLlmWorker::spawn_with_timeout(&path, Duration::from_secs(1)).expect("spawn");
            worker
                .send(1, "load", Value::Null)
                .expect("send request to fake worker");
            let start = Instant::now();
            let err = worker.read_message().expect_err("silent worker times out");
            assert!(start.elapsed() < Duration::from_secs(5));
            assert!(format!("{err}").contains("timed out after 1s"), "{err}");

            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn exposes_crate_name() {
        assert_eq!(CRATE_NAME, "mayhem-engine");
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
