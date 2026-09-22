use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{
    verify_artifact, CancellationToken, ConcurrentGenerationBackend, EngineBackend, EngineError,
    FinishReason, GenerateOutput, GenerateRequest, GenerateSpecialityTarget, GrammarSpec,
    LoadConfig, LoadedModelInfo, Result, TokenChunk, TokenSink, Tokenization, ToolSpec,
    UsageCounters,
};

const BACKEND_ID: &str = "openai-compatible";
const REQUEST_POLL_INTERVAL: Duration = Duration::from_millis(10);
const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(45);
const ALLOWED_CAPABILITIES: &[&str] = &[
    "cancellation",
    "image",
    "json",
    "prefix_cache",
    "reasoning",
    "streaming",
    "tools",
    "video",
];

/// Runtime and serving identity committed by the signed catalog artifact.
/// The operator supplies the loopback origin separately; network topology is
/// deliberately not catalog data.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenAiCompatibleRuntimeBinding {
    pub schema_version: u32,
    pub lifecycle: OpenAiCompatibleLifecycle,
    pub runtime_id: String,
    pub runtime_version: String,
    pub runtime_revision: String,
    pub implementation: String,
    pub implementation_version: String,
    pub container_image_digest: String,
    pub served_model: String,
    pub native_context: u32,
    pub served_context: u32,
    pub max_concurrent: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibrated_peak_gpu_memory_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibrated_peak_host_memory_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_measurement_source: Option<String>,
    pub model_snapshot_file_count: u32,
    pub snapshot_manifest_sidecar: String,
    pub snapshot_manifest_sha256: String,
    pub runtime_recipe_sidecar: String,
    pub runtime_recipe_sha256: String,
    pub capabilities: BTreeSet<String>,
    pub server_info_checks: BTreeMap<String, Value>,
    pub preflight: OpenAiCompatiblePreflightProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_cache_metric: Option<String>,
}

/// Signed request controls and budgets used to prove live capabilities.  These
/// are catalog data because a model's default reasoning mode can consume a
/// short probe before it emits text, JSON, or a tool call.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenAiCompatiblePreflightProfile {
    pub non_reasoning_chat_template_kwargs: BTreeMap<String, Value>,
    pub reasoning_chat_template_kwargs: BTreeMap<String, Value>,
    pub streaming_max_tokens: u32,
    pub tools_max_tokens: u32,
    pub json_max_tokens: u32,
    pub reasoning_max_tokens: u32,
    pub cache_max_tokens: u32,
    pub cancellation_max_tokens: u32,
    pub concurrency_max_tokens: u32,
    pub cancellation_idle_metrics: BTreeSet<String>,
    pub concurrency_active_metric: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiCompatibleLifecycle {
    ManagedOrVerifiedAttach,
}

impl OpenAiCompatibleRuntimeBinding {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return Err(invalid("runtime binding schema_version must be 1"));
        }
        for (name, value) in [
            ("runtime_id", self.runtime_id.as_str()),
            ("runtime_version", self.runtime_version.as_str()),
            ("implementation", self.implementation.as_str()),
            (
                "implementation_version",
                self.implementation_version.as_str(),
            ),
        ] {
            if !safe_identity(value) {
                return Err(invalid(format!(
                    "runtime binding {name} must be a safe non-empty identifier"
                )));
            }
        }
        if !matches!(self.runtime_revision.len(), 40 | 64)
            || !self
                .runtime_revision
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.runtime_revision != self.runtime_revision.to_ascii_lowercase()
        {
            return Err(invalid(
                "runtime binding runtime_revision must be exact lowercase 20- or 32-byte hex",
            ));
        }
        let Some(container_digest) = self.container_image_digest.strip_prefix("sha256:") else {
            return Err(invalid(
                "runtime binding container_image_digest must use sha256:<lowercase-hex>",
            ));
        };
        if container_digest.len() != 64
            || !container_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || container_digest != container_digest.to_ascii_lowercase()
        {
            return Err(invalid(
                "runtime binding container_image_digest must be exact lowercase SHA-256",
            ));
        }
        if self.served_model.trim().is_empty()
            || self.served_model.len() > 256
            || self.served_model.chars().any(char::is_control)
        {
            return Err(invalid(
                "runtime binding served_model must be a non-empty printable value of at most 256 bytes",
            ));
        }
        if self.native_context == 0 || self.served_context < self.native_context {
            return Err(invalid(
                "runtime binding contexts must be positive and served_context must cover native_context",
            ));
        }
        if !(1..=64).contains(&self.max_concurrent) {
            return Err(invalid(
                "runtime binding max_concurrent must be between 1 and 64",
            ));
        }
        match (
            self.calibrated_peak_gpu_memory_bytes,
            self.calibrated_peak_host_memory_bytes,
            self.memory_measurement_source.as_deref(),
        ) {
            (None, None, None) => {}
            (Some(gpu), Some(host), Some(source))
                if gpu > 0 && host > 0 && !source.trim().is_empty() && source.len() <= 1024 => {}
            _ => {
                return Err(invalid(
                    "runtime binding calibrated GPU/host peak bytes and memory measurement source must be supplied together",
                ));
            }
        }
        if self.model_snapshot_file_count == 0 {
            return Err(invalid(
                "runtime binding model_snapshot_file_count must be positive",
            ));
        }
        for (label, sidecar) in [
            (
                "snapshot_manifest_sidecar",
                self.snapshot_manifest_sidecar.as_str(),
            ),
            (
                "runtime_recipe_sidecar",
                self.runtime_recipe_sidecar.as_str(),
            ),
        ] {
            if !safe_identity(sidecar) {
                return Err(invalid(format!(
                    "runtime binding {label} must be a safe sidecar name"
                )));
            }
        }
        if self.snapshot_manifest_sidecar == self.runtime_recipe_sidecar {
            return Err(invalid(
                "runtime binding snapshot and recipe sidecars must be distinct",
            ));
        }
        for (label, digest) in [
            (
                "snapshot_manifest_sha256",
                self.snapshot_manifest_sha256.as_str(),
            ),
            ("runtime_recipe_sha256", self.runtime_recipe_sha256.as_str()),
        ] {
            if digest.len() != 64
                || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                || digest != digest.to_ascii_lowercase()
            {
                return Err(invalid(format!(
                    "runtime binding {label} must be exact lowercase SHA-256"
                )));
            }
        }
        if !self.capabilities.contains("streaming") {
            return Err(invalid(
                "runtime binding capabilities must include streaming",
            ));
        }
        for capability in &self.capabilities {
            if !ALLOWED_CAPABILITIES.contains(&capability.as_str()) {
                return Err(invalid(format!(
                    "runtime binding contains unsupported capability {capability}"
                )));
            }
        }
        if self.server_info_checks.is_empty() || self.server_info_checks.len() > 32 {
            return Err(invalid(
                "runtime binding server_info_checks must contain 1 to 32 exact checks",
            ));
        }
        for (pointer, expected) in &self.server_info_checks {
            if !pointer.starts_with('/')
                || pointer.len() > 256
                || matches!(expected, Value::Array(_) | Value::Object(_))
            {
                return Err(invalid(format!(
                    "runtime binding server_info check {pointer:?} must be a JSON pointer to a scalar"
                )));
            }
        }
        self.preflight
            .validate(self.capabilities.contains("cancellation"))?;
        if self.capabilities.contains("prefix_cache") {
            let metric = self
                .prefix_cache_metric
                .as_deref()
                .ok_or_else(|| invalid("prefix_cache capability requires prefix_cache_metric"))?;
            if !safe_metric_name(metric) {
                return Err(invalid(
                    "runtime binding prefix_cache_metric must be a safe Prometheus metric name",
                ));
            }
        } else if self.prefix_cache_metric.is_some() {
            return Err(invalid(
                "runtime binding prefix_cache_metric requires prefix_cache capability",
            ));
        }
        Ok(())
    }
}

impl OpenAiCompatiblePreflightProfile {
    fn validate(&self, cancellation_claimed: bool) -> Result<()> {
        for (label, controls) in [
            (
                "non_reasoning_chat_template_kwargs",
                &self.non_reasoning_chat_template_kwargs,
            ),
            (
                "reasoning_chat_template_kwargs",
                &self.reasoning_chat_template_kwargs,
            ),
        ] {
            if controls.len() > 16 {
                return Err(invalid(format!("preflight {label} has too many controls")));
            }
            for (key, value) in controls {
                if !safe_identity(key) || matches!(value, Value::Array(_) | Value::Object(_)) {
                    return Err(invalid(format!(
                        "preflight {label}.{key} must be a scalar chat-template control"
                    )));
                }
            }
        }
        for (label, budget) in [
            ("streaming_max_tokens", self.streaming_max_tokens),
            ("tools_max_tokens", self.tools_max_tokens),
            ("json_max_tokens", self.json_max_tokens),
            ("reasoning_max_tokens", self.reasoning_max_tokens),
            ("cache_max_tokens", self.cache_max_tokens),
            ("cancellation_max_tokens", self.cancellation_max_tokens),
            ("concurrency_max_tokens", self.concurrency_max_tokens),
        ] {
            if !(1..=16_384).contains(&budget) {
                return Err(invalid(format!(
                    "preflight {label} must be between 1 and 16384"
                )));
            }
        }
        if cancellation_claimed && self.cancellation_idle_metrics.is_empty() {
            return Err(invalid(
                "cancellation capability requires signed cancellation_idle_metrics",
            ));
        }
        for metric in &self.cancellation_idle_metrics {
            if !safe_metric_name(metric) {
                return Err(invalid(format!(
                    "preflight cancellation idle metric {metric:?} is invalid"
                )));
            }
        }
        if !safe_metric_name(&self.concurrency_active_metric) {
            return Err(invalid(
                "preflight concurrency_active_metric must be a safe Prometheus metric name",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct OpenAiCompatibleBackendConfig {
    pub base_url: String,
    pub runtime: OpenAiCompatibleRuntimeBinding,
    pub readiness_timeout: Duration,
}

#[derive(Debug)]
struct GenerationGate {
    capacity: usize,
    active: Mutex<usize>,
    changed: Condvar,
}

impl GenerationGate {
    fn acquire(self: &Arc<Self>, cancellation: &CancellationToken) -> Result<GenerationPermit> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| backend_error("generation gate lock poisoned"))?;
        loop {
            cancellation.check()?;
            if *active < self.capacity {
                *active += 1;
                return Ok(GenerationPermit {
                    gate: Arc::clone(self),
                });
            }
            let (next, _) = self
                .changed
                .wait_timeout(active, Duration::from_millis(25))
                .map_err(|_| backend_error("generation gate lock poisoned"))?;
            active = next;
        }
    }
}

struct GenerationPermit {
    gate: Arc<GenerationGate>,
}

impl Drop for GenerationPermit {
    fn drop(&mut self) {
        if let Ok(mut active) = self.gate.active.lock() {
            *active = active.saturating_sub(1);
            self.gate.changed.notify_one();
        }
    }
}

#[derive(Clone)]
struct LoadedBackend {
    client: Client,
    base_url: Url,
    runtime: OpenAiCompatibleRuntimeBinding,
    artifact: crate::ModelArtifact,
    ctx_size: u32,
    proven_capabilities: BTreeSet<String>,
    gate: Arc<GenerationGate>,
}

pub struct OpenAiCompatibleBackend {
    config: OpenAiCompatibleBackendConfig,
    loaded: Option<Arc<LoadedBackend>>,
}

impl OpenAiCompatibleBackend {
    pub fn new(config: OpenAiCompatibleBackendConfig) -> Result<Self> {
        config.runtime.validate()?;
        validate_loopback_base_url(&config.base_url)?;
        Ok(Self {
            config,
            loaded: None,
        })
    }

    fn loaded(&self) -> Result<Arc<LoadedBackend>> {
        self.loaded.clone().ok_or(EngineError::NotLoaded)
    }
}

impl EngineBackend for OpenAiCompatibleBackend {
    fn backend_id(&self) -> &'static str {
        BACKEND_ID
    }

    fn load(&mut self, config: LoadConfig) -> Result<LoadedModelInfo> {
        self.config.runtime.validate()?;
        if config.ctx_size == 0 || config.ctx_size > self.config.runtime.native_context {
            return Err(invalid(format!(
                "requested ctx_size {} exceeds signed native context {}",
                config.ctx_size, self.config.runtime.native_context
            )));
        }
        verify_artifact(&config.artifact)?;
        let base_url = validate_loopback_base_url(&self.config.base_url)?;
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(map_http_error)?;
        let loaded = Arc::new(LoadedBackend {
            client,
            base_url,
            runtime: self.config.runtime.clone(),
            artifact: config.artifact.clone(),
            ctx_size: config.ctx_size,
            proven_capabilities: BTreeSet::new(),
            gate: Arc::new(GenerationGate {
                capacity: usize::try_from(self.config.runtime.max_concurrent)
                    .map_err(|_| invalid("max_concurrent does not fit usize"))?,
                active: Mutex::new(0),
                changed: Condvar::new(),
            }),
        });
        let deadline = std::time::Instant::now() + self.config.readiness_timeout;
        loop {
            match verify_live_identity(&loaded) {
                Ok(()) => break,
                Err(error) if std::time::Instant::now() < deadline => {
                    thread::sleep(Duration::from_secs(2));
                    if std::time::Instant::now() >= deadline {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        let proven_capabilities = preflight_capabilities(&loaded)?;
        let mut admitted = Arc::unwrap_or_clone(loaded);
        admitted.proven_capabilities = proven_capabilities;
        let admitted = Arc::new(admitted);
        self.loaded = Some(Arc::clone(&admitted));
        Ok(LoadedModelInfo {
            backend: BACKEND_ID.to_owned(),
            artifact: config.artifact,
            ctx_size: config.ctx_size,
            n_ctx_train: self.config.runtime.native_context,
            n_vocab: 0,
        })
    }

    fn prefix_caching_enabled(&self) -> bool {
        self.loaded
            .as_ref()
            .is_some_and(|loaded| loaded.proven_capabilities.contains("prefix_cache"))
    }

    fn loaded_backend_evidence(&self) -> Option<Value> {
        let loaded = self.loaded.as_ref()?;
        Some(json!({
            "schema_version": 1,
            "engine": BACKEND_ID,
            "lifecycle": loaded.runtime.lifecycle,
            "runtime_id": loaded.runtime.runtime_id,
            "runtime_version": loaded.runtime.runtime_version,
            "runtime_revision": loaded.runtime.runtime_revision,
            "implementation": loaded.runtime.implementation,
            "implementation_version": loaded.runtime.implementation_version,
            "container_image_digest": loaded.runtime.container_image_digest,
            "snapshot_manifest_sha256": loaded.runtime.snapshot_manifest_sha256,
            "runtime_recipe_sha256": loaded.runtime.runtime_recipe_sha256,
            "served_model": loaded.runtime.served_model,
            "native_context": loaded.runtime.native_context,
            "served_context": loaded.runtime.served_context,
            "max_concurrent": loaded.runtime.max_concurrent,
            "ctx_size": loaded.ctx_size,
            "artifact_format": loaded.artifact.format,
            "proven_capabilities": loaded.proven_capabilities,
        }))
    }

    fn component_healthy(&mut self) -> bool {
        self.loaded
            .as_ref()
            .is_some_and(|loaded| verify_live_identity(loaded).is_ok())
    }

    fn concurrent_generation_backend(&self) -> Option<Arc<dyn ConcurrentGenerationBackend>> {
        self.loaded.as_ref().map(|loaded| {
            Arc::new(OpenAiCompatibleConcurrent {
                loaded: Arc::clone(loaded),
            }) as Arc<dyn ConcurrentGenerationBackend>
        })
    }

    fn tokenize(&self, text: &str) -> Result<Tokenization> {
        tokenize(self.loaded()?, text)
    }

    fn generate(
        &mut self,
        request: GenerateRequest,
        sink: &mut dyn TokenSink,
        cancellation: &CancellationToken,
    ) -> Result<GenerateOutput> {
        generate(self.loaded()?, request, sink, cancellation)
    }
}

struct OpenAiCompatibleConcurrent {
    loaded: Arc<LoadedBackend>,
}

impl ConcurrentGenerationBackend for OpenAiCompatibleConcurrent {
    fn capacity(&self) -> usize {
        self.loaded.gate.capacity
    }

    fn tokenize(&self, text: &str) -> Result<Tokenization> {
        let tokenization = tokenize(Arc::clone(&self.loaded), text)?;
        if !text.is_empty() && tokenization.is_empty() {
            return Err(backend_error(
                "/v1/tokenize returned no tokens for non-empty input",
            ));
        }
        Ok(tokenization)
    }

    fn generate(
        &self,
        request: GenerateRequest,
        sink: &mut dyn TokenSink,
        cancellation: &CancellationToken,
    ) -> Result<GenerateOutput> {
        generate(Arc::clone(&self.loaded), request, sink, cancellation)
    }
}

fn verify_live_identity(loaded: &LoadedBackend) -> Result<()> {
    verify_models_identity(loaded)?;
    let server_info = get_json(loaded, "server_info")?;
    for (pointer, expected) in &loaded.runtime.server_info_checks {
        let actual = server_info.pointer(pointer).ok_or_else(|| {
            backend_error(format!(
                "/server_info omitted signed identity field {pointer}"
            ))
        })?;
        if actual != expected {
            return Err(backend_error(format!(
                "/server_info identity mismatch at {pointer}: expected {expected}, got {actual}"
            )));
        }
    }
    Ok(())
}

fn verify_models_identity(loaded: &LoadedBackend) -> Result<()> {
    let models = get_json(loaded, "v1/models")?;
    let entries = models
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| backend_error("/v1/models response is missing data"))?;
    let matches = entries
        .iter()
        .filter(|entry| {
            entry.get("id").and_then(Value::as_str) == Some(loaded.runtime.served_model.as_str())
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(backend_error(format!(
            "/v1/models must contain the exact signed model ID {:?} once",
            loaded.runtime.served_model
        )));
    }
    let max_model_len = matches[0]
        .get("max_model_len")
        .and_then(Value::as_u64)
        .ok_or_else(|| backend_error("/v1/models model is missing max_model_len"))?;
    if max_model_len != u64::from(loaded.runtime.served_context) {
        return Err(backend_error(format!(
            "/v1/models max_model_len {max_model_len} differs from signed served_context {}",
            loaded.runtime.served_context
        )));
    }
    Ok(())
}

fn preflight_capabilities(loaded: &Arc<LoadedBackend>) -> Result<BTreeSet<String>> {
    let mut proven = BTreeSet::new();
    let profile = &loaded.runtime.preflight;
    let output = generate(
        Arc::clone(loaded),
        preflight_request(
            "Reply with the single word OK.",
            profile.streaming_max_tokens,
            &profile.non_reasoning_chat_template_kwargs,
        ),
        &mut crate::NoopTokenSink,
        &CancellationToken::new(),
    )?;
    if output.text.trim().is_empty() {
        return Err(backend_error("streaming preflight returned no text"));
    }
    proven.insert("streaming".to_owned());

    if loaded.runtime.capabilities.contains("tools") {
        let mut request = preflight_request(
            "Call the probe tool with ok=true.",
            profile.tools_max_tokens,
            &profile.non_reasoning_chat_template_kwargs,
        );
        request.grammar = Some(GrammarSpec::ToolCall {
            tools: vec![ToolSpec {
                name: "mayhem_runtime_probe".to_owned(),
                description: Some("Report runtime probe success".to_owned()),
                parameters: json!({
                    "type": "object",
                    "properties": {"ok": {"type": "boolean"}},
                    "required": ["ok"],
                    "additionalProperties": false,
                }),
                strict: true,
            }],
        });
        let output = generate(
            Arc::clone(loaded),
            request,
            &mut crate::NoopTokenSink,
            &CancellationToken::new(),
        )?;
        let calls = serde_json::from_str::<Value>(&output.text)
            .ok()
            .and_then(|value| value.get("tool_calls").cloned())
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        if calls.is_empty() {
            return Err(backend_error(
                "tools capability preflight returned no tool call",
            ));
        }
        proven.insert("tools".to_owned());
    }

    if loaded.runtime.capabilities.contains("json") {
        let mut request = preflight_request(
            "Return a JSON object whose ok field is true.",
            profile.json_max_tokens,
            &profile.non_reasoning_chat_template_kwargs,
        );
        request.grammar = Some(GrammarSpec::JsonSchema {
            schema: json!({
                "type": "object",
                "properties": {"ok": {"type": "boolean"}},
                "required": ["ok"],
                "additionalProperties": false,
            }),
        });
        let output = generate(
            Arc::clone(loaded),
            request,
            &mut crate::NoopTokenSink,
            &CancellationToken::new(),
        )?;
        serde_json::from_str::<Value>(output.text.trim())
            .map_err(|error| backend_error(format!("JSON capability preflight failed: {error}")))?;
        proven.insert("json".to_owned());
    }

    if loaded.runtime.capabilities.contains("reasoning") {
        let request = preflight_request(
            "Think carefully, then answer 1+1.",
            profile.reasoning_max_tokens,
            &profile.reasoning_chat_template_kwargs,
        );
        let output = generate(
            Arc::clone(loaded),
            request,
            &mut crate::NoopTokenSink,
            &CancellationToken::new(),
        )?;
        if !output.text.contains("<think>") || !output.text.contains("</think>") {
            return Err(backend_error(
                "reasoning capability preflight returned no reasoning_content",
            ));
        }
        proven.insert("reasoning".to_owned());
    }

    if loaded.runtime.capabilities.contains("prefix_cache") {
        verify_prefix_cache(loaded)?;
        proven.insert("prefix_cache".to_owned());
    }

    if loaded.runtime.capabilities.contains("cancellation") {
        verify_cancellation(loaded)?;
        proven.insert("cancellation".to_owned());
    }

    if loaded.runtime.max_concurrent > 1 {
        verify_concurrency(loaded)?;
        proven.insert("concurrency".to_owned());
    }

    // Media is proven later with the signed catalog canaries, before the first
    // provider heartbeat. Preserve the claim only after those requests pass.
    for capability in ["image", "video"] {
        if loaded.runtime.capabilities.contains(capability) {
            proven.insert(format!("{capability}_pending_catalog_canary"));
        }
    }
    Ok(proven)
}

fn preflight_request(
    prompt: impl Into<String>,
    max_tokens: u32,
    chat_template_kwargs: &BTreeMap<String, Value>,
) -> GenerateRequest {
    let mut request = GenerateRequest::new(prompt).with_max_new_tokens(max_tokens);
    request
        .speciality_parameters
        .extend(chat_template_kwargs.iter().map(|(native_path, value)| {
            crate::GenerateSpecialityParameter {
                name: native_path.clone(),
                level: "signed".to_owned(),
                target: GenerateSpecialityTarget::ChatTemplateKwarg,
                native_path: native_path.clone(),
                value: value.clone(),
                max_reasoning_tokens: None,
            }
        }));
    request
}

fn verify_prefix_cache(loaded: &Arc<LoadedBackend>) -> Result<()> {
    let metric = loaded
        .runtime
        .prefix_cache_metric
        .as_deref()
        .ok_or_else(|| invalid("prefix cache preflight has no signed metric"))?;
    let prefix = "Mayhem prefix cache live verification sequence. ".repeat(1_500);
    let prompt = format!("{prefix}\nReply with CACHE_OK.");
    let request = preflight_request(
        prompt,
        loaded.runtime.preflight.cache_max_tokens,
        &loaded.runtime.preflight.non_reasoning_chat_template_kwargs,
    );
    generate(
        Arc::clone(loaded),
        request.clone(),
        &mut crate::NoopTokenSink,
        &CancellationToken::new(),
    )?;
    let between = get_text(loaded, "metrics")?;
    generate(
        Arc::clone(loaded),
        request,
        &mut crate::NoopTokenSink,
        &CancellationToken::new(),
    )?;
    let after = get_text(loaded, "metrics")?;
    verify_prefix_cache_metric_increase(metric, &between, &after)
}

fn verify_prefix_cache_metric_increase(
    metric: &str,
    between_metrics: &str,
    after_metrics: &str,
) -> Result<()> {
    // SGLang creates the labeled counter only after the first cache hit. The
    // warm request can therefore leave the signed series absent; that is an
    // exact zero baseline. The replay must materialize the same signed series
    // and increase it, so absence after replay remains a hard failure.
    let between = prometheus_metric_sum_optional(between_metrics, metric)?.unwrap_or(0.0);
    let after = prometheus_metric_sum(after_metrics, metric)?;
    if after <= between {
        return Err(backend_error(format!(
            "prefix cache metric {metric} did not increase during repeated-prefix preflight"
        )));
    }
    Ok(())
}

fn verify_cancellation(loaded: &Arc<LoadedBackend>) -> Result<()> {
    let cancellation = CancellationToken::new();
    let sink_cancellation = cancellation.clone();
    let mut saw_token = false;
    let result = generate(
        Arc::clone(loaded),
        {
            let mut request = preflight_request(
                "Write a long technical explanation of distributed consensus.",
                loaded.runtime.preflight.cancellation_max_tokens,
                &loaded.runtime.preflight.non_reasoning_chat_template_kwargs,
            );
            request.ignore_eos = true;
            request
        },
        &mut |chunk: TokenChunk| {
            if !chunk.text.is_empty() {
                saw_token = true;
                sink_cancellation.cancel();
            }
            Ok(())
        },
        &cancellation,
    );
    if !saw_token || !matches!(result, Err(EngineError::Cancelled)) {
        return Err(backend_error(
            "cancellation capability preflight did not abort an active stream",
        ));
    }
    wait_for_scheduler_idle(loaded)?;
    let recovered = generate(
        Arc::clone(loaded),
        preflight_request(
            "Reply with RECOVERED.",
            loaded.runtime.preflight.streaming_max_tokens,
            &loaded.runtime.preflight.non_reasoning_chat_template_kwargs,
        ),
        &mut crate::NoopTokenSink,
        &CancellationToken::new(),
    )?;
    if recovered.text.trim().is_empty() {
        return Err(backend_error(
            "runtime did not recover after cancellation preflight",
        ));
    }
    Ok(())
}

fn wait_for_scheduler_idle(loaded: &LoadedBackend) -> Result<()> {
    let deadline = std::time::Instant::now() + PREFLIGHT_TIMEOUT;
    loop {
        let metrics = get_text(loaded, "metrics")?;
        let all_idle = loaded
            .runtime
            .preflight
            .cancellation_idle_metrics
            .iter()
            .map(|metric| prometheus_metric_sum(&metrics, metric))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .all(|value| value == 0.0);
        if all_idle {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(backend_error(
                "runtime scheduler did not become idle after upstream cancellation",
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn verify_concurrency(loaded: &Arc<LoadedBackend>) -> Result<()> {
    let count = usize::try_from(loaded.runtime.max_concurrent)
        .map_err(|_| invalid("max_concurrent does not fit usize"))?;
    let (started_tx, started_rx) = mpsc::channel();
    let mut releases = Vec::new();
    let mut handles = Vec::new();
    for index in 0..count {
        let backend = Arc::clone(loaded);
        let started = started_tx.clone();
        let (release_tx, release_rx) = mpsc::channel();
        releases.push(release_tx);
        handles.push(thread::spawn(move || {
            let cancellation = CancellationToken::new();
            let cancel_after_release = cancellation.clone();
            let mut first = true;
            let mut request = preflight_request(
                format!("Runtime concurrency probe {index}: explain consensus at length."),
                backend.runtime.preflight.concurrency_max_tokens,
                &backend.runtime.preflight.non_reasoning_chat_template_kwargs,
            );
            request.ignore_eos = true;
            generate(
                backend,
                request,
                &mut |chunk: TokenChunk| {
                    if first && !chunk.text.is_empty() {
                        first = false;
                        started.send(index).map_err(|_| {
                            backend_error("concurrency preflight coordinator stopped")
                        })?;
                        release_rx.recv_timeout(PREFLIGHT_TIMEOUT).map_err(|_| {
                            backend_error("concurrency preflight release timed out")
                        })?;
                        cancel_after_release.cancel();
                    }
                    Ok(())
                },
                &cancellation,
            )
        }));
    }
    drop(started_tx);
    let proof = wait_for_concurrency_proof(
        count,
        &started_rx,
        PREFLIGHT_TIMEOUT,
        Duration::from_millis(100),
        || {
            prometheus_metric_sum(
                &get_text(loaded, "metrics")?,
                &loaded.runtime.preflight.concurrency_active_metric,
            )
        },
    );
    for release in releases {
        let _ = release.send(());
    }
    let mut worker_error = None;
    for handle in handles {
        match handle.join() {
            Ok(Err(EngineError::Cancelled)) | Ok(Ok(_)) => {}
            Ok(Err(error)) if worker_error.is_none() => worker_error = Some(error),
            Err(_) if worker_error.is_none() => {
                worker_error = Some(backend_error("concurrency preflight worker panicked"));
            }
            Ok(Err(_)) | Err(_) => {}
        }
    }
    proof?;
    if let Some(error) = worker_error {
        return Err(error);
    }
    if loaded.runtime.capabilities.contains("cancellation") {
        wait_for_scheduler_idle(loaded)?;
    }
    Ok(())
}

fn wait_for_concurrency_proof<F>(
    count: usize,
    started_rx: &mpsc::Receiver<usize>,
    timeout: Duration,
    poll_interval: Duration,
    mut active_sample: F,
) -> Result<()>
where
    F: FnMut() -> Result<f64>,
{
    let deadline = std::time::Instant::now() + timeout;
    let mut started = BTreeSet::new();
    let mut peak_active = 0.0_f64;
    let mut observed_active = false;
    loop {
        loop {
            match started_rx.try_recv() {
                Ok(index) if index < count => {
                    started.insert(index);
                }
                Ok(_) => {
                    return Err(backend_error(
                        "concurrency preflight worker reported an invalid index",
                    ));
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
        let active = active_sample()?;
        peak_active = peak_active.max(active);
        observed_active |= active >= count as f64;
        if observed_active && started.len() == count {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(backend_error(format!(
                "runtime did not prove {count} concurrent requests: first content from {}/{count}, peak signed active metric {peak_active}",
                started.len()
            )));
        }
        thread::sleep(poll_interval);
    }
}

fn generate(
    loaded: Arc<LoadedBackend>,
    request: GenerateRequest,
    sink: &mut dyn TokenSink,
    cancellation: &CancellationToken,
) -> Result<GenerateOutput> {
    request.validate_sampling()?;
    cancellation.check()?;
    let _permit = loaded.gate.acquire(cancellation)?;
    let body = chat_request_body(&loaded.runtime.served_model, request)?;
    let url = endpoint_url(&loaded.base_url, "v1/chat/completions")?;
    // Unbounded delivery keeps cancellation from deadlocking behind a full
    // producer queue while the caller is unwinding after a disconnect.
    let (events_tx, events_rx) = mpsc::channel();
    let network_cancellation = cancellation.clone();
    let internal_cancellation = CancellationToken::new();
    let worker_cancellation = internal_cancellation.clone();
    let client = loaded.client.clone();
    let error_events = events_tx.clone();
    let worker = thread::spawn(move || {
        let result = run_async(move || async move {
            stream_completion(
                client,
                url,
                body,
                network_cancellation,
                worker_cancellation,
                events_tx,
            )
            .await
        });
        if let Err(error) = result {
            let _ = error_events.send(StreamEvent::Error(error));
        }
    });
    let mut index = 0u32;
    let result = loop {
        if cancellation.is_cancelled() {
            internal_cancellation.cancel();
            break Err(EngineError::Cancelled);
        }
        match events_rx.recv_timeout(REQUEST_POLL_INTERVAL) {
            Ok(StreamEvent::Text { kind, text }) => {
                let chunk = TokenChunk {
                    index,
                    token_id: openai_compatible_pseudo_token_id(kind, &text),
                    text,
                };
                index = index.saturating_add(1);
                if let Err(error) = sink.on_token(chunk) {
                    internal_cancellation.cancel();
                    break Err(error);
                }
            }
            Ok(StreamEvent::Complete(output)) => break Ok(output),
            Ok(StreamEvent::Error(error)) => break Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err(backend_error(
                    "stream worker stopped without a terminal event",
                ));
            }
        }
    };
    internal_cancellation.cancel();
    if worker.join().is_err() && result.is_ok() {
        return Err(backend_error("stream worker panicked"));
    }
    result
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StreamTextKind {
    Content,
    Reasoning,
    Synthetic,
}

impl StreamTextKind {
    fn domain(self) -> &'static [u8] {
        match self {
            Self::Content => b"content",
            Self::Reasoning => b"reasoning",
            Self::Synthetic => b"synthetic",
        }
    }
}

fn openai_compatible_pseudo_token_id(kind: StreamTextKind, text: &str) -> i32 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mayhem-openai-compatible-sse-pseudo-token-v1\0");
    hasher.update(kind.domain());
    hasher.update(b"\0");
    hasher.update(&u64::try_from(text.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(text.as_bytes());
    let bytes: [u8; 4] = hasher.finalize().as_bytes()[..4].try_into().unwrap();
    let token_id = i32::from_be_bytes(bytes);
    if token_id == 0 {
        1
    } else {
        token_id
    }
}

enum StreamEvent {
    Text { kind: StreamTextKind, text: String },
    Complete(GenerateOutput),
    Error(EngineError),
}

async fn stream_completion(
    client: Client,
    url: Url,
    body: Value,
    cancellation: CancellationToken,
    internal_cancellation: CancellationToken,
    events: mpsc::Sender<StreamEvent>,
) -> Result<()> {
    let response = tokio::select! {
        () = wait_cancelled(&cancellation, &internal_cancellation) => {
            return Err(EngineError::Cancelled);
        }
        response = client.post(url).json(&body).send() => response.map_err(map_http_error)?,
    };
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(backend_error(format!(
            "chat completion returned HTTP {status}: {}",
            bounded_error_text(&body)
        )));
    }
    let mut bytes = response.bytes_stream();
    let mut pending = Vec::new();
    let mut collector = StreamCollector::default();
    loop {
        let next = tokio::select! {
            () = wait_cancelled(&cancellation, &internal_cancellation) => {
                return Err(EngineError::Cancelled);
            }
            next = bytes.next() => next,
        };
        let Some(next) = next else { break };
        let chunk = next.map_err(map_http_error)?;
        pending.extend_from_slice(&chunk);
        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let mut line = pending.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = std::str::from_utf8(&line)
                .map_err(|_| backend_error("chat completion SSE line was not UTF-8"))?;
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if data == "[DONE]" {
                    let output = collector.finish(&events)?;
                    let _ = events.send(StreamEvent::Complete(output));
                    return Ok(());
                }
                if !data.is_empty() {
                    let value: Value = serde_json::from_str(data).map_err(|error| {
                        backend_error(format!("invalid chat completion SSE JSON: {error}"))
                    })?;
                    collector.push(value, &events)?;
                }
            }
        }
    }
    Err(backend_error("chat completion stream ended before [DONE]"))
}

#[derive(Default)]
struct StreamCollector {
    text: String,
    reasoning: String,
    reasoning_open: bool,
    reasoning_closed: bool,
    tool_calls: BTreeMap<usize, ToolCallAccumulator>,
    usage: UsageCounters,
    finish_reason: Option<FinishReason>,
}

#[derive(Default)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

impl StreamCollector {
    fn push(&mut self, value: Value, events: &mpsc::Sender<StreamEvent>) -> Result<()> {
        if let Some(usage) = value.get("usage") {
            self.usage = parse_usage(usage);
        }
        for choice in value
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                self.finish_reason = Some(if reason == "length" {
                    FinishReason::Length
                } else {
                    FinishReason::Stop
                });
            }
            let Some(delta) = choice.get("delta").and_then(Value::as_object) else {
                continue;
            };
            if let Some(reasoning) = delta.get("reasoning_content").and_then(Value::as_str) {
                if !reasoning.is_empty() {
                    if !self.reasoning_open {
                        self.reasoning_open = true;
                        events
                            .send(StreamEvent::Text {
                                kind: StreamTextKind::Synthetic,
                                text: "<think>".to_owned(),
                            })
                            .map_err(|_| EngineError::Cancelled)?;
                    }
                    self.reasoning.push_str(reasoning);
                    events
                        .send(StreamEvent::Text {
                            kind: StreamTextKind::Reasoning,
                            text: reasoning.to_owned(),
                        })
                        .map_err(|_| EngineError::Cancelled)?;
                }
            }
            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                if !content.is_empty() {
                    self.close_reasoning(events)?;
                    self.text.push_str(content);
                    events
                        .send(StreamEvent::Text {
                            kind: StreamTextKind::Content,
                            text: content.to_owned(),
                        })
                        .map_err(|_| EngineError::Cancelled)?;
                }
            }
            for call in delta
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let index = call.get("index").and_then(Value::as_u64).unwrap_or(0);
                let index = usize::try_from(index)
                    .map_err(|_| backend_error("tool call index does not fit usize"))?;
                let target = self.tool_calls.entry(index).or_default();
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    target.id.push_str(id);
                }
                if let Some(function) = call.get("function") {
                    if let Some(name) = function.get("name").and_then(Value::as_str) {
                        target.name.push_str(name);
                    }
                    if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                        target.arguments.push_str(arguments);
                    }
                }
            }
        }
        Ok(())
    }

    fn close_reasoning(&mut self, events: &mpsc::Sender<StreamEvent>) -> Result<()> {
        if self.reasoning_open && !self.reasoning_closed {
            self.reasoning_closed = true;
            events
                .send(StreamEvent::Text {
                    kind: StreamTextKind::Synthetic,
                    text: "</think>".to_owned(),
                })
                .map_err(|_| EngineError::Cancelled)?;
        }
        Ok(())
    }

    fn finish(mut self, events: &mpsc::Sender<StreamEvent>) -> Result<GenerateOutput> {
        self.close_reasoning(events)?;
        let mut output = mayhem_proto::openai_compatible_canary_output(&self.reasoning, &self.text);
        if !self.tool_calls.is_empty() {
            let calls = self
                .tool_calls
                .into_values()
                .map(|call| {
                    json!({
                        "id": call.id,
                        "type": "function",
                        "function": {
                            "name": call.name,
                            "arguments": call.arguments,
                        }
                    })
                })
                .collect::<Vec<_>>();
            let envelope = serde_json::to_string(&json!({"tool_calls": calls}))?;
            events
                .send(StreamEvent::Text {
                    kind: StreamTextKind::Synthetic,
                    text: envelope.clone(),
                })
                .map_err(|_| EngineError::Cancelled)?;
            output.push_str(&envelope);
        }
        Ok(GenerateOutput {
            text: output,
            usage: self.usage,
            finish_reason: self.finish_reason.unwrap_or(FinishReason::Stop),
        })
    }
}

fn chat_request_body(model: &str, request: GenerateRequest) -> Result<Value> {
    let messages = if request.messages.is_empty() {
        vec![json!({"role": "user", "content": request.prompt})]
    } else {
        normalize_openai_chat_messages(request.messages)?
    };
    let mut body = Map::from_iter([
        ("model".to_owned(), json!(model)),
        ("messages".to_owned(), Value::Array(messages)),
        ("max_tokens".to_owned(), json!(request.max_new_tokens)),
        ("stream".to_owned(), json!(true)),
        ("stream_options".to_owned(), json!({"include_usage": true})),
    ]);
    insert_optional(&mut body, "temperature", request.temperature);
    insert_optional(&mut body, "top_p", request.top_p);
    insert_optional(&mut body, "top_k", request.top_k);
    insert_optional(&mut body, "min_p", request.min_p);
    insert_optional(&mut body, "repetition_penalty", request.repeat_penalty);
    insert_optional(&mut body, "frequency_penalty", request.frequency_penalty);
    insert_optional(&mut body, "presence_penalty", request.presence_penalty);
    insert_optional(&mut body, "seed", request.seed);
    if !request.stop.is_empty() {
        body.insert("stop".to_owned(), json!(request.stop));
    }
    if request.ignore_eos {
        body.insert("ignore_eos".to_owned(), json!(true));
    }
    if !request.tools.is_empty() {
        body.insert("tools".to_owned(), Value::Array(request.tools));
    }
    if let Some(parallel) = request.parallel_tool_calls {
        body.insert("parallel_tool_calls".to_owned(), json!(parallel));
    }
    if let Some(grammar) = request.grammar {
        match grammar {
            GrammarSpec::JsonSchema { schema } => {
                body.insert(
                    "response_format".to_owned(),
                    json!({
                        "type": "json_schema",
                        "json_schema": {"name": "mayhem_response", "schema": schema, "strict": true}
                    }),
                );
            }
            GrammarSpec::ToolCall { tools } => {
                if !body.contains_key("tools") {
                    body.insert(
                        "tools".to_owned(),
                        Value::Array(tools.into_iter().map(openai_tool).collect()),
                    );
                }
                body.insert("tool_choice".to_owned(), json!("required"));
            }
            GrammarSpec::Gbnf { .. } => {
                return Err(EngineError::InvalidRequest(
                    "OpenAI-compatible runtimes do not have a portable GBNF request field"
                        .to_owned(),
                ));
            }
        }
    }
    let mut template_kwargs = Map::new();
    for parameter in request.speciality_parameters {
        match parameter.target {
            GenerateSpecialityTarget::ChatTemplateKwarg => {
                template_kwargs.insert(parameter.native_path, parameter.value);
            }
            GenerateSpecialityTarget::SamplingParameter
            | GenerateSpecialityTarget::BackendParameter => {
                body.insert(parameter.native_path, parameter.value);
            }
            GenerateSpecialityTarget::PromptSuffix => {
                return Err(EngineError::InvalidRequest(
                    "OpenAI-compatible message requests cannot append a prompt suffix".to_owned(),
                ));
            }
        }
    }
    if !template_kwargs.is_empty() {
        body.insert(
            "chat_template_kwargs".to_owned(),
            Value::Object(template_kwargs),
        );
    }
    Ok(Value::Object(body))
}

fn normalize_openai_chat_messages(mut messages: Vec<Value>) -> Result<Vec<Value>> {
    for (message_index, message) in messages.iter_mut().enumerate() {
        let Some(parts) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        for (part_index, part) in parts.iter_mut().enumerate() {
            if part.get("type").and_then(Value::as_str) != Some("video") {
                continue;
            }
            let video = part
                .get("video")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    EngineError::InvalidRequest(format!(
                        "message {message_index} content part {part_index} has no video descriptor"
                    ))
                })?;
            let url = match (
                video.get("url").and_then(Value::as_str),
                video.get("data").and_then(Value::as_str),
            ) {
                (Some(url), None) if !url.trim().is_empty() => url.to_owned(),
                (None, Some(data)) if !data.is_empty() => {
                    let content_type = video
                        .get("content_type")
                        .and_then(Value::as_str)
                        .filter(|content_type| valid_video_content_type(content_type))
                        .ok_or_else(|| {
                            EngineError::InvalidRequest(format!(
                                "message {message_index} content part {part_index} has no valid video content_type"
                            ))
                        })?;
                    format!("data:{content_type};base64,{data}")
                }
                (Some(_), Some(_)) => {
                    return Err(EngineError::InvalidRequest(format!(
                        "message {message_index} content part {part_index} has ambiguous video data and url"
                    )));
                }
                _ => {
                    return Err(EngineError::InvalidRequest(format!(
                        "message {message_index} content part {part_index} has no usable video data or url"
                    )));
                }
            };
            *part = json!({"type": "video_url", "video_url": {"url": url}});
        }
    }
    Ok(messages)
}

fn valid_video_content_type(content_type: &str) -> bool {
    content_type.strip_prefix("video/").is_some_and(|subtype| {
        !subtype.is_empty()
            && subtype.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                    )
            })
    })
}

fn openai_tool(tool: ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.parameters,
            "strict": tool.strict,
        }
    })
}

fn insert_optional<T: Serialize>(body: &mut Map<String, Value>, key: &str, value: Option<T>) {
    if let Some(value) = value {
        body.insert(key.to_owned(), json!(value));
    }
}

fn tokenize(loaded: Arc<LoadedBackend>, text: &str) -> Result<Tokenization> {
    let client = loaded.client.clone();
    let url = endpoint_url(&loaded.base_url, "v1/tokenize")?;
    let model = loaded.runtime.served_model.clone();
    let prompt = text.to_owned();
    let response = run_async(move || async move {
        let response = client
            .post(url)
            .json(&json!({"model": model, "prompt": prompt}))
            .send()
            .await
            .map_err(map_http_error)?;
        json_response(response, "/v1/tokenize").await
    })?;
    let tokens = response
        .get("tokens")
        .and_then(Value::as_array)
        .ok_or_else(|| backend_error("/v1/tokenize response is missing tokens"))?;
    let token_ids = tokens
        .iter()
        .map(|token| {
            token
                .as_i64()
                .and_then(|token| i32::try_from(token).ok())
                .ok_or_else(|| backend_error("/v1/tokenize returned a token outside i32"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Tokenization { token_ids })
}

fn get_json(loaded: &LoadedBackend, path: &str) -> Result<Value> {
    let client = loaded.client.clone();
    let url = endpoint_url(&loaded.base_url, path)?;
    let label = path.to_owned();
    run_async(move || async move {
        let response = client.get(url).send().await.map_err(map_http_error)?;
        json_response(response, &label).await
    })
}

fn get_text(loaded: &LoadedBackend, path: &str) -> Result<String> {
    let client = loaded.client.clone();
    let url = endpoint_url(&loaded.base_url, path)?;
    let label = path.to_owned();
    run_async(move || async move {
        let response = client.get(url).send().await.map_err(map_http_error)?;
        if !response.status().is_success() {
            return Err(backend_error(format!(
                "/{label} returned HTTP {}",
                response.status()
            )));
        }
        response.text().await.map_err(map_http_error)
    })
}

async fn json_response(response: reqwest::Response, label: &str) -> Result<Value> {
    if !response.status().is_success() {
        return Err(backend_error(format!(
            "{label} returned HTTP {}",
            response.status()
        )));
    }
    response.json().await.map_err(map_http_error)
}

fn endpoint_url(base: &Url, path: &str) -> Result<Url> {
    base.join(path)
        .map_err(|error| invalid(format!("invalid endpoint path {path}: {error}")))
}

fn validate_loopback_base_url(value: &str) -> Result<Url> {
    let mut url = Url::parse(value)
        .map_err(|error| invalid(format!("invalid OpenAI-compatible base URL: {error}")))?;
    if url.scheme() != "http" {
        return Err(invalid("OpenAI-compatible base URL must use loopback HTTP"));
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(invalid(
            "OpenAI-compatible base URL must be a bare origin without credentials, path, query, or fragment",
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| invalid("OpenAI-compatible base URL is missing an IP host"))?;
    let ip = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .map_err(|_| invalid("OpenAI-compatible base URL host must be a loopback IP literal"))?;
    if !ip.is_loopback() {
        return Err(invalid(
            "OpenAI-compatible base URL host must be a loopback IP literal",
        ));
    }
    if url.port().is_none() {
        return Err(invalid(
            "OpenAI-compatible base URL must include an explicit port",
        ));
    }
    url.set_path("/");
    Ok(url)
}

fn prometheus_metric_sum(metrics: &str, metric: &str) -> Result<f64> {
    prometheus_metric_sum_optional(metrics, metric)?.ok_or_else(|| {
        backend_error(format!(
            "/metrics omitted signed prefix cache metric {metric}"
        ))
    })
}

fn prometheus_metric_sum_optional(metrics: &str, metric: &str) -> Result<Option<f64>> {
    let mut found = false;
    let mut total = 0.0;
    for line in metrics.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix(metric) else {
            continue;
        };
        if !rest.starts_with('{') && !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let value = line
            .split_whitespace()
            .last()
            .and_then(|value| value.parse::<f64>().ok())
            .ok_or_else(|| backend_error(format!("metric {metric} has an invalid sample")))?;
        found = true;
        total += value;
    }
    Ok(found.then_some(total))
}

fn parse_usage(value: &Value) -> UsageCounters {
    let u32_field = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0)
    };
    let reasoning_tokens = value
        .pointer("/completion_tokens_details/reasoning_tokens")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0);
    let vision_tokens = value
        .pointer("/prompt_tokens_details/image_tokens")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0)
        .saturating_add(
            value
                .pointer("/prompt_tokens_details/video_tokens")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(0),
        );
    UsageCounters {
        prompt_tokens: u32_field("prompt_tokens"),
        completion_tokens: u32_field("completion_tokens"),
        total_tokens: u32_field("total_tokens"),
        reasoning_tokens,
        vision_tokens,
        audio_tokens: value
            .pointer("/prompt_tokens_details/audio_tokens")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0),
    }
}

async fn wait_cancelled(external: &CancellationToken, internal: &CancellationToken) {
    loop {
        if external.is_cancelled() || internal.is_cancelled() {
            return;
        }
        tokio::time::sleep(REQUEST_POLL_INTERVAL).await;
    }
}

fn run_async<T, F, Fut>(operation: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T>> + 'static,
{
    thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| backend_error(format!("building HTTP runtime failed: {error}")))?
            .block_on(operation())
    })
    .join()
    .map_err(|_| backend_error("HTTP runtime worker panicked"))?
}

fn safe_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn safe_metric_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'.'))
}

fn bounded_error_text(value: &str) -> String {
    value.chars().take(512).collect()
}

fn map_http_error(error: reqwest::Error) -> EngineError {
    backend_error(error.to_string())
}

fn backend_error(message: impl Into<String>) -> EngineError {
    EngineError::OpenAiCompatible(message.into())
}

fn invalid(message: impl Into<String>) -> EngineError {
    EngineError::InvalidConfig(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Instant;

    fn runtime() -> OpenAiCompatibleRuntimeBinding {
        OpenAiCompatibleRuntimeBinding {
            schema_version: 1,
            lifecycle: OpenAiCompatibleLifecycle::ManagedOrVerifiedAttach,
            runtime_id: "runtime".to_owned(),
            runtime_version: "1.0.0".to_owned(),
            runtime_revision: "ab".repeat(20),
            implementation: "engine".to_owned(),
            implementation_version: "1.0.0+rev".to_owned(),
            container_image_digest: format!("sha256:{}", "cd".repeat(32)),
            served_model: "org/model".to_owned(),
            native_context: 262_144,
            served_context: 524_288,
            max_concurrent: 2,
            calibrated_peak_gpu_memory_bytes: None,
            calibrated_peak_host_memory_bytes: None,
            memory_measurement_source: None,
            model_snapshot_file_count: 419,
            snapshot_manifest_sidecar: "snapshot_manifest".to_owned(),
            snapshot_manifest_sha256: "de".repeat(32),
            runtime_recipe_sidecar: "runtime_recipe".to_owned(),
            runtime_recipe_sha256: "ef".repeat(32),
            capabilities: BTreeSet::from(["streaming".to_owned()]),
            server_info_checks: BTreeMap::from([("/version".to_owned(), json!("1.0"))]),
            preflight: OpenAiCompatiblePreflightProfile {
                non_reasoning_chat_template_kwargs: BTreeMap::from([(
                    "enable_thinking".to_owned(),
                    json!(false),
                )]),
                reasoning_chat_template_kwargs: BTreeMap::from([(
                    "enable_thinking".to_owned(),
                    json!(true),
                )]),
                streaming_max_tokens: 32,
                tools_max_tokens: 384,
                json_max_tokens: 256,
                reasoning_max_tokens: 1024,
                cache_max_tokens: 32,
                cancellation_max_tokens: 4096,
                concurrency_max_tokens: 4096,
                cancellation_idle_metrics: BTreeSet::from([
                    "runtime_active_requests".to_owned(),
                    "runtime_queued_requests".to_owned(),
                ]),
                concurrency_active_metric: "runtime_active_requests".to_owned(),
            },
            prefix_cache_metric: None,
        }
    }

    #[test]
    fn endpoint_accepts_only_bare_loopback_http_origins() {
        assert!(validate_loopback_base_url("http://127.0.0.1:8000/").is_ok());
        assert!(validate_loopback_base_url("http://[::1]:8000/").is_ok());
        for rejected in [
            "https://127.0.0.1:8000/",
            "http://localhost:8000/",
            "http://192.0.2.1:8000/",
            "http://127.0.0.1:8000/v1",
            "http://user@127.0.0.1:8000/",
            "http://127.0.0.1/",
        ] {
            assert!(
                validate_loopback_base_url(rejected).is_err(),
                "accepted {rejected}"
            );
        }
    }

    #[test]
    fn signed_binding_validates_identity_and_capability_invariants() {
        runtime().validate().unwrap();
        let mut binding = runtime();
        binding.max_concurrent = 0;
        assert!(binding.validate().is_err());
        let mut binding = runtime();
        binding.capabilities.insert("prefix_cache".to_owned());
        assert!(binding.validate().is_err());
        let mut binding = runtime();
        binding.container_image_digest = "latest".to_owned();
        assert!(binding.validate().is_err());
        let mut binding = runtime();
        binding.preflight.concurrency_active_metric.clear();
        assert!(binding.validate().is_err());
    }

    #[test]
    fn request_maps_native_tools_json_and_reasoning_controls() {
        let mut request = GenerateRequest::new("hello");
        request.grammar = Some(GrammarSpec::ToolCall {
            tools: vec![ToolSpec::new("lookup", json!({"type": "object"}))],
        });
        request
            .speciality_parameters
            .push(crate::GenerateSpecialityParameter {
                name: "reasoning".to_owned(),
                level: "high".to_owned(),
                target: GenerateSpecialityTarget::ChatTemplateKwarg,
                native_path: "enable_thinking".to_owned(),
                value: json!(true),
                max_reasoning_tokens: None,
            });
        let body = chat_request_body("org/model", request).unwrap();
        assert_eq!(body["model"], "org/model");
        assert_eq!(body["tool_choice"], "required");
        assert_eq!(body["tools"][0]["function"]["name"], "lookup");
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], true);
    }

    #[test]
    fn request_normalizes_hf_video_inside_mixed_chat_content() {
        let mut request = GenerateRequest::new("unused");
        request.messages = vec![json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "Compare the inputs."},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,aW1hZ2U="}},
                {"type": "video", "video": {
                    "data": "dmlkZW8=",
                    "content_type": "video/mp4",
                    "num_frames": 8,
                    "fps": 2
                }},
                {"type": "input_audio", "input_audio": {"data": "UklGRg==", "format": "wav"}}
            ]
        })];

        let body = chat_request_body("org/model", request).unwrap();
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(
            content[0],
            json!({"type": "text", "text": "Compare the inputs."})
        );
        assert_eq!(
            content[1],
            json!({"type": "image_url", "image_url": {"url": "data:image/png;base64,aW1hZ2U="}})
        );
        assert_eq!(
            content[2],
            json!({"type": "video_url", "video_url": {"url": "data:video/mp4;base64,dmlkZW8="}})
        );
        assert_eq!(
            content[3],
            json!({"type": "input_audio", "input_audio": {"data": "UklGRg==", "format": "wav"}})
        );
    }

    #[test]
    fn request_rejects_ambiguous_or_unsafe_hf_video_descriptors() {
        for video in [
            json!({"data": "dmlkZW8=", "url": "https://example.test/video.mp4", "content_type": "video/mp4"}),
            json!({"data": "dmlkZW8=", "content_type": "text/plain"}),
            json!({"frames": ["ZnJhbWU="]}),
        ] {
            let mut request = GenerateRequest::new("unused");
            request.messages = vec![json!({
                "role": "user",
                "content": [{"type": "video", "video": video}]
            })];
            assert!(chat_request_body("org/model", request).is_err(), "{video}");
        }
    }

    #[test]
    fn canonical_canary_units_ignore_sse_segmentation_and_bind_content() {
        let fingerprint = |tokens: &[i32]| {
            let mut hasher = blake3::Hasher::new();
            for token in tokens {
                hasher.update(&token.to_be_bytes());
            }
            hasher.finalize().to_hex().to_string()
        };
        let collect = |deltas: &[Value]| {
            let (events, _receiver) = mpsc::channel();
            let mut collector = StreamCollector::default();
            for delta in deltas {
                collector.push(delta.clone(), &events).unwrap();
            }
            collector.finish(&events).unwrap().text
        };
        let split = collect(&[
            json!({"choices":[{"delta":{"reasoning_content":"inspect "}}]}),
            json!({"choices":[{"delta":{"reasoning_content":"alpha"}}]}),
            json!({"choices":[{"delta":{"content":"answer "}}]}),
            json!({"choices":[{"delta":{"content":"α"}}]}),
        ]);
        let joined = collect(&[
            json!({"choices":[{"delta":{"reasoning_content":"inspect alpha"}}]}),
            json!({"choices":[{"delta":{"content":"answer α"}}]}),
        ]);
        let different = collect(&[
            json!({"choices":[{"delta":{"reasoning_content":"inspect bravo"}}]}),
            json!({"choices":[{"delta":{"content":"answer β"}}]}),
        ]);
        let split_units = mayhem_proto::openai_compatible_canary_units(&split);
        let joined_units = mayhem_proto::openai_compatible_canary_units(&joined);
        let different_units = mayhem_proto::openai_compatible_canary_units(&different);

        assert_eq!(split, "<think>inspect alpha</think>answer α");
        assert_eq!(split, joined);
        assert_eq!(split_units, joined_units);
        assert_eq!(fingerprint(&split_units), fingerprint(&joined_units));
        assert_eq!(split_units.len(), different_units.len());
        assert_ne!(split_units, different_units);
        assert_ne!(fingerprint(&split_units), fingerprint(&different_units));
    }

    #[test]
    fn preflight_reasoning_controls_have_unique_safe_names() {
        let controls = BTreeMap::from([
            ("enable_thinking".to_owned(), json!(true)),
            ("preserve_thinking".to_owned(), json!(true)),
            ("reasoning_effort".to_owned(), json!("xhigh")),
        ]);
        let request = preflight_request("reason", 1024, &controls);

        request.validate_sampling().unwrap();
        assert_eq!(request.speciality_parameters.len(), 3);
        assert_eq!(
            request
                .speciality_parameters
                .iter()
                .map(|parameter| (parameter.name.as_str(), parameter.native_path.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("enable_thinking", "enable_thinking"),
                ("preserve_thinking", "preserve_thinking"),
                ("reasoning_effort", "reasoning_effort"),
            ]
        );
    }

    #[test]
    fn concurrency_proof_polls_past_a_delayed_active_metric_sample() {
        let (started_tx, started_rx) = mpsc::channel();
        started_tx.send(0).unwrap();
        started_tx.send(1).unwrap();
        let mut samples = [0.0, 0.0, 2.0].into_iter();
        let mut polls = 0;

        wait_for_concurrency_proof(
            2,
            &started_rx,
            Duration::from_secs(1),
            Duration::ZERO,
            || {
                polls += 1;
                Ok(samples.next().unwrap_or(2.0))
            },
        )
        .unwrap();

        assert_eq!(polls, 3);
    }

    #[test]
    fn prometheus_parser_sums_labeled_samples_exactly() {
        let metrics = "# HELP x cache\nsglang:cached_tokens_total{rank=\"0\"} 12\nsglang:cached_tokens_total{rank=\"1\"} 7\nsglang:cached_tokens_total_extra 90\n";
        assert_eq!(
            prometheus_metric_sum(metrics, "sglang:cached_tokens_total").unwrap(),
            19.0
        );
    }

    #[test]
    fn prefix_cache_proof_accepts_lazy_zero_baseline_but_requires_replay_increase() {
        let metric = "sglang:cached_tokens_total";
        let absent = "# HELP sglang:num_running_reqs running\nsglang:num_running_reqs 0\n";
        let first_hit = "sglang:cached_tokens_total{rank=\"0\",source=\"gpu\"} 25472\n";

        verify_prefix_cache_metric_increase(metric, absent, first_hit).unwrap();
        assert!(verify_prefix_cache_metric_increase(metric, absent, absent).is_err());
        assert!(verify_prefix_cache_metric_increase(metric, first_hit, first_hit).is_err());
    }

    fn spawn_sse_server(chunks: Vec<Vec<u8>>, hold_open: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = socket.read(&mut buffer).unwrap();
                if read == 0 {
                    return;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n",
                )
                .unwrap();
            for chunk in chunks {
                write!(socket, "{:x}\r\n", chunk.len()).unwrap();
                socket.write_all(&chunk).unwrap();
                socket.write_all(b"\r\n").unwrap();
                socket.flush().unwrap();
                thread::sleep(Duration::from_millis(2));
            }
            if hold_open {
                thread::sleep(Duration::from_secs(3));
            } else {
                socket.write_all(b"0\r\n\r\n").unwrap();
            }
        });
        format!("http://{address}/")
    }

    fn spawn_identity_server(responses: Vec<(&'static str, Value)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            for (expected_path, body) in responses {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let read = socket.read(&mut buffer).unwrap();
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let request = String::from_utf8(request).unwrap();
                assert!(
                    request.starts_with(&format!("GET {expected_path} HTTP/1.1\r\n")),
                    "unexpected request: {request}"
                );
                let body = serde_json::to_vec(&body).unwrap();
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                socket.write_all(&body).unwrap();
            }
        });
        format!("http://{address}/")
    }

    #[test]
    fn component_health_rechecks_models_and_signed_server_identity() {
        let binding = runtime();
        let base_url = spawn_identity_server(vec![
            (
                "/v1/models",
                json!({"data":[{"id":"org/model","max_model_len":524288}]}),
            ),
            ("/server_info", json!({"version":"1.0"})),
            (
                "/v1/models",
                json!({"data":[{"id":"org/model","max_model_len":524288}]}),
            ),
            ("/server_info", json!({"version":"drifted"})),
        ]);
        let mut backend = OpenAiCompatibleBackend::new(OpenAiCompatibleBackendConfig {
            base_url: base_url.clone(),
            runtime: binding.clone(),
            readiness_timeout: Duration::ZERO,
        })
        .unwrap();
        backend.loaded = Some(Arc::new(LoadedBackend {
            client: Client::builder().build().unwrap(),
            base_url: validate_loopback_base_url(&base_url).unwrap(),
            runtime: binding,
            artifact: crate::ModelArtifact::openai_compatible_model("unused"),
            ctx_size: 1024,
            proven_capabilities: BTreeSet::from(["streaming".to_owned()]),
            gate: Arc::new(GenerationGate {
                capacity: 2,
                active: Mutex::new(0),
                changed: Condvar::new(),
            }),
        }));
        assert!(backend.component_healthy());
        assert!(!backend.component_healthy());
    }

    #[test]
    fn actual_http_stream_retains_split_utf8_and_sse_boundaries() {
        let line = "data: {\"choices\":[{\"delta\":{\"content\":\"Grüße\"}}]}\n\n";
        let bytes = line.as_bytes();
        let split = bytes
            .windows(2)
            .position(|window| window == "ü".as_bytes())
            .unwrap()
            + 1;
        let base = spawn_sse_server(
            vec![
                bytes[..split].to_vec(),
                bytes[split..].to_vec(),
                b"data: [DONE]\n\n".to_vec(),
            ],
            false,
        );
        let client = Client::builder().build().unwrap();
        let url = Url::parse(&format!("{base}v1/chat/completions")).unwrap();
        let (tx, rx) = mpsc::channel();
        run_async(move || async move {
            stream_completion(
                client,
                url,
                json!({}),
                CancellationToken::new(),
                CancellationToken::new(),
                tx,
            )
            .await
        })
        .unwrap();
        let mut text = String::new();
        let mut complete = None;
        for event in rx {
            match event {
                StreamEvent::Text { text: part, .. } => text.push_str(&part),
                StreamEvent::Complete(output) => complete = Some(output),
                StreamEvent::Error(error) => panic!("unexpected stream error: {error}"),
            }
        }
        assert_eq!(text, "Grüße");
        assert_eq!(complete.unwrap().text, "Grüße");
    }

    #[test]
    fn cancellation_does_not_deadlock_behind_many_fast_deltas() {
        let mut chunks = Vec::new();
        for _ in 0..256 {
            chunks.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n".to_vec());
        }
        let base_url = validate_loopback_base_url(&spawn_sse_server(chunks, true)).unwrap();
        let loaded = Arc::new(LoadedBackend {
            client: Client::builder().build().unwrap(),
            base_url,
            runtime: runtime(),
            artifact: crate::ModelArtifact::openai_compatible_model("unused"),
            ctx_size: 1024,
            proven_capabilities: BTreeSet::new(),
            gate: Arc::new(GenerationGate {
                capacity: 2,
                active: Mutex::new(0),
                changed: Condvar::new(),
            }),
        });
        let cancellation = CancellationToken::new();
        let cancel_from_sink = cancellation.clone();
        let started = Instant::now();
        let result = generate(
            loaded,
            GenerateRequest::new("cancel"),
            &mut move |_chunk: TokenChunk| {
                cancel_from_sink.cancel();
                Ok(())
            },
            &cancellation,
        );
        assert!(matches!(result, Err(EngineError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
