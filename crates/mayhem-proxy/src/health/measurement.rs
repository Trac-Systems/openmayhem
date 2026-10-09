use super::*;
use serde_json::Value;

#[derive(Clone, Debug, Serialize)]
pub struct Measurement {
    #[serde(skip)]
    pub(super) observed_at: Instant,
    #[serde(skip)]
    pub(super) native_observed_at: Option<Instant>,
    pub headers_ms: Option<u64>,
    pub first_output_ms: Option<u64>,
    pub total_ms: u64,
    pub native_tok_s: Option<f64>,
    pub native_interval_tokens: Option<u64>,
    pub native_interval_us: Option<u64>,
    pub tokenizer: Option<Digest>,
    pub reported_output_tokens: Option<u64>,
    pub meaningful_updates: u64,
}
/// Local timing record. Optional native measurement holds bounded output in
/// memory until validation/encoding; no prompt, URL, credential or per-token I/O.
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
    native: Option<native::Capture>,
    completed: Option<Instant>,
    finished: bool,
}
/// Captured network timing, published after any required durable local transition.
pub(crate) struct Success {
    sample: Sample,
    measurement: Measurement,
}
impl Success {
    pub(crate) fn publish(mut self) {
        self.sample.finished = true;
        self.sample
            .monitor
            .finish(&self.sample, self.measurement, Outcome::Success);
    }
}
impl Sample {
    pub(crate) fn recovery_is_current(&self) -> Result<bool> {
        self.monitor.recovery_is_current(self)
    }
    /// Probe admission/decoder startup precedes actual network timing. Keeping its
    /// recovery latch must not attribute our fsync or process startup to the model.
    pub(crate) fn start_execution(&mut self) -> Result<()> {
        if self.headers.is_some() || self.first.is_some() || self.native_first.is_some() {
            return Err(Error::Invalid);
        }
        self.start = Instant::now();
        Ok(())
    }
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
            native: None,
            completed: None,
            finished: false,
        }
    }
    pub fn headers(&mut self) {
        self.headers.get_or_insert_with(Instant::now);
    }
    /// Supply only already validated normalized deltas. Role-only changes,
    /// keepalives, usage counters and finish markers are not model output.
    pub fn delta(&mut self, value: &Value) {
        self.delta_at(value, Instant::now());
    }
    pub(crate) fn delta_at(&mut self, value: &Value, at: Instant) {
        if let Some(native) = &mut self.native {
            native.delta(value, at)
        }
        if !meaningful(value) {
            return;
        }
        self.first.get_or_insert(at);
        self.updates = self.updates.saturating_add(1);
    }
    /// Trusted local tokenizer's cumulative count at observed output boundaries.
    /// Never pass billing units, SSE event counts or upstream-reported usage here.
    /// Tokenizer identity must stay fixed; buffered output with one timestamp
    /// cannot establish a generation interval or certify a throughput floor.
    pub fn native_progress(&mut self, tokenizer: Digest, tokens: u64) -> Result<()> {
        if self.native.is_some()
            || !self.class.streaming
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
    pub(crate) fn use_native(&mut self, source: &native::Source) {
        if self.class.streaming && self.native_first.is_none() {
            self.native = source.capture()
        }
    }
    pub(crate) fn backpressure(&self) -> Option<Arc<std::sync::atomic::AtomicBool>> {
        self.native.as_ref().map(native::Capture::backpressure)
    }
    pub(crate) async fn finish_native(&mut self) {
        self.completed.get_or_insert_with(Instant::now);
        if let Some(capture) = self.native.take() {
            if let Some(evidence) = capture.finish().await {
                self.tokenizer = Some(evidence.digest);
                self.native_first = Some((evidence.first, 0));
                self.native_last = Some((evidence.last, evidence.tokens));
            }
        }
    }
    pub(crate) fn network_complete(&mut self, at: Instant) {
        self.completed = Some(at);
    }
    fn measurement(&self, reported_output_tokens: Option<u64>) -> Measurement {
        let now = self.completed.unwrap_or_else(Instant::now);
        let rate = match (self.native_first, self.native_last) {
            (Some((a, first)), Some((b, last))) if b > a && last > first => {
                Some((last - first) as f64 / b.duration_since(a).as_secs_f64())
            }
            _ => None,
        };
        Measurement {
            observed_at: now,
            native_observed_at: self.native_last.map(|(at, _)| at),
            headers_ms: self.headers.map(|t| millis(t.duration_since(self.start))),
            first_output_ms: self.first.map(|t| millis(t.duration_since(self.start))),
            total_ms: millis(now.duration_since(self.start)),
            native_tok_s: rate.filter(|v| v.is_finite()),
            native_interval_tokens: rate.and_then(|_| {
                self.native_last
                    .zip(self.native_first)
                    .map(|((_, last), (_, first))| last - first)
            }),
            native_interval_us: rate.and_then(|_| {
                self.native_last
                    .zip(self.native_first)
                    .map(|((last, _), (first, _))| {
                        last.duration_since(first).as_micros().min(u64::MAX as u128) as u64
                    })
            }),
            tokenizer: self.tokenizer.clone(),
            reported_output_tokens,
            meaningful_updates: self.updates,
        }
    }
    /// Called after complete endpoint/schema verification, not HTTP200 alone.
    pub fn success(self, reported_output_tokens: Option<u64>) {
        self.prepare_success(reported_output_tokens).publish();
    }
    pub(crate) fn prepare_success(self, reported_output_tokens: Option<u64>) -> Success {
        let observed = self.measurement(
            reported_output_tokens.filter(|v| *v <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER),
        );
        Success {
            sample: self,
            measurement: observed,
        }
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
pub(crate) fn meaningful(value: &Value) -> bool {
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
