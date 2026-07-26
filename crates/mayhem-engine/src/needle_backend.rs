use super::{
    validate_load_config, verify_safetensors_header_as, ArtifactFormat, CancellationToken,
    EngineBackend, EngineError, FinishReason, GenerateOutput, GenerateRequest, GrammarSpec,
    LoadConfig, LoadedModelInfo, Result, TokenChunk, TokenSink, Tokenization, UsageCounters,
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
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const WORKER: &str = include_str!("needle_worker.py");
const WORKER_STDIN_BOOTSTRAP: &str = concat!(
    "import base64,sys;",
    "exec(compile(base64.b64decode(sys.stdin.buffer.readline()),",
    "'<mayhem-needle-worker>','exec'))"
);
const PYTHON_ENV: &str = "MAYHEM_NEEDLE_PYTHON";
const DEVICE_ENV: &str = "MAYHEM_NEEDLE_DEVICE";
const MAX_ENCODER_TOKENS: u32 = 1_024;
const MAX_DECODER_TOKENS: u32 = 512;
const MAX_WORKER_REQUEST_LINE_BYTES: usize = 2 * 1024 * 1024;
const MAX_WORKER_RESPONSE_LINE_BYTES: usize = 4 * 1024 * 1024;
const WORKER_STDERR_TAIL_BYTES: usize = 64 * 1024;
const REQUIRED_MODEL_FILES: &[&str] = &[
    "config.json",
    "configuration_needle.py",
    "model.safetensors",
    "modeling_needle.py",
    "special_tokens_map.json",
    "tokenization_needle.py",
    "tokenizer.model",
    "tokenizer_config.json",
];

pub const NEEDLE_SOURCE_COMMIT: &str = "ffd0d081401257fee31150d30c494b2f98910fc0";
pub const NEEDLE_MODEL_REVISION: &str = "5f89b4307696d669c3df1d38ae057e6e1728b107";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NeedleExecutionConfig {
    pub api_version: u32,
    pub greedy_decoding_only: bool,
    pub device: String,
    pub dtype: String,
    pub max_decoder_tokens: u32,
    pub max_encoder_tokens: u32,
    pub model_revision: String,
    pub source_commit: String,
    pub torch_version: String,
    pub transformers_version: String,
    pub trusted_file_sha256: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NeedleGenerationMetrics {
    pub completion_tokens: u32,
    pub generation_ms: f64,
    pub output_tokens_per_second: f64,
    pub prefill_tokens_per_second: f64,
    pub prompt_eval_ms: f64,
    pub prompt_tokens: u32,
    pub time_to_first_token_ms: f64,
}

pub struct NeedleBackend {
    python: PathBuf,
    device: String,
    worker: RefCell<Option<NeedleWorker>>,
    loaded: Option<LoadedModelInfo>,
    next_id: Cell<u64>,
    memory_limit_bytes: Cell<Option<u64>>,
    model_root: RefCell<Option<PathBuf>>,
    cache_root: RefCell<Option<PathBuf>>,
    expected_sha256: RefCell<BTreeMap<String, String>>,
    execution_config: RefCell<Option<NeedleExecutionConfig>>,
    last_generation: RefCell<Option<NeedleGenerationMetrics>>,
}

impl NeedleBackend {
    pub fn new() -> Result<Self> {
        let python = env::var_os(PYTHON_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("python3"));
        Self::with_python(python)
    }

    pub fn with_python(python: impl Into<PathBuf>) -> Result<Self> {
        let device = env::var(DEVICE_ENV).unwrap_or_else(|_| "cpu".to_owned());
        Self::with_python_for_device(python, device)
    }

    pub fn with_python_for_device(
        python: impl Into<PathBuf>,
        device: impl AsRef<str>,
    ) -> Result<Self> {
        let device = normalize_device(device.as_ref())?;
        Ok(Self {
            python: python.into(),
            device,
            worker: RefCell::new(None),
            loaded: None,
            next_id: Cell::new(1),
            memory_limit_bytes: Cell::new(None),
            model_root: RefCell::new(None),
            cache_root: RefCell::new(None),
            expected_sha256: RefCell::new(BTreeMap::new()),
            execution_config: RefCell::new(None),
            last_generation: RefCell::new(None),
        })
    }

    fn call<T>(
        &self,
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
        &self,
        operation: &str,
        payload: Value,
        cancellation: Option<&CancellationToken>,
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let id = self.next_id.get();
        self.next_id.set(id.saturating_add(1));
        let mut worker_slot = self.worker.borrow_mut();
        let worker = worker_slot
            .as_mut()
            .ok_or_else(|| EngineError::Needle("Needle worker is not running".to_owned()))?;
        worker.send(id, operation, payload)?;
        loop {
            if cancellation.is_some_and(CancellationToken::is_cancelled) {
                worker.stop();
                *worker_slot = None;
                return Err(EngineError::Cancelled);
            }
            let message = match worker.read_message(Duration::from_millis(25)) {
                Ok(Some(message)) => message,
                Ok(None) => continue,
                Err(error) => {
                    worker.stop();
                    *worker_slot = None;
                    return Err(error);
                }
            };
            if message.id != id {
                worker.stop();
                *worker_slot = None;
                return Err(EngineError::Needle(format!(
                    "worker response id {} did not match request id {id}",
                    message.id
                )));
            }
            if !message.ok {
                return Err(EngineError::Needle(
                    message
                        .error
                        .unwrap_or_else(|| "worker returned an unknown error".to_owned()),
                ));
            }
            let decoded = serde_json::from_value(message.result.unwrap_or(Value::Null));
            return match decoded {
                Ok(result) => Ok(result),
                Err(error) => {
                    worker.stop();
                    let error = EngineError::Needle(format!(
                        "decoding Needle worker response failed: {error}"
                    ));
                    *worker_slot = None;
                    Err(error)
                }
            };
        }
    }

    fn ensure_worker_loaded(&self) -> Result<WorkerLoadInfo> {
        if self.worker.borrow().is_some() {
            let execution_config = self.execution_config.borrow().clone().ok_or_else(|| {
                EngineError::Needle("Needle worker has no load evidence".to_owned())
            })?;
            return Ok(WorkerLoadInfo {
                execution_config,
                n_ctx_train: MAX_ENCODER_TOKENS,
                n_vocab: 8_192,
            });
        }
        let model_root = self
            .model_root
            .borrow()
            .clone()
            .ok_or(EngineError::NotLoaded)?;
        let cache_root = self
            .cache_root
            .borrow()
            .clone()
            .ok_or(EngineError::NotLoaded)?;
        let expected_sha256 = self.expected_sha256.borrow().clone();
        let worker = NeedleWorker::spawn(
            &self.python,
            self.memory_limit_bytes.get(),
            &cache_root,
            &model_root,
            &self.device,
        )?;
        *self.worker.borrow_mut() = Some(worker);
        let info: WorkerLoadInfo = match self.call_existing(
            "load",
            json!({
                "cache_root": cache_root,
                "expected_sha256": expected_sha256,
                "model_root": model_root,
            }),
            None,
        ) {
            Ok(info) => info,
            Err(error) => {
                self.stop_worker();
                return Err(error);
            }
        };
        validate_execution_config(&info.execution_config, &self.device)?;
        *self.execution_config.borrow_mut() = Some(info.execution_config.clone());
        Ok(info)
    }

    fn stop_worker(&self) {
        if let Some(mut worker) = self.worker.borrow_mut().take() {
            worker.stop();
        }
    }
}

impl EngineBackend for NeedleBackend {
    fn backend_id(&self) -> &'static str {
        "needle"
    }

    fn load(&mut self, config: LoadConfig) -> Result<LoadedModelInfo> {
        validate_load_config(&config)?;
        if config.artifact.format != ArtifactFormat::TransformersSafetensors {
            return Err(EngineError::InvalidConfig(format!(
                "Needle requires Transformers safetensors, got {:?}",
                config.artifact.format
            )));
        }
        if config.ctx_size != MAX_ENCODER_TOKENS {
            return Err(EngineError::InvalidConfig(format!(
                "Needle has a fixed native encoder ceiling of {MAX_ENCODER_TOKENS} tokens"
            )));
        }
        let (model_root, expected_sha256) = validate_model_root(&config)?;
        let cache_root = needle_worker_cache(
            config.backend_cache_dir.as_deref(),
            &model_root,
            &self.device,
        )?;

        self.stop_worker();
        self.loaded = None;
        *self.execution_config.borrow_mut() = None;
        *self.last_generation.borrow_mut() = None;
        self.memory_limit_bytes.set(config.memory_limit_bytes);
        *self.model_root.borrow_mut() = Some(model_root);
        *self.cache_root.borrow_mut() = Some(cache_root);
        *self.expected_sha256.borrow_mut() = expected_sha256;
        let info = self.ensure_worker_loaded()?;
        let loaded = LoadedModelInfo {
            backend: self.backend_id().to_owned(),
            artifact: config.artifact,
            ctx_size: MAX_ENCODER_TOKENS,
            n_ctx_train: info.n_ctx_train,
            n_vocab: info.n_vocab,
        };
        self.loaded = Some(loaded.clone());
        Ok(loaded)
    }

    fn loaded_backend_evidence(&self) -> Option<Value> {
        self.execution_config.borrow().as_ref().map(|execution| {
            json!({
                "execution_config": execution,
                "last_generation": self.last_generation.borrow().as_ref(),
            })
        })
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
        self.call("tokenize", json!({ "text": text }), None)
    }

    fn generate(
        &mut self,
        request: GenerateRequest,
        sink: &mut dyn TokenSink,
        cancellation: &CancellationToken,
    ) -> Result<GenerateOutput> {
        cancellation.check()?;
        self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
        validate_needle_request(&request)?;
        let prompt = needle_request_prompt(&request)?;
        let result: WorkerGenerationResult = self.call(
            "generate",
            needle_worker_generation_payload(&request, &prompt),
            Some(cancellation),
        )?;
        result.validate()?;
        let final_index = result.token_ids.len().saturating_sub(1);
        for (index, token_id) in result.token_ids.iter().copied().enumerate() {
            sink.on_token(TokenChunk {
                index: u32::try_from(index).unwrap_or(u32::MAX),
                token_id,
                text: if index == final_index {
                    result.output_text.clone()
                } else {
                    String::new()
                },
            })?;
        }
        let metrics = NeedleGenerationMetrics {
            completion_tokens: result.completion_tokens,
            generation_ms: result.generation_ms,
            output_tokens_per_second: result.output_tokens_per_second,
            prefill_tokens_per_second: result.prefill_tokens_per_second,
            prompt_eval_ms: result.prompt_eval_ms,
            prompt_tokens: result.prompt_tokens,
            time_to_first_token_ms: result.time_to_first_token_ms,
        };
        *self.last_generation.borrow_mut() = Some(metrics);
        Ok(GenerateOutput {
            text: result.output_text,
            usage: UsageCounters::new(result.prompt_tokens, result.completion_tokens),
            finish_reason: match result.finish_reason.as_str() {
                "stop" => FinishReason::Stop,
                "length" => FinishReason::Length,
                other => {
                    return Err(EngineError::Needle(format!(
                        "worker returned unsupported finish reason {other:?}"
                    )))
                }
            },
        })
    }
}

impl Drop for NeedleBackend {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

fn needle_worker_generation_payload(request: &GenerateRequest, prompt: &str) -> Value {
    json!({
        "frequency_penalty": request.frequency_penalty,
        "ignore_eos": request.ignore_eos,
        "max_new_tokens": request.max_new_tokens,
        "min_p": request.min_p,
        "parallel_tool_calls": request.parallel_tool_calls,
        "presence_penalty": request.presence_penalty,
        "prompt": prompt,
        "repeat_penalty": request.repeat_penalty,
        "seed": request.seed,
        "stop": request.stop,
        "temperature": request.temperature,
        "tools": request.tools,
        "top_k": request.top_k,
        "top_p": request.top_p,
    })
}

fn validate_needle_request(request: &GenerateRequest) -> Result<()> {
    request.validate_sampling()?;
    needle_request_prompt(request)?;
    if request.tools.is_empty() {
        return Err(EngineError::InvalidConfig(
            "Needle is tools-only and requires at least one tool".to_owned(),
        ));
    }
    if !request.messages.is_empty()
        && (request.messages.len() != 1
            || request.messages[0].get("role").and_then(Value::as_str) != Some("user"))
    {
        return Err(EngineError::InvalidConfig(
            "Needle supports one single-shot user turn and refuses conversational history"
                .to_owned(),
        ));
    }
    if !request.media.is_empty() {
        return Err(EngineError::InvalidConfig(
            "Needle does not accept media".to_owned(),
        ));
    }
    if request.max_new_tokens == 0 || request.max_new_tokens > MAX_DECODER_TOKENS {
        return Err(EngineError::InvalidConfig(format!(
            "Needle max_new_tokens must be between 1 and {MAX_DECODER_TOKENS}"
        )));
    }
    if request.temperature.is_some_and(|value| value != 0.0)
        || request.top_p.is_some_and(|value| value != 1.0)
        || request.top_k.is_some_and(|value| !matches!(value, 0 | 1))
        || request.min_p.is_some_and(|value| value != 0.0)
        || request.repeat_penalty.is_some_and(|value| value != 1.0)
        || request.frequency_penalty.is_some_and(|value| value != 0.0)
        || request.presence_penalty.is_some_and(|value| value != 0.0)
    {
        return Err(EngineError::InvalidConfig(
            "Needle supports deterministic greedy decoding only".to_owned(),
        ));
    }
    if !request.stop.is_empty() || request.ignore_eos {
        return Err(EngineError::InvalidConfig(
            "Needle uses its pinned EOS and does not support custom stop behavior".to_owned(),
        ));
    }
    if request
        .grammar
        .as_ref()
        .is_some_and(|grammar| !matches!(grammar, GrammarSpec::ToolCall { .. }))
        || !request.speciality_parameters.is_empty()
    {
        return Err(EngineError::InvalidConfig(
            "Needle does not accept external grammar or speciality controls".to_owned(),
        ));
    }
    Ok(())
}

fn needle_request_prompt(request: &GenerateRequest) -> Result<String> {
    if request.messages.is_empty() {
        if request.prompt.trim().is_empty() {
            return Err(EngineError::InvalidConfig(
                "Needle requires a non-empty single-shot query".to_owned(),
            ));
        }
        return Ok(request.prompt.clone());
    }
    if request.messages.len() != 1
        || request.messages[0].get("role").and_then(Value::as_str) != Some("user")
    {
        return Err(EngineError::InvalidConfig(
            "Needle supports one single-shot user turn and refuses conversational history"
                .to_owned(),
        ));
    }
    let content = request.messages[0].get("content").ok_or_else(|| {
        EngineError::InvalidConfig("Needle user message has no content".to_owned())
    })?;
    let prompt = match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) if !parts.is_empty() => {
            let mut text = Vec::with_capacity(parts.len());
            for part in parts {
                if part.get("type").and_then(Value::as_str) != Some("text") {
                    return Err(EngineError::InvalidConfig(
                        "Needle accepts text message parts only".to_owned(),
                    ));
                }
                text.push(
                    part.get("text")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            EngineError::InvalidConfig(
                                "Needle text message part has no text".to_owned(),
                            )
                        })?
                        .to_owned(),
                );
            }
            text.join("\n")
        }
        _ => {
            return Err(EngineError::InvalidConfig(
                "Needle user message content must be text".to_owned(),
            ))
        }
    };
    if prompt.trim().is_empty() {
        return Err(EngineError::InvalidConfig(
            "Needle requires a non-empty single-shot query".to_owned(),
        ));
    }
    Ok(prompt)
}

fn normalize_device(device: &str) -> Result<String> {
    let device = device.trim().to_ascii_lowercase();
    if matches!(device.as_str(), "cpu" | "cuda") {
        Ok(device)
    } else {
        Err(EngineError::InvalidConfig(
            "Needle device must be cpu or cuda".to_owned(),
        ))
    }
}

fn validate_execution_config(config: &NeedleExecutionConfig, expected_device: &str) -> Result<()> {
    if config.api_version != 1
        || !config.greedy_decoding_only
        || config.device != expected_device
        || config.max_encoder_tokens != MAX_ENCODER_TOKENS
        || config.max_decoder_tokens != MAX_DECODER_TOKENS
        || config.source_commit != NEEDLE_SOURCE_COMMIT
        || config.model_revision != NEEDLE_MODEL_REVISION
        || config.trusted_file_sha256.len() != REQUIRED_MODEL_FILES.len()
        || REQUIRED_MODEL_FILES
            .iter()
            .any(|name| !config.trusted_file_sha256.contains_key(*name))
    {
        return Err(EngineError::Needle(
            "worker reported an unsupported Needle runtime".to_owned(),
        ));
    }
    Ok(())
}

fn validate_model_root(config: &LoadConfig) -> Result<(PathBuf, BTreeMap<String, String>)> {
    if !config.artifact.path.exists() {
        return Err(EngineError::ModelPathMissing(config.artifact.path.clone()));
    }
    let root = if config.artifact.path.is_file() {
        config.artifact.path.parent().ok_or_else(|| {
            EngineError::InvalidConfig(format!(
                "Needle weights {} have no parent",
                config.artifact.path.display()
            ))
        })?
    } else {
        config.artifact.path.as_path()
    };
    let root = fs::canonicalize(root).map_err(|error| {
        EngineError::Needle(format!(
            "canonicalizing Needle model root {} failed: {error}",
            root.display()
        ))
    })?;
    let mut hashes = BTreeMap::new();
    for name in REQUIRED_MODEL_FILES {
        let path = root.join(name);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            EngineError::Needle(format!(
                "inspecting required Needle file {} failed: {error}",
                path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(EngineError::Needle(format!(
                "required Needle file {} must be a regular non-symlink file",
                path.display()
            )));
        }
        hashes.insert((*name).to_owned(), sha256_file(&path)?);
    }
    for name in [
        "config.json",
        "special_tokens_map.json",
        "tokenizer_config.json",
    ] {
        let path = root.join(name);
        let value: Value = serde_json::from_reader(File::open(&path)?)?;
        if !value.is_object() {
            return Err(EngineError::InvalidConfig(format!(
                "Needle sidecar {} must be a JSON object",
                path.display()
            )));
        }
    }
    let config_value: Value = serde_json::from_reader(File::open(root.join("config.json"))?)?;
    if config_value.get("model_type").and_then(Value::as_str) != Some("needle")
        || config_value.get("vocab_size").and_then(Value::as_u64) != Some(8_192)
        || config_value
            .get("is_encoder_decoder")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err(EngineError::InvalidConfig(
            "Needle config.json does not describe the pinned architecture".to_owned(),
        ));
    }
    let weights = root.join("model.safetensors");
    if config.artifact.path.is_file() && fs::canonicalize(&config.artifact.path)? != weights {
        return Err(EngineError::InvalidConfig(
            "Needle file artifacts must identify model.safetensors".to_owned(),
        ));
    }
    verify_safetensors_header_as(&weights, "Needle Transformers safetensors")?;
    if config.artifact.sha256_path.is_some() && config.artifact.sha256.is_none() {
        return Err(EngineError::InvalidConfig(
            "artifact sha256_path requires an expected sha256".to_owned(),
        ));
    }
    if let Some(hash_path) = &config.artifact.sha256_path {
        let canonical_hash_path = fs::canonicalize(hash_path)?;
        if canonical_hash_path != weights {
            return Err(EngineError::InvalidConfig(
                "Needle artifact sha256_path must identify model.safetensors".to_owned(),
            ));
        }
    }
    if let Some(expected) = &config.artifact.sha256 {
        let actual = hashes
            .get("model.safetensors")
            .cloned()
            .ok_or_else(|| EngineError::Needle("Needle weights hash is missing".to_owned()))?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(EngineError::ArtifactHashMismatch {
                path: weights,
                expected: expected.clone(),
                actual,
            });
        }
    }
    Ok((root, hashes))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut source = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn needle_worker_cache(
    configured: Option<&Path>,
    model_root: &Path,
    device: &str,
) -> Result<PathBuf> {
    let cache_root = configured
        .map(Path::to_path_buf)
        .or_else(|| {
            env::var_os("MAYHEM_HOME")
                .map(PathBuf::from)
                .map(|home| home.join("cache/needle"))
        })
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".mayhem/cache/needle"))
        })
        .unwrap_or_else(|| env::temp_dir().join("mayhem-needle-cache"));
    let digest = Sha256::digest(format!("{}\0{device}", model_root.to_string_lossy()).as_bytes());
    let worker = cache_root.join("workers").join(format!("{digest:x}"));
    fs::create_dir_all(&worker).map_err(|error| {
        EngineError::Needle(format!(
            "creating Needle worker cache {} failed: {error}",
            worker.display()
        ))
    })?;
    fs::canonicalize(&worker).map_err(|error| {
        EngineError::Needle(format!(
            "canonicalizing Needle worker cache {} failed: {error}",
            worker.display()
        ))
    })
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerLoadInfo {
    execution_config: NeedleExecutionConfig,
    n_ctx_train: u32,
    n_vocab: i32,
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
struct WorkerGenerationResult {
    completion_tokens: u32,
    finish_reason: String,
    generation_ms: f64,
    output_text: String,
    output_tokens_per_second: f64,
    prefill_tokens_per_second: f64,
    prompt_eval_ms: f64,
    prompt_tokens: u32,
    token_ids: Vec<i32>,
    time_to_first_token_ms: f64,
}

impl WorkerGenerationResult {
    fn validate(&self) -> Result<()> {
        if self.completion_tokens > MAX_DECODER_TOKENS
            || self.token_ids.len() != self.completion_tokens as usize
            || self
                .token_ids
                .iter()
                .any(|token| !(0..8_192).contains(token))
            || self.prompt_tokens > MAX_ENCODER_TOKENS
            || self.output_text.len() > MAX_WORKER_RESPONSE_LINE_BYTES
            || [
                self.generation_ms,
                self.output_tokens_per_second,
                self.prefill_tokens_per_second,
                self.prompt_eval_ms,
                self.time_to_first_token_ms,
            ]
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(EngineError::Needle(
                "worker returned invalid bounded generation metrics".to_owned(),
            ));
        }
        Ok(())
    }
}

struct NeedleWorker {
    child: SandboxedChild,
    stdin: SandboxedChildStdin,
    stdout_rx: Option<Receiver<WorkerRead>>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    stderr_reader: Option<JoinHandle<()>>,
}

impl NeedleWorker {
    fn spawn(
        python: &Path,
        memory_limit_bytes: Option<u64>,
        cache_root: &Path,
        model_root: &Path,
        device: &str,
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
        configure_worker_environment(&mut command, &python, cache_root, device)?;
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
            EngineError::Needle(format!(
                "starting sandboxed Needle worker with {} failed: {error}",
                python.display()
            ))
        })?;
        let mut stdin = child
            .take_stdin()
            .ok_or_else(|| EngineError::Needle("opening Needle worker stdin failed".to_owned()))?;
        let worker_source = base64::engine::general_purpose::STANDARD.encode(WORKER.as_bytes());
        stdin
            .write_all(worker_source.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .map_err(|error| {
                EngineError::Needle(format!(
                    "sending Needle worker source over the bootstrap pipe failed: {error}"
                ))
            })?;
        let stdout = child
            .take_stdout()
            .ok_or_else(|| EngineError::Needle("opening Needle worker stdout failed".to_owned()))?;
        let stderr = child
            .take_stderr()
            .ok_or_else(|| EngineError::Needle("opening Needle worker stderr failed".to_owned()))?;
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
            return Err(EngineError::Needle(
                "Needle worker request exceeds the protocol bound".to_owned(),
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
            .ok_or_else(|| EngineError::Needle("Needle stdout reader is closed".to_owned()))?
            .recv_timeout(wait)
        {
            Ok(read) => read,
            Err(RecvTimeoutError::Timeout) => return Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                return Err(EngineError::Needle(
                    "Needle stdout reader stopped".to_owned(),
                ))
            }
        };
        let line = match read {
            WorkerRead::Line(line) => line,
            WorkerRead::Eof => return Err(self.exit_error("Needle worker exited before replying")),
            WorkerRead::Error(error) => return Err(EngineError::Needle(error)),
        };
        serde_json::from_str(line.trim_end())
            .map(Some)
            .map_err(|error| {
                EngineError::Needle(format!(
                    "decoding Needle worker protocol line failed: {error}"
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
            EngineError::Needle(format!("{message}; exit status {status}; stderr was empty"))
        } else {
            EngineError::Needle(format!(
                "{message}; exit status {status}; stderr tail: {stderr}"
            ))
        }
    }
}

fn configure_worker_environment(
    command: &mut SandboxedCommand,
    python: &Path,
    cache_root: &Path,
    device: &str,
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
    ] {
        fs::create_dir_all(&path).map_err(|error| {
            EngineError::Needle(format!(
                "creating Needle cache directory {} failed: {error}",
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
        .env("DO_NOT_TRACK", "1")
        .env("PIP_NO_INDEX", "1")
        .env("UV_OFFLINE", "1")
        .env("TOKENIZERS_PARALLELISM", "false")
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env(DEVICE_ENV, device);
    if device == "cuda" {
        command.env("CUBLAS_WORKSPACE_CONFIG", ":4096:8");
    }
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
    if let Some(python_bin) = python.parent() {
        let mut paths = vec![python_bin.to_path_buf()];
        #[cfg(unix)]
        paths.extend([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
        #[cfg(windows)]
        if let Some(system_root) = env::var_os("SystemRoot") {
            paths.push(PathBuf::from(system_root).join("System32"));
        }
        let path = env::join_paths(paths).map_err(|error| {
            EngineError::Needle(format!("building Needle PATH failed: {error}"))
        })?;
        command.env("PATH", path);
    }
    Ok(())
}

fn python_runtime_roots(python: &Path) -> Result<(PathBuf, Vec<PathBuf>)> {
    let python = resolve_python_program(python)?;
    let canonical_python = fs::canonicalize(&python).map_err(|error| {
        EngineError::Needle(format!(
            "canonicalizing Needle Python {} failed: {error}",
            python.display()
        ))
    })?;
    let executable_root = canonical_python.parent().ok_or_else(|| {
        EngineError::Needle(format!(
            "Needle Python {} has no runtime directory",
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
            EngineError::Needle(format!("canonicalizing Needle runtime failed: {error}"))
        })?;
        insert_non_overlapping_root(&mut roots, candidate);
    }
    if roots.is_empty() {
        return Err(EngineError::Needle(
            "Needle Python has no readable runtime roots".to_owned(),
        ));
    }
    Ok((python, roots))
}

fn resolve_python_program(python: &Path) -> Result<PathBuf> {
    let candidates = if python.is_absolute() || python.components().count() > 1 {
        vec![if python.is_absolute() {
            python.to_path_buf()
        } else {
            env::current_dir()?.join(python)
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
            EngineError::Needle(format!("Needle Python {} was not found", python.display()))
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
                    "reading Needle worker stdout failed: {error}"
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
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool() -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "lookup",
                "parameters": {
                    "type": "object",
                    "properties": {"id": {"type": "integer"}},
                    "required": ["id"],
                    "additionalProperties": false
                }
            }
        })
    }

    #[test]
    fn explicit_devices_are_validated_without_ambient_state() {
        for device in ["cpu", "cuda"] {
            let backend =
                NeedleBackend::with_python_for_device("python3", device).expect("valid device");
            assert_eq!(backend.device, device);
        }
        assert!(NeedleBackend::with_python_for_device("python3", "mps").is_err());
        assert!(NeedleBackend::with_python_for_device("python3", "auto").is_err());
    }

    #[test]
    fn ordinary_chat_and_non_greedy_sampling_are_rejected() {
        let chat = GenerateRequest::new("hello");
        assert!(validate_needle_request(&chat)
            .unwrap_err()
            .to_string()
            .contains("tools-only"));

        let mut sampled = GenerateRequest::new("look up one");
        sampled.tools = vec![tool()];
        sampled.temperature = Some(0.5);
        assert!(validate_needle_request(&sampled)
            .unwrap_err()
            .to_string()
            .contains("greedy"));
    }

    #[test]
    fn greedy_tool_request_is_accepted_up_to_decoder_ceiling() {
        let mut request = GenerateRequest::new("look up one");
        request.tools = vec![tool()];
        request.max_new_tokens = MAX_DECODER_TOKENS;
        request.temperature = Some(0.0);
        request.top_p = Some(1.0);
        request.top_k = Some(1);
        assert!(validate_needle_request(&request).is_ok());
        request.grammar = Some(GrammarSpec::ToolCall {
            tools: vec![super::super::ToolSpec::new(
                "lookup",
                json!({"type": "object"}),
            )],
        });
        assert!(validate_needle_request(&request).is_ok());
        request.max_new_tokens = MAX_DECODER_TOKENS + 1;
        assert!(validate_needle_request(&request).is_err());

        request.max_new_tokens = MAX_DECODER_TOKENS;
        request.messages = vec![
            json!({"role": "user", "content": "look up one"}),
            json!({"role": "assistant", "content": "history"}),
        ];
        assert!(validate_needle_request(&request)
            .unwrap_err()
            .to_string()
            .contains("single-shot"));
    }

    #[test]
    fn parallel_tool_call_preference_is_carried_to_the_worker_payload() {
        let mut request = GenerateRequest::new("look up one");
        request.tools = vec![tool()];
        assert_eq!(
            needle_worker_generation_payload(&request, "look up one")["parallel_tool_calls"],
            Value::Null
        );
        request.parallel_tool_calls = Some(false);
        assert_eq!(
            needle_worker_generation_payload(&request, "look up one")["parallel_tool_calls"],
            json!(false)
        );
    }

    #[test]
    fn endpoint_messages_override_rendered_prompt_with_native_query() {
        let mut request = GenerateRequest::new("<|im_start|>user\nlook up one<|im_end|>");
        request.tools = vec![tool()];
        request.messages = vec![json!({"role": "user", "content": "look up one"})];
        assert_eq!(
            needle_request_prompt(&request).expect("native query"),
            "look up one"
        );

        request.messages = vec![json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "look up"},
                {"type": "text", "text": "one"}
            ]
        })];
        assert_eq!(
            needle_request_prompt(&request).expect("text-part query"),
            "look up\none"
        );

        request.messages = vec![json!({"role": "user", "content": [{"type": "image_url"}]})];
        assert!(needle_request_prompt(&request).is_err());
    }

    #[test]
    fn worker_protocol_reader_is_bounded() {
        let mut valid = std::io::Cursor::new(b"{\"ok\":true}\n".to_vec());
        assert_eq!(
            read_bounded_line(&mut valid, 32).expect("line"),
            Some("{\"ok\":true}\n".to_owned())
        );
        let mut oversized = std::io::Cursor::new(vec![b'x'; 33]);
        assert!(read_bounded_line(&mut oversized, 32).is_err());
    }
}
