#![forbid(unsafe_code)]

use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::PathBuf;

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
}

impl ArtifactFormat {
    fn magic(&self) -> &'static [u8] {
        match self {
            Self::Gguf => b"GGUF",
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Gguf => "GGUF",
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

    let mut file = File::open(&artifact.path)?;
    let mut actual = vec![0_u8; artifact.format.magic().len()];
    file.read_exact(&mut actual)?;
    if actual.as_slice() != artifact.format.magic() {
        return Err(EngineError::UnsupportedArtifactHeader {
            path: artifact.path.clone(),
            expected: artifact.format.label(),
            actual,
        });
    }

    if let Some(expected) = &artifact.sha256 {
        let actual = file_sha256_hex(&artifact.path)?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(EngineError::ArtifactHashMismatch {
                path: artifact.path.clone(),
                expected: expected.clone(),
                actual,
            });
        }
    }

    Ok(())
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
        tool_call_json_schema, verify_artifact, EngineBackend, EngineError, FinishReason,
        GenerateOutput, GenerateRequest, GrammarSpec, LoadConfig, LoadedModelInfo, Result,
        TokenChunk, TokenSink, Tokenization, UsageCounters, DEFAULT_SEED,
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
