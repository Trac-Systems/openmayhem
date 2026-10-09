//! Closed-vocabulary public errors. Arbitrary vendor messages, headers, URLs and
//! parameter values never become diagnostic strings or automatically authorize
//! retry/settlement. A documented adapter can add stronger execution evidence.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, time::SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    InvalidRequest,
    InvalidSchema,
    UnsupportedControl,
    ContextTooLarge,
    UpstreamBusy,
    UpstreamRateLimited,
    UpstreamAuthentication,
    UpstreamPaymentRequired,
    UpstreamModelUnavailable,
    UpstreamEndpointUnavailable,
    UpstreamUnavailable,
    UpstreamProtocol,
    UpstreamTimeout,
    LocalCapacity,
    DestinationRejected,
    RequestTooLarge,
    ResponseTooLarge,
    AdmissionUnavailable,
    RecoveryRequired,
    RequestCancelled,
    ProviderUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Request,
    Model,
    Connection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    BeforeDispatch,
    Connecting,
    Dispatch,
    ResponseHeaders,
    ResponseBody,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Execution {
    NotDispatched,
    /// Sent, but rejected before generation under an explicitly selected and
    /// verified upstream contract. This is NOT a financial closure or retry permit.
    Rejected,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryAdvice {
    CorrectRequest,
    OperatorAction,
    SafeBeforeDispatch,
    RecoverSameAttempt,
}

#[derive(Clone, Debug, Serialize)]
pub struct Failure {
    pub code: Code,
    pub scope: Scope,
    pub stage: Stage,
    pub execution: Execution,
    pub upstream_status: Option<u16>,
    pub upstream_code: Option<&'static str>,
    pub parameter: Option<&'static str>,
    pub retry_after_ms: Option<u64>,
}

impl Failure {
    pub(crate) fn verified_rejection(&self) -> bool {
        self.execution == Execution::Rejected
            && self.stage == Stage::ResponseHeaders
            && self.code == Code::UpstreamBusy
            && self.scope == Scope::Connection
            && self.upstream_status == Some(503)
            && matches!(
                self.upstream_code,
                Some("vllm_queue_overflow" | "vllm_prefill_backlog")
            )
            && self.parameter.is_none()
    }
    pub fn new(code: Code, scope: Scope, stage: Stage, execution: Execution) -> Self {
        Self {
            code,
            scope,
            stage,
            execution,
            upstream_status: None,
            upstream_code: None,
            parameter: None,
            retry_after_ms: None,
        }
    }

    pub fn retry_advice(&self) -> RetryAdvice {
        match self.code {
            Code::InvalidRequest
            | Code::InvalidSchema
            | Code::UnsupportedControl
            | Code::ContextTooLarge
            | Code::RequestTooLarge => RetryAdvice::CorrectRequest,
            Code::UpstreamAuthentication
            | Code::UpstreamPaymentRequired
            | Code::UpstreamEndpointUnavailable
            | Code::DestinationRejected => RetryAdvice::OperatorAction,
            _ if self.execution == Execution::NotDispatched => RetryAdvice::SafeBeforeDispatch,
            _ => RetryAdvice::RecoverSameAttempt,
        }
    }

    /// Translation recommendation, not a replacement for the gateway envelope.
    /// Upstream 401/402 must never impersonate buyer auth or balance failure.
    pub fn public_status(&self) -> u16 {
        match self.code {
            Code::InvalidRequest
            | Code::InvalidSchema
            | Code::UnsupportedControl
            | Code::ContextTooLarge => 400,
            Code::RequestTooLarge => 413,
            Code::RecoveryRequired | Code::RequestCancelled => 409,
            Code::AdmissionUnavailable | Code::ProviderUnavailable => 503,
            Code::LocalCapacity
            | Code::UpstreamBusy
            | Code::UpstreamRateLimited
            | Code::UpstreamModelUnavailable
            | Code::UpstreamUnavailable => 503,
            Code::UpstreamTimeout => 504,
            _ => 502,
        }
    }

    pub fn message(&self) -> &'static str {
        match self.code {
            Code::InvalidRequest => "The upstream rejected the request arguments.",
            Code::InvalidSchema => {
                "The requested JSON schema is invalid or exceeds this adapter's supported limits."
            }
            Code::UnsupportedControl => {
                "The selected upstream does not support a requested setting."
            }
            Code::ContextTooLarge => "The request exceeds the selected upstream's context limit.",
            Code::UpstreamBusy => "The selected upstream is busy.",
            Code::UpstreamRateLimited => "The provider's upstream connection is rate limited.",
            Code::UpstreamAuthentication => "The provider's upstream credentials were rejected.",
            Code::UpstreamPaymentRequired => {
                "The provider's upstream account cannot fund this request."
            }
            Code::UpstreamModelUnavailable => "The selected upstream model is unavailable.",
            Code::UpstreamEndpointUnavailable => "The configured upstream endpoint is unavailable.",
            Code::UpstreamUnavailable => "The provider's upstream connection failed.",
            Code::UpstreamProtocol => "The upstream returned an invalid or unsupported response.",
            Code::UpstreamTimeout => {
                "The upstream exceeded its configured connection or inactivity limit."
            }
            Code::LocalCapacity => "This proxy connection has no free local dispatch slot.",
            Code::DestinationRejected => {
                "The upstream destination is not authorized by the local connection policy."
            }
            Code::RequestTooLarge => "The request exceeds this connection's configured byte limit.",
            Code::ResponseTooLarge => {
                "The upstream response exceeds this connection's configured byte limit."
            }
            Code::AdmissionUnavailable => "The agreed request has no verified financial admission yet. Recover the same request.",
            Code::RecoveryRequired => "This request requires recovery of its saved state; do not submit a replacement inference.",
            Code::RequestCancelled => "Cancellation was requested. Upstream execution and payment may still require reconciliation.",
            Code::ProviderUnavailable => "The provider cannot complete this control operation right now. Recover the same request.",
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}
impl std::error::Error for Failure {}

/// Documented OpenAI-shaped stream error envelopes. Ordinary model refusals or
/// text mentioning an error are not infrastructure failures.
pub fn openai_stream_error(body: &[u8]) -> Option<Failure> {
    if body.len() > 16 * 1024 {
        return None;
    }
    let value: Value = serde_json::from_slice(body).ok()?;
    let normalized = if value.get("type").and_then(Value::as_str) == Some("response.failed") {
        serde_json::json!({"error":value.pointer("/response/error")?})
    } else if value.get("error").is_some_and(Value::is_object) {
        value
    } else if value.get("type").and_then(Value::as_str) == Some("error") {
        serde_json::json!({"error": value})
    } else {
        return None;
    };
    let bytes = serde_json::to_vec(&normalized).ok()?;
    let mut failure = openai_error(200, &bytes, None, SystemTime::now());
    failure.stage = Stage::ResponseBody;
    failure.upstream_status = None;
    Some(failure)
}

/// OpenAI-shaped error decoding is a compatibility profile, not a general body
/// heuristic. Other protocols will supply their own documented error extraction.
pub fn openai_error(
    status: u16,
    body: &[u8],
    retry_after: Option<&str>,
    now: SystemTime,
) -> Failure {
    let parsed = (body.len() <= 16 * 1024)
        .then(|| serde_json::from_slice::<Value>(body).ok())
        .flatten();
    let error = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .filter(|v| v.is_object());
    let raw = error.and_then(|e| {
        e.get("code")
            .and_then(Value::as_str)
            .or_else(|| e.get("type").and_then(Value::as_str))
    });
    let known = raw.and_then(safe_upstream_code);
    let (code, scope) = match (status, known) {
        (401 | 403, _) => (Code::UpstreamAuthentication, Scope::Connection),
        (402, _) => (Code::UpstreamPaymentRequired, Scope::Connection),
        (_, Some("insufficient_quota")) => (Code::UpstreamPaymentRequired, Scope::Connection),
        (_, Some("invalid_api_key")) => (Code::UpstreamAuthentication, Scope::Connection),
        (_, Some("context_length_exceeded")) => (Code::ContextTooLarge, Scope::Request),
        (_, Some("unsupported_parameter")) => (Code::UnsupportedControl, Scope::Request),
        (_, Some("model_not_found")) => (Code::UpstreamModelUnavailable, Scope::Model),
        (429, _) | (_, Some("rate_limit_exceeded")) => {
            (Code::UpstreamRateLimited, Scope::Connection)
        }
        (503, _) | (_, Some("overloaded_error")) => (Code::UpstreamBusy, Scope::Model),
        (400 | 422, _) | (_, Some("invalid_request_error")) => {
            (Code::InvalidRequest, Scope::Request)
        }
        (404 | 405, _) => (Code::UpstreamEndpointUnavailable, Scope::Connection),
        (408 | 504, _) => (Code::UpstreamTimeout, Scope::Model),
        (500..=599, _) | (_, Some("server_error")) => (Code::UpstreamUnavailable, Scope::Model),
        _ => (Code::UpstreamProtocol, Scope::Model),
    };
    let mut failure = Failure::new(code, scope, Stage::ResponseHeaders, Execution::Unknown);
    failure.upstream_status = Some(status);
    failure.upstream_code = known;
    failure.parameter = error
        .and_then(|e| e.get("param"))
        .and_then(Value::as_str)
        .and_then(safe_parameter);
    failure.retry_after_ms = retry_after.and_then(|value| parse_retry_after(value, now));
    failure
}

pub(crate) fn safe_upstream_code(value: &str) -> Option<&'static str> {
    match Some(value) {
        Some("vllm_queue_overflow") => Some("vllm_queue_overflow"),
        Some("vllm_prefill_backlog") => Some("vllm_prefill_backlog"),
        Some("context_length_exceeded") => Some("context_length_exceeded"),
        Some("unsupported_parameter") => Some("unsupported_parameter"),
        Some("invalid_request_error") => Some("invalid_request_error"),
        Some("model_not_found") => Some("model_not_found"),
        Some("rate_limit_exceeded") => Some("rate_limit_exceeded"),
        Some("insufficient_quota") => Some("insufficient_quota"),
        Some("invalid_api_key") => Some("invalid_api_key"),
        Some("overloaded_error") => Some("overloaded_error"),
        Some("server_error") => Some("server_error"),
        _ => None,
    }
}

pub(crate) fn safe_parameter(value: &str) -> Option<&'static str> {
    // Only known public root fields, never user-controlled schema/property names.
    match Some(value) {
        Some("messages") => Some("messages"),
        Some("input") => Some("input"),
        Some("tools") => Some("tools"),
        Some("response_format") => Some("response_format"),
        Some("max_tokens") => Some("max_tokens"),
        Some("temperature") => Some("temperature"),
        Some("model") => Some("model"),
        Some("prompt") => Some("prompt"),
        Some("state") => Some("state"),
        Some("questions") => Some("questions"),
        Some("tool_choice") => Some("tool_choice"),
        Some("parallel_tool_calls") => Some("parallel_tool_calls"),
        Some("stream") => Some("stream"),
        Some("max_completion_tokens") => Some("max_completion_tokens"),
        Some("max_output_tokens") => Some("max_output_tokens"),
        Some("reasoning") => Some("reasoning"),
        Some("text") => Some("text"),
        Some("previous_response_id") => Some("previous_response_id"),
        Some("conversation") => Some("conversation"),
        Some("background") => Some("background"),
        Some("store") => Some("store"),
        _ => None,
    }
}

pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<u64> {
    if value.len() > 128 {
        return None;
    }
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return value.parse::<u64>().ok()?.checked_mul(1000);
    }
    let target = httpdate::parse_http_date(value).ok()?;
    Some(
        target
            .duration_since(now)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .ok()?,
    )
}
