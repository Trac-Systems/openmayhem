use super::*;
use serde_json::Value;

#[derive(Clone, Debug, Serialize)]
pub struct Measurement {
    pub headers_ms: Option<u64>,
    pub first_output_ms: Option<u64>,
    pub total_ms: u64,
    pub native_tok_s: Option<f64>,
    pub tokenizer: Option<Digest>,
    pub reported_output_tokens: Option<u64>,
    pub meaningful_updates: u64,
}
/// Local, constant-size timing record. It retains no prompt, model text, tool
/// arguments, URL or credential, and writes no journal entries on token delivery.
pub struct Sample {
    monitor: Monitor,
    pub(super) route: Digest,
    pub(super) class: Class,
    pub(super) generation: (u64, u64),
    pub(super) sequence: u64,
    pub(super) recovery: bool,
    start: Instant,
    headers: Option<Instant>,
    first: Option<Instant>,
    updates: u64,
    tokenizer: Option<Digest>,
    native_first: Option<(Instant, u64)>,
    native_last: Option<(Instant, u64)>,
    finished: bool,
}
impl Sample {
    pub(super) fn new(
        monitor: Monitor,
        route: Digest,
        class: Class,
        generation: (u64, u64),
        sequence: u64,
        recovery: bool,
        start: Instant,
    ) -> Self {
        Self {
            monitor,
            route,
            class,
            generation,
            sequence,
            recovery,
            start,
            headers: None,
            first: None,
            updates: 0,
            tokenizer: None,
            native_first: None,
            native_last: None,
            finished: false,
        }
    }
    pub fn headers(&mut self) {
        self.headers.get_or_insert_with(Instant::now);
    }
    /// Supply only already validated normalized deltas. Role-only changes,
    /// keepalives, usage counters and finish markers are not model output.
    pub fn delta(&mut self, value: &Value) {
        if !meaningful(value) {
            return;
        }
        self.first.get_or_insert_with(Instant::now);
        self.updates = self.updates.saturating_add(1);
    }
    /// Trusted local tokenizer's cumulative count at observed output boundaries.
    /// Never pass billing units, SSE event counts or upstream-reported usage here.
    /// Tokenizer identity must stay fixed; buffered output with one timestamp
    /// cannot establish a generation interval or certify a throughput floor.
    pub fn native_progress(&mut self, tokenizer: Digest, tokens: u64) -> Result<()> {
        if !self.class.streaming
            || self.first.is_none()
            || tokens > mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER
            || self.tokenizer.as_ref().is_some_and(|old| old != &tokenizer)
            || self.native_last.is_some_and(|(_, old)| tokens < old)
        {
            return Err(Error::Invalid);
        }
        self.tokenizer = Some(tokenizer);
        let now = Instant::now();
        self.native_first.get_or_insert((now, tokens));
        self.native_last = Some((now, tokens));
        Ok(())
    }
    fn measurement(&self, reported_output_tokens: Option<u64>) -> Measurement {
        let now = Instant::now();
        let rate = match (self.native_first, self.native_last) {
            (Some((a, first)), Some((b, last))) if b > a && last > first => {
                Some((last - first) as f64 / b.duration_since(a).as_secs_f64())
            }
            _ => None,
        };
        Measurement {
            headers_ms: self.headers.map(|t| millis(t.duration_since(self.start))),
            first_output_ms: self.first.map(|t| millis(t.duration_since(self.start))),
            total_ms: millis(now.duration_since(self.start)),
            native_tok_s: rate.filter(|v| v.is_finite()),
            tokenizer: self.tokenizer.clone(),
            reported_output_tokens,
            meaningful_updates: self.updates,
        }
    }
    /// Called after complete endpoint/schema verification, not HTTP200 alone.
    pub fn success(mut self, reported_output_tokens: Option<u64>) {
        let observed = self.measurement(
            reported_output_tokens.filter(|v| *v <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER),
        );
        self.finished = true;
        self.monitor.finish(&self, observed, Outcome::Success);
    }
    pub fn failure(mut self, failure: Failure) {
        let observed = self.measurement(None);
        self.finished = true;
        self.monitor
            .finish(&self, observed, Outcome::Failure(failure));
    }
}
impl Drop for Sample {
    fn drop(&mut self) {
        if !self.finished {
            self.monitor
                .finish(self, self.measurement(None), Outcome::Abandoned)
        }
        // Dropping this observer never releases a durable request/probe lease.
    }
}
fn text(v: Option<&Value>) -> bool {
    v.and_then(Value::as_str).is_some_and(|s| !s.is_empty())
}
fn meaningful(value: &Value) -> bool {
    if value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| {
            matches!(
                kind,
                "response.output_text.delta"
                    | "response.reasoning_text.delta"
                    | "response.reasoning_summary_text.delta"
                    | "response.refusal.delta"
                    | "response.function_call_arguments.delta"
            )
        })
    {
        return text(value.get("delta"));
    }
    value
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| {
            choices.iter().any(|choice| {
                if text(choice.get("text")) {
                    return true;
                }
                let Some(delta) = choice.get("delta") else {
                    return false;
                };
                ["content", "reasoning", "reasoning_content", "refusal"]
                    .iter()
                    .any(|key| text(delta.get(key)))
                    || delta
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .is_some_and(|tools| {
                            tools.iter().any(|tool| {
                                tool.get("function").is_some_and(|f| {
                                    text(f.get("name")) || text(f.get("arguments"))
                                })
                            })
                        })
            })
        })
}
