//! Request-bound JSON/stream protocol adapters. Provider JSON is never a Core
//! receipt. Upstream counts remain claims; observed_usage is independently derived.
//! Full structured-output/tool-argument schema validation belongs to the bounded
//! semantic verifier before this result may authorize delivery/execution/settlement.

mod public;
mod responses_stream;
pub mod stream;
pub use public::{PublicAdapter, PublicAdapterSnapshot, PublicRequest, PublicStream};
#[cfg(test)]
mod public_stream_tests;

use crate::{
    attempts::Digest,
    connector::{
        config::Operation,
        failure::{Code, Execution, Failure, Scope, Stage},
    },
};
use mayhem_proto::{
    endpoint_contract_canonical_fingerprint, endpoint_request_fingerprint, proxy::ProxyEndpoint,
    validate_endpoint_request, EndpointFamilyContract,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("proxy endpoint configuration is invalid")]
    Configuration,
    #[error("proxy endpoint binding does not match the accepted request")]
    Binding,
    #[error("{0}")]
    Request(Failure),
    #[error("the upstream response violates the requested endpoint protocol")]
    Protocol,
}
pub type Result<T> = std::result::Result<T, Error>;

fn require(ok: bool) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Protocol)
    }
}
fn invalid(param: Option<&'static str>, code: Code) -> Error {
    let mut failure = Failure::new(
        code,
        Scope::Request,
        Stage::BeforeDispatch,
        Execution::NotDispatched,
    );
    failure.parameter = param;
    Error::Request(failure)
}
fn request(ok: bool, param: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(invalid(Some(param), Code::InvalidRequest))
    }
}
fn identifier(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
}
fn name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key).and_then(Value::as_str).ok_or(Error::Protocol)
}

// Known public response data only. Consumers must still render text/URLs as
// untrusted model output; these references never authorize broker URL fetching.
fn response_part(value: &Value, item_kind: &str, summary: bool) -> Result<Value> {
    let kind = string(value, "type")?;
    require(match (item_kind, summary, kind) {
        ("message", false, "output_text" | "refusal")
        | ("reasoning", false, "reasoning_text")
        | ("reasoning", true, "summary_text") => true,
        _ => false,
    })?;
    let key = if kind == "refusal" { "refusal" } else { "text" };
    let mut result = json!({"type":kind,key:string(value,key)?});
    if kind == "output_text" {
        let annotations = match value.get("annotations") {
            Some(v) => v
                .as_array()
                .ok_or(Error::Protocol)?
                .iter()
                .map(response_annotation)
                .collect::<Result<Vec<_>>>()?,
            None => vec![],
        };
        result["annotations"] = json!(annotations);
        result["logprobs"] = value
            .get("logprobs")
            .map(response_logprobs)
            .transpose()?
            .unwrap_or_else(|| json!([]));
    }
    Ok(result)
}
fn response_annotation(value: &Value) -> Result<Value> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    let kind = string(value, "type")?;
    let (strings, numbers): (&[&str], &[&str]) = match kind {
        "url_citation" => (&["url", "title"], &["start_index", "end_index"]),
        "file_citation" => (&["file_id", "filename"], &["index"]),
        "container_file_citation" => (
            &["container_id", "file_id", "filename"],
            &["start_index", "end_index"],
        ),
        "file_path" => (&["file_id"], &["index"]),
        _ => return Err(Error::Protocol),
    };
    let mut clean = json!({"type":kind});
    for key in strings {
        clean[*key] = json!(string(value, key)?);
    }
    for key in numbers {
        let n = value
            .get(*key)
            .and_then(Value::as_u64)
            .ok_or(Error::Protocol)?;
        require(n <= 9_007_199_254_740_991)?;
        clean[*key] = json!(n);
    }
    if numbers.contains(&"start_index") {
        require(clean["start_index"].as_u64() <= clean["end_index"].as_u64())?;
    }
    if kind == "url_citation" {
        let u = url::Url::parse(string(value, "url")?).map_err(|_| Error::Protocol)?;
        require(
            matches!(u.scheme(), "http" | "https")
                && u.host_str().is_some()
                && u.username().is_empty()
                && u.password().is_none(),
        )?;
    }
    Ok(clean)
}
fn response_logprobs(value: &Value) -> Result<Value> {
    fn token(value: &Value) -> Result<Value> {
        require(value.is_object())?;
        let mut out = json!({});
        if let Some(t) = value.get("token") {
            require(t.is_string())?;
            out["token"] = t.clone();
        }
        if let Some(p) = value.get("logprob") {
            require(p.as_f64().is_some_and(|v| v.is_finite() && v <= 0.))?;
            out["logprob"] = p.clone();
        }
        require(!out.as_object().unwrap().is_empty())?;
        Ok(out)
    }
    let values = value.as_array().ok_or(Error::Protocol)?;
    let mut clean = Vec::with_capacity(values.len());
    for value in values {
        let mut t = token(value)?;
        if let Some(top) = value.get("top_logprobs") {
            let top = top.as_array().ok_or(Error::Protocol)?;
            require(top.len() <= 20)?;
            t["top_logprobs"] = Value::Array(top.iter().map(token).collect::<Result<Vec<_>>>()?);
        }
        clean.push(t);
    }
    Ok(Value::Array(clean))
}

/// Explicit local resource bounds, not maximum model context or generation time.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub choices: usize,
    pub tools: usize,
    pub questions: usize,
    pub decision_options: usize,
}
impl Limits {
    fn validate(self) -> Result<()> {
        if [self.request_bytes, self.response_bytes]
            .into_iter()
            .any(|n| n == 0 || n > 256 * 1024 * 1024)
            || [
                self.choices,
                self.tools,
                self.questions,
                self.decision_options,
            ]
            .into_iter()
            .any(|n| n == 0 || n > 4096)
        {
            return Err(Error::Configuration);
        }
        Ok(())
    }
}

/// One same-protocol adapter. Custom recipes will produce this public protocol;
/// they cannot select network origins, payment terms, or silently drop controls.
pub struct Adapter {
    protocol: Protocol,
    upstream_model: String,
}

/// Shared public request/result rules. Only Adapter has an upstream model mapping
/// and can produce a dispatchable Request; the buyer exposes a read-only wrapper.
struct Protocol {
    endpoint: ProxyEndpoint,
    contract: EndpointFamilyContract,
    contract_hash: Digest,
    recipe_hash: Digest,
    limits: Limits,
}

/// Private recovery data. Includes the original endpoint contract and upstream
/// model mapping, but no network address, credential, executable or financial key.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterSnapshot {
    pub version: u32,
    pub endpoint: ProxyEndpoint,
    pub contract: EndpointFamilyContract,
    pub upstream_model: String,
    pub limits: Limits,
}
impl fmt::Debug for AdapterSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdapterSnapshot")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}
impl fmt::Debug for Adapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Adapter")
            .field("endpoint", &self.protocol.endpoint)
            .finish_non_exhaustive()
    }
}
impl Adapter {
    pub fn snapshot(&self) -> AdapterSnapshot {
        AdapterSnapshot {
            version: 1,
            endpoint: self.protocol.endpoint,
            contract: self.protocol.contract.clone(),
            upstream_model: self.upstream_model.clone(),
            limits: self.protocol.limits,
        }
    }
    pub fn restore(snapshot: AdapterSnapshot) -> Result<Self> {
        if snapshot.version != 1 {
            return Err(Error::Configuration);
        }
        Self::new(
            snapshot.endpoint,
            snapshot.contract,
            snapshot.upstream_model,
            snapshot.limits,
        )
    }
    pub fn new(
        endpoint: ProxyEndpoint,
        contract: EndpointFamilyContract,
        upstream_model: String,
        limits: Limits,
    ) -> Result<Self> {
        if !identifier(&upstream_model) {
            return Err(Error::Configuration);
        }
        let contract_hash = validate_contract(endpoint, &contract, limits)?;
        let recipe = json!({"version":1,"adapter":"same_protocol_json","endpoint":endpoint,"contract":contract_hash,"upstream_model":upstream_model,"limits":limits});
        let recipe_hash = digest("mayhem/proxy/endpoint-adapter/v1", &recipe)?;
        Ok(Self {
            protocol: Protocol {
                endpoint,
                contract,
                contract_hash,
                recipe_hash,
                limits,
            },
            upstream_model,
        })
    }
    pub fn endpoint(&self) -> ProxyEndpoint {
        self.protocol.endpoint
    }
    pub fn contract_hash(&self) -> &Digest {
        &self.protocol.contract_hash
    }
    pub fn recipe_hash(&self) -> &Digest {
        &self.protocol.recipe_hash
    }
    pub fn limits(&self) -> Limits {
        self.protocol.limits
    }
    pub fn operation(&self) -> Operation {
        match self.protocol.endpoint {
            ProxyEndpoint::Chat => Operation::ChatCompletions,
            ProxyEndpoint::Completions => Operation::Completions,
            ProxyEndpoint::Responses => Operation::Responses,
            ProxyEndpoint::Decisions => Operation::Decisions,
        }
    }
    /// No settings are removed to make a backend accept a request. This adapter
    /// explicitly handles JSON replies; streaming is a separate execution path.
    pub fn prepare_json(&self, bytes: &[u8]) -> Result<Request> {
        self.protocol
            .prepare(bytes, false, Some(&self.upstream_model))
    }
    pub fn prepare_stream(&self, bytes: &[u8]) -> Result<Request> {
        self.protocol
            .prepare(bytes, true, Some(&self.upstream_model))
    }
}

fn validate_contract(
    endpoint: ProxyEndpoint,
    contract: &EndpointFamilyContract,
    limits: Limits,
) -> Result<Digest> {
    limits.validate()?;
    let family = match endpoint {
        ProxyEndpoint::Chat => mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
        ProxyEndpoint::Completions => mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS,
        ProxyEndpoint::Responses => mayhem_proto::ENDPOINT_OPENAI_RESPONSES,
        ProxyEndpoint::Decisions => mayhem_proto::ENDPOINT_MAYHEM_DECISIONS,
    };
    if contract.family != family
        || serde_json::to_vec(contract)
            .map_err(|_| Error::Configuration)?
            .len()
            > 128 * 1024
    {
        return Err(Error::Configuration);
    }
    Digest::new(endpoint_contract_canonical_fingerprint(contract)).map_err(|_| Error::Configuration)
}

impl Protocol {
    fn prepare(
        &self,
        bytes: &[u8],
        streaming: bool,
        upstream_model: Option<&str>,
    ) -> Result<Request> {
        if streaming
            && !matches!(
                self.endpoint,
                ProxyEndpoint::Chat | ProxyEndpoint::Completions | ProxyEndpoint::Responses
            )
        {
            return Err(invalid(Some("stream"), Code::UnsupportedControl));
        }
        if bytes.len() > self.limits.request_bytes {
            return Err(invalid(None, Code::RequestTooLarge));
        }
        let original: Value =
            serde_json::from_slice(bytes).map_err(|_| invalid(None, Code::InvalidRequest))?;
        validate_endpoint_request(&self.contract, &original).map_err(|errors| {
            let param = errors
                .first()
                .and_then(|e| e.path.split('.').next())
                .and_then(crate::connector::failure::safe_parameter);
            invalid(param, Code::InvalidRequest)
        })?;
        if if streaming {
            original.get("stream") != Some(&Value::Bool(true))
        } else {
            original
                .get("stream")
                .is_some_and(|v| v != &Value::Bool(false))
        } {
            return Err(invalid(Some("stream"), Code::UnsupportedControl));
        }
        // This is stateless Responses. Never inherit vendor-side history or stores.
        for key in [
            "previous_response_id",
            "conversation",
            "background",
            "store",
        ] {
            if original
                .get(key)
                .is_some_and(|v| v != &Value::Bool(false) && !v.is_null())
            {
                return Err(invalid(Some(key), Code::UnsupportedControl));
            }
        }
        let public_model = original
            .get("model")
            .and_then(Value::as_str)
            .filter(|s| identifier(s))
            .ok_or_else(|| invalid(Some("model"), Code::InvalidRequest))?
            .to_owned();
        let request_hash = Digest::new(endpoint_request_fingerprint(&original))
            .map_err(|_| Error::Configuration)?;
        let mut choices = original
            .get("n")
            .map(|v| {
                v.as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| invalid(None, Code::InvalidRequest))
            })
            .transpose()?
            .unwrap_or(1);
        if self.endpoint == ProxyEndpoint::Completions {
            if let Some(prompts) = original.get("prompt").and_then(Value::as_array) {
                if prompts.first().is_some_and(Value::is_string) {
                    choices = choices
                        .checked_mul(prompts.len())
                        .ok_or_else(|| invalid(None, Code::InvalidRequest))?;
                }
            }
        }
        request(choices > 0 && choices <= self.limits.choices, "input")?;
        let mut tools = BTreeSet::new();
        if let Some(raw) = original.get("tools") {
            let entries = raw
                .as_array()
                .ok_or_else(|| invalid(Some("tools"), Code::InvalidRequest))?;
            request(entries.len() <= self.limits.tools, "tools")?;
            for tool in entries {
                request(
                    tool.get("type").and_then(Value::as_str) == Some("function"),
                    "tools",
                )?;
                let function = if self.endpoint == ProxyEndpoint::Responses {
                    tool
                } else {
                    &tool["function"]
                };
                let tool_name = function
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid(Some("tools"), Code::InvalidRequest))?;
                request(
                    name(tool_name) && tools.insert(tool_name.to_owned()),
                    "tools",
                )?;
                if let Some(schema) = function.get("parameters") {
                    request(schema.is_object(), "tools")?;
                }
            }
        }
        let mut forced_tool = None;
        let mut require_tool = false;
        let mut forbid_tools = false;
        if let Some(choice) = original.get("tool_choice") {
            match choice.as_str() {
                Some("auto") => (),
                Some("none") => forbid_tools = true,
                Some("required") => require_tool = true,
                Some(_) => return Err(invalid(Some("tools"), Code::UnsupportedControl)),
                None => {
                    request(
                        choice.get("type").and_then(Value::as_str) == Some("function"),
                        "tools",
                    )?;
                    forced_tool = if self.endpoint == ProxyEndpoint::Responses {
                        choice.get("name")
                    } else {
                        choice.pointer("/function/name")
                    }
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                    request(
                        forced_tool.as_ref().is_some_and(|n| tools.contains(n)),
                        "tools",
                    )?;
                    require_tool = true;
                }
            }
        }
        request(!require_tool || !tools.is_empty(), "tools")?;
        let parallel = original
            .get("parallel_tool_calls")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let questions = if self.endpoint == ProxyEndpoint::Decisions {
            decision_questions(&original, self.limits)?
        } else {
            BTreeMap::new()
        };
        let semantic_policy =
            crate::semantics::Policy::from_request(self.endpoint, request_hash.clone(), &original)
                .map_err(Error::Request)?;
        let metering = crate::metering::Policy::for_endpoint(self.endpoint)
            .prepare(self.endpoint, &original)
            .map_err(|_| {
                invalid(
                    Some(if self.endpoint == ProxyEndpoint::Chat {
                        "messages"
                    } else {
                        "input"
                    }),
                    Code::UnsupportedControl,
                )
            })?;
        let health_class = crate::health::Class::request(&original, bytes.len(), streaming);
        let body = if let Some(model) = upstream_model {
            let mut body = original;
            body["model"] = json!(model);
            // OpenAI Responses defaults may otherwise retain vendor-side state.
            // This provider profile must be probed for store=false support.
            if self.endpoint == ProxyEndpoint::Responses {
                body["store"] = json!(false);
            }
            let body = serde_json::to_vec(&body).map_err(|_| Error::Configuration)?;
            if body.len() > self.limits.request_bytes {
                return Err(invalid(None, Code::RequestTooLarge));
            }
            body
        } else {
            // Buyer verification neither translates nor builds an upstream POST.
            Vec::new()
        };
        Ok(Request {
            endpoint: self.endpoint,
            request_hash,
            contract_hash: self.contract_hash.clone(),
            recipe_hash: self.recipe_hash.clone(),
            public_model,
            body,
            choices,
            tools,
            forced_tool,
            require_tool,
            forbid_tools,
            parallel,
            questions,
            limits: self.limits,
            semantic_policy,
            metering,
            streaming,
            health_class,
        })
    }
}

pub(crate) fn digest(domain: &'static str, value: &Value) -> Result<Digest> {
    let stable = endpoint_request_fingerprint(value);
    let mut hasher = blake3::Hasher::new_derive_key(domain);
    hasher.update(stable.as_bytes());
    Digest::new(hasher.finalize().to_hex().to_string()).map_err(|_| Error::Configuration)
}

pub struct Request {
    endpoint: ProxyEndpoint,
    request_hash: Digest,
    contract_hash: Digest,
    recipe_hash: Digest,
    public_model: String,
    body: Vec<u8>,
    choices: usize,
    tools: BTreeSet<String>,
    forced_tool: Option<String>,
    require_tool: bool,
    forbid_tools: bool,
    parallel: bool,
    questions: BTreeMap<String, Question>,
    limits: Limits,
    semantic_policy: crate::semantics::Policy,
    metering: crate::metering::Prepared,
    streaming: bool,
    pub(crate) health_class: crate::health::Class,
}
impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

/// Validated *syntax and consistency*, never trusted for billing or rate floors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportedUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolReply {
    pub body: Value,
    pub reported_usage: Option<ReportedUsage>,
    /// Recomputed from observable input/result; not a receipt or authorization.
    /// Legacy retained results lack this and require explicit verification.
    #[serde(default)]
    pub observed_usage: Option<crate::metering::Observation>,
    pub upstream_id: Option<crate::attempts::RemoteId>,
}
impl fmt::Debug for ProtocolReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProtocolReply").finish_non_exhaustive()
    }
}

impl Request {
    pub fn semantic_policy(&self) -> &crate::semantics::Policy {
        &self.semantic_policy
    }
    pub fn request_hash(&self) -> &Digest {
        &self.request_hash
    }
    pub fn metering_policy_hash(&self) -> Digest {
        self.metering.policy_hash()
    }
    pub fn maximum_usage(&self, output_units: Option<u64>) -> Result<BTreeMap<String, u64>> {
        self.metering
            .maximum_usage(output_units)
            .map_err(|_| Error::Configuration)
    }
    pub fn endpoint(&self) -> ProxyEndpoint {
        self.endpoint
    }
    pub(crate) fn body(&self) -> &[u8] {
        &self.body
    }
    pub fn matches_binding(&self, binding: &crate::attempts::Binding) -> bool {
        self.endpoint == binding.endpoint
            && self.request_hash == binding.request_hash
            && self.contract_hash == binding.endpoint_contract
            && self.recipe_hash == binding.recipe_digest
    }
    pub fn decode_json(
        &self,
        value: Value,
        public_id: &str,
        created: u64,
    ) -> Result<ProtocolReply> {
        if !identifier(public_id) {
            return Err(Error::Configuration);
        }
        require(value.is_object() && value.get("error").is_none_or(Value::is_null))?;
        require(
            serde_json::to_vec(&value)
                .map_err(|_| Error::Protocol)?
                .len()
                <= self.limits.response_bytes,
        )?;
        let upstream_id = value
            .get("id")
            .map(|v| {
                v.as_str()
                    .ok_or(Error::Protocol)
                    .and_then(|s| crate::attempts::RemoteId::new(s).map_err(|_| Error::Protocol))
            })
            .transpose()?;
        let reported_usage = usage(value.get("usage"), self.endpoint)?;
        let mut body = match self.endpoint {
            ProxyEndpoint::Chat | ProxyEndpoint::Completions => self.decode_choices(&value)?,
            ProxyEndpoint::Responses => self.decode_responses(&value)?,
            ProxyEndpoint::Decisions => self.decode_decisions(&value)?,
        };
        body["id"] = json!(public_id);
        body["model"] = json!(self.public_model);
        body[if self.endpoint == ProxyEndpoint::Responses {
            "created_at"
        } else {
            "created"
        }] = json!(created);
        // Deliberately do not copy upstream mayhem/receipt/cost/provider metadata.
        // Independent metering will populate the public usage/receipt fields.
        // Added public identities/defaults must fit the same result bound. This
        // also makes pre-dispatch storage reservation sufficient for this body.
        require(
            serde_json::to_vec(&body)
                .map_err(|_| Error::Protocol)?
                .len()
                <= self.limits.response_bytes,
        )?;
        let observed_usage = Some(self.metering.observe(&body).map_err(|_| Error::Protocol)?);
        Ok(ProtocolReply {
            body,
            reported_usage,
            observed_usage,
            upstream_id,
        })
    }
    fn calls(&self, raw: &Value, responses: bool) -> Result<Vec<Value>> {
        let calls = raw.as_array().ok_or(Error::Protocol)?;
        require(
            calls.len() <= self.limits.tools
                && (self.parallel || calls.len() <= 1)
                && (!self.forbid_tools || calls.is_empty()),
        )?;
        let mut ids = BTreeSet::new();
        let mut output = Vec::with_capacity(calls.len());
        for call in calls {
            let id = string(call, if responses { "call_id" } else { "id" })?;
            require(identifier(id) && ids.insert(id))?;
            require(
                string(call, "type")?
                    == if responses {
                        "function_call"
                    } else {
                        "function"
                    },
            )?;
            let function = if responses { call } else { &call["function"] };
            let tool_name = string(function, "name")?;
            require(
                self.tools.contains(tool_name)
                    && self.forced_tool.as_ref().is_none_or(|n| n == tool_name),
            )?;
            let arguments = string(function, "arguments")?;
            let parsed: Value = serde_json::from_str(arguments).map_err(|_| Error::Protocol)?;
            require(parsed.is_object())?;
            // Keep the exact arguments. Never repair spelling, paths, JSON or tool names.
            if responses {
                require(call.get("status").is_none_or(|v| v == "completed"))?;
                let item_id = string(call, "id")?;
                require(identifier(item_id))?;
                output.push(json!({"type":"function_call","id":item_id,"call_id":id,"name":tool_name,"arguments":arguments,"status":"completed"}));
            } else {
                output.push(json!({"id":id,"type":"function","function":{"name":tool_name,"arguments":arguments}}));
            }
        }
        Ok(output)
    }
    fn decode_choices(&self, value: &Value) -> Result<Value> {
        let chat = self.endpoint == ProxyEndpoint::Chat;
        let object = if chat {
            "chat.completion"
        } else {
            "text_completion"
        };
        require(value.get("object").is_none_or(|v| v == object))?;
        let choices = value
            .get("choices")
            .and_then(Value::as_array)
            .ok_or(Error::Protocol)?;
        require(choices.len() == self.choices)?;
        let mut indices = BTreeSet::new();
        let mut output = Vec::with_capacity(choices.len());
        for choice in choices {
            let index = choice
                .get("index")
                .and_then(Value::as_u64)
                .ok_or(Error::Protocol)?;
            require(index < self.choices as u64 && indices.insert(index))?;
            let finish = string(choice, "finish_reason")?;
            require(matches!(
                finish,
                "stop" | "length" | "content_filter" | "tool_calls"
            ))?;
            if !chat {
                require(finish != "tool_calls")?;
                output.push(
                    json!({"index":index,"text":string(choice,"text")?,"finish_reason":finish}),
                );
                continue;
            }
            let message = choice
                .get("message")
                .filter(|v| v.is_object())
                .ok_or(Error::Protocol)?;
            require(string(message, "role")? == "assistant")?;
            let mut cleaned = json!({"role":"assistant","content":null});
            for field in ["content", "reasoning_content", "reasoning", "refusal"] {
                if let Some(v) = message.get(field) {
                    require(v.is_null() || v.is_string())?;
                    cleaned[field] = v.clone();
                }
            }
            let calls = message
                .get("tool_calls")
                .filter(|v| !v.is_null())
                .map(|v| self.calls(v, false))
                .transpose()?
                .unwrap_or_default();
            require((finish == "tool_calls") == !calls.is_empty())?;
            let refusal = message
                .get("refusal")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
                || finish == "content_filter";
            require(!self.require_tool || !calls.is_empty() || refusal || finish == "length")?;
            if !calls.is_empty() {
                cleaned["tool_calls"] = json!(calls);
            }
            output.push(json!({"index":index,"message":cleaned,"finish_reason":finish}));
        }
        output.sort_by_key(|v| v["index"].as_u64());
        Ok(json!({"object":object,"choices":output}))
    }
    fn decode_responses(&self, value: &Value) -> Result<Value> {
        require(value.get("object").is_none_or(|v| v == "response"))?;
        let status = string(value, "status")?;
        require(matches!(status, "completed" | "incomplete"))?;
        let raw = value
            .get("output")
            .and_then(Value::as_array)
            .ok_or(Error::Protocol)?;
        require(raw.len() <= self.limits.tools + self.limits.choices)?;
        let mut call_ids = BTreeSet::new();
        let mut output = Vec::with_capacity(raw.len());
        let mut ids = BTreeSet::new();
        let mut refusal = false;
        for item in raw {
            let id = string(item, "id")?;
            require(identifier(id) && ids.insert(id))?;
            let cleaned = self.response_item(item, status)?;
            if cleaned["type"] == "function_call" {
                require(call_ids.insert(string(item, "call_id")?))?;
            }
            refusal |= cleaned
                .get("content")
                .and_then(Value::as_array)
                .is_some_and(|parts| parts.iter().any(|p| p["type"] == "refusal"));
            output.push(cleaned);
        }
        require(!self.require_tool || !call_ids.is_empty() || refusal || status == "incomplete")?;
        require(call_ids.len() <= self.limits.tools && (self.parallel || call_ids.len() <= 1))?;
        let mut result = json!({"object":"response","status":status,"output":output});
        if status == "incomplete" {
            let reason = value
                .pointer("/incomplete_details/reason")
                .and_then(Value::as_str)
                .ok_or(Error::Protocol)?;
            require(matches!(reason, "max_output_tokens" | "content_filter"))?;
            result["incomplete_details"] = json!({"reason":reason});
        }
        Ok(result)
    }
    fn response_item(&self, item: &Value, response_status: &str) -> Result<Value> {
        let id = string(item, "id")?;
        require(identifier(id))?;
        let kind = string(item, "type")?;
        let status = item
            .get("status")
            .map(|_| string(item, "status"))
            .transpose()?;
        require(status.is_none_or(|s| {
            s == "completed"
                || (response_status == "in_progress" && s == "in_progress")
                || (response_status == "incomplete" && s == "incomplete")
        }))?;
        if response_status == "in_progress" {
            require(status.is_none_or(|s| s == "in_progress"))?;
        }
        let status = status.unwrap_or(response_status);
        match kind {
            "function_call" => {
                let call_id = string(item, "call_id")?;
                let name = string(item, "name")?;
                let arguments = string(item, "arguments")?;
                require(
                    !self.forbid_tools
                        && identifier(call_id)
                        && self.tools.contains(name)
                        && self.forced_tool.as_ref().is_none_or(|n| n == name),
                )?;
                if status == "completed" {
                    let parsed: Value =
                        serde_json::from_str(arguments).map_err(|_| Error::Protocol)?;
                    require(parsed.is_object())?;
                }
                Ok(
                    json!({"id":id,"type":kind,"call_id":call_id,"name":name,"arguments":arguments,"status":status}),
                )
            }
            "message" | "reasoning" => {
                let mut result = json!({"id":id,"type":kind});
                if kind == "message" {
                    require(string(item, "role")? == "assistant")?;
                    result["role"] = json!("assistant");
                    result["status"] = json!(status);
                    require(item.get("content").is_some())?;
                } else if item.get("status").is_some() {
                    result["status"] = json!(status);
                }
                for (field, summary) in [("content", false), ("summary", true)] {
                    if summary && kind != "reasoning" {
                        continue;
                    }
                    let parts = match item.get(field) {
                        Some(v) => v.as_array().ok_or(Error::Protocol)?,
                        None => {
                            result[field] = json!([]);
                            continue;
                        }
                    };
                    result[field] = Value::Array(
                        parts
                            .iter()
                            .map(|p| response_part(p, kind, summary))
                            .collect::<Result<Vec<_>>>()?,
                    );
                }
                if kind == "reasoning" {
                    if let Some(v) = item.get("encrypted_content").filter(|v| !v.is_null()) {
                        require(v.is_string())?;
                        result["encrypted_content"] = v.clone();
                    }
                }
                Ok(result)
            }
            _ => Err(Error::Protocol),
        }
    }
    fn decode_decisions(&self, value: &Value) -> Result<Value> {
        let answers = value
            .get("answers")
            .and_then(Value::as_object)
            .ok_or(Error::Protocol)?;
        require(answers.len() == self.questions.len())?;
        let mut clean = Map::new();
        for (id, question) in &self.questions {
            let answer = answers.get(id).ok_or(Error::Protocol)?;
            let mut result = match question {
                Question::Choice(labels) => {
                    require(string(answer, "type")? == "choice")?;
                    let choice = string(answer, "choice")?;
                    require(labels.iter().any(|s| s == choice))?;
                    let probabilities = probabilities(answer, labels)?;
                    json!({"type":"choice","choice":choice,"probabilities":probabilities})
                }
                Question::Score(labels) => {
                    require(string(answer, "type")? == "score")?;
                    let score = answer
                        .get("score")
                        .and_then(Value::as_f64)
                        .ok_or(Error::Protocol)?;
                    require(
                        score.is_finite() && score >= 0. && score <= (labels.len() - 1) as f64,
                    )?;
                    let keys = (0..labels.len()).map(|i| i.to_string()).collect::<Vec<_>>();
                    let probabilities = probabilities(answer, &keys)?;
                    let legend = labels
                        .iter()
                        .enumerate()
                        .map(|(i, label)| (i.to_string(), label.clone()))
                        .collect::<Map<_, _>>();
                    json!({"type":"score","score":score,"legend":legend,"probabilities":probabilities})
                }
                Question::Noul => {
                    require(string(answer, "type")? == "noul")?;
                    json!({"type":"noul","noul":probability(&answer["noul"])?})
                }
            };
            if let Some(confidence) = answer.get("confidence") {
                result["confidence"] = json!(probability(confidence)?);
            }
            if let Some(action) = answer.get("action") {
                result["action"] =
                    json!({"act_probability":probability(&action["act_probability"])?});
            }
            clean.insert(id.clone(), result);
        }
        Ok(json!({"object":"decision","answers":clean}))
    }
}

enum Question {
    Choice(Vec<String>),
    Score(Vec<Value>),
    Noul,
}
fn decision_questions(value: &Value, limits: Limits) -> Result<BTreeMap<String, Question>> {
    let raw = value
        .get("questions")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(None, Code::InvalidRequest))?;
    request(
        !raw.is_empty() && raw.len() <= limits.questions,
        "questions",
    )?;
    let mut questions = BTreeMap::new();
    for (id, q) in raw {
        request(
            identifier(id) && q.get("instructions").is_some(),
            "questions",
        )?;
        let question = match q.get("type").and_then(Value::as_str) {
            Some("choice") => {
                let keys = match q.get("criteria") {
                    Some(Value::Object(o)) => o.keys().cloned().collect::<Vec<_>>(),
                    Some(Value::Array(a)) => (0..a.len()).map(|i| i.to_string()).collect(),
                    _ => return Err(invalid(None, Code::InvalidRequest)),
                };
                request(
                    keys.len() >= 2
                        && keys.len() <= limits.decision_options
                        && keys.iter().all(|s| identifier(s)),
                    "questions",
                )?;
                Question::Choice(keys)
            }
            Some("score") => {
                let labels = q
                    .get("criteria")
                    .and_then(Value::as_array)
                    .ok_or_else(|| invalid(None, Code::InvalidRequest))?;
                request(
                    labels.len() >= 2 && labels.len() <= limits.decision_options,
                    "questions",
                )?;
                Question::Score(labels.clone())
            }
            Some("noul") => Question::Noul,
            _ => return Err(invalid(None, Code::InvalidRequest)),
        };
        questions.insert(id.clone(), question);
    }
    Ok(questions)
}
fn probability(value: &Value) -> Result<f64> {
    let n = value.as_f64().ok_or(Error::Protocol)?;
    require(n.is_finite() && (0.0..=1.0).contains(&n))?;
    Ok(n)
}
fn probabilities(value: &Value, keys: &[String]) -> Result<Value> {
    let raw = value
        .get("probabilities")
        .and_then(Value::as_object)
        .ok_or(Error::Protocol)?;
    require(raw.len() == keys.len())?;
    let mut sum = 0.;
    let mut clean = Map::new();
    for key in keys {
        let p = probability(raw.get(key).ok_or(Error::Protocol)?)?;
        sum += p;
        clean.insert(key.clone(), json!(p));
    }
    // Existing decisions round each probability to four decimals. Check the
    // mathematical accumulated rounding bound, not a model-name exception.
    require((sum - 1.).abs() <= keys.len() as f64 * 0.00005 + 1e-9)?;
    Ok(Value::Object(clean))
}
fn count(value: Option<&Value>) -> Result<u64> {
    value
        .and_then(Value::as_u64)
        .filter(|n| *n <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER)
        .ok_or(Error::Protocol)
}
fn usage(value: Option<&Value>, endpoint: ProxyEndpoint) -> Result<Option<ReportedUsage>> {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    require(value.is_object())?;
    let responses = matches!(
        endpoint,
        ProxyEndpoint::Responses | ProxyEndpoint::Decisions
    );
    let input_key = if responses {
        "input_tokens"
    } else {
        "prompt_tokens"
    };
    let output_key = if responses {
        "output_tokens"
    } else {
        "completion_tokens"
    };
    let input_tokens = count(value.get(input_key).or_else(|| {
        if endpoint == ProxyEndpoint::Decisions {
            value.get("prompt_tokens")
        } else {
            None
        }
    }))?;
    let output_tokens = count(value.get(output_key).or_else(|| {
        if endpoint == ProxyEndpoint::Decisions {
            value.get("completion_tokens")
        } else {
            None
        }
    }))?;
    let total = input_tokens
        .checked_add(output_tokens)
        .filter(|n| *n <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER)
        .ok_or(Error::Protocol)?;
    if let Some(declared) = value.get("total_tokens") {
        require(count(Some(declared))? == total)?;
    }
    let input_details = if responses {
        "input_tokens_details"
    } else {
        "prompt_tokens_details"
    };
    let output_details = if responses {
        "output_tokens_details"
    } else {
        "completion_tokens_details"
    };
    for key in [input_details, output_details] {
        if let Some(v) = value.get(key) {
            require(v.is_null() || v.is_object())?;
        }
    }
    let cached_input_tokens = value
        .get(input_details)
        .and_then(|v| v.get("cached_tokens"))
        .map(|v| count(Some(v)))
        .transpose()?;
    let reasoning_tokens = value
        .get(output_details)
        .and_then(|v| v.get("reasoning_tokens"))
        .map(|v| count(Some(v)))
        .transpose()?;
    require(
        cached_input_tokens.is_none_or(|n| n <= input_tokens)
            && reasoning_tokens.is_none_or(|n| n <= output_tokens),
    )?;
    Ok(Some(ReportedUsage {
        input_tokens,
        output_tokens,
        cached_input_tokens,
        reasoning_tokens,
    }))
}
