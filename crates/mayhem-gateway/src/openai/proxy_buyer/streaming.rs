//! One attached observer of the original purchase. Provisional events are never
//! durable fragments or financial evidence. Replays use the ordinary job route.
use super::*;
use axum::{
    http::{header, HeaderValue},
    response::sse::{Event, KeepAlive, Sse},
};
use futures_util::stream;
use serde_json::json;
use std::convert::Infallible;

type Reply = oneshot::Receiver<Result<StoredGatewayJob, ApiError>>;

/// The foreground controller's confirmation timeout does not make a verified
/// answer fail. Continue observing the original durable owner while its existing
/// recovery task settles it. This observer never sends, signs, or reconciles work.
pub(super) struct SettlementObserver {
    state: SharedState,
    binding: Binding,
    halted: watch::Receiver<bool>,
}
impl SettlementObserver {
    pub(super) fn new(state: SharedState, binding: Binding, halted: watch::Receiver<bool>) -> Self {
        Self {
            state,
            binding,
            halted,
        }
    }

    async fn wait(&mut self) -> Option<StoredGatewayJob> {
        use crate::job_store::proxy::ProxyPhase;
        loop {
            if *self.halted.borrow() {
                return None;
            }
            let jobs = self.state.jobs.clone();
            let binding = self.binding.clone();
            let observation = tokio::select! {
                biased;
                _ = stop_signal(&mut self.halted) => return None,
                value = storage(move || {
                    let jobs = jobs.lock().map_err(|_| unavailable())?;
                    // One indexed borrowed record: no expiry sweep, ledger read,
                    // result clone, or growing history traversal on a poll.
                    let job = jobs.proxy_evidence_record(&binding.job_id, now_secs()).ok_or_else(unavailable)?;
                    let proxy = job.proxy.as_ref().ok_or_else(unavailable)?;
                    if job.model != binding.model || job.request_fingerprint != binding.fingerprint
                        || job.owner_token_id.as_deref() != Some(binding.token.token_id.as_str())
                        || proxy.identity() != &binding.identity {
                        return Err(unavailable());
                    }
                    match (job.status, proxy.phase()) {
                        (GatewayJobStatus::Completed, ProxyPhase::Closed) if job.result.is_some() => Ok(Some(job.clone())),
                        (GatewayJobStatus::ReconciliationPending,
                            ProxyPhase::OutputRetained | ProxyPhase::ClosurePendingBudget) => Ok(None),
                        // Unverified output and actual terminal failures still
                        // expose recovery. A pending budget commit cannot grant success.
                        _ => Err(unavailable()),
                    }
                }) => value,
            };
            match observation {
                Ok(Some(job)) => return Some(job),
                Err(_) => return None,
                Ok(None) => (),
            }
            tokio::select! {
                biased;
                _ = stop_signal(&mut self.halted) => return None,
                _ = tokio::time::sleep(Duration::from_secs(2)) => (),
            }
        }
    }
}

pub(super) fn channel(
    runtime: &Runtime,
) -> Result<
    (
        buyer_controller::StreamSender,
        buyer_controller::StreamReceiver,
        OwnedSemaphorePermit,
    ),
    ApiError,
> {
    // Execution and observers have separate caps: a completed purchase must not
    // free the slot that accounts for a slow reader's queued/final response.
    let permit = runtime
        .streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| busy())?;
    let total = runtime.controller.response_byte_limit();
    let (send, receive) =
        buyer_controller::stream_channel(buyer_controller::StreamLimits::for_response_bytes(total))
            .map_err(|_| unavailable())?;
    Ok((send, receive, permit))
}

struct Observer {
    events: Option<buyer_controller::StreamReceiver>,
    reply: Option<Reply>,
    endpoint: ProxyEndpoint,
    final_events: Option<FinalEvents>,
    sequence: u64,
    ended: bool,
    job: String,
    settlement: SettlementObserver,
    _permit: OwnedSemaphorePermit,
}

pub(super) fn response(
    id: &str,
    endpoint: ProxyEndpoint,
    events: buyer_controller::StreamReceiver,
    reply: Reply,
    permit: OwnedSemaphorePermit,
    settlement: SettlementObserver,
) -> Response {
    let observer = Observer {
        events: Some(events),
        reply: Some(reply),
        endpoint,
        final_events: None,
        sequence: 0,
        ended: false,
        job: id.into(),
        settlement,
        _permit: permit,
    };
    // No forwarding task/queue. Dropping this body drops the receiver; the
    // controller notices, cancels delivery, and joins its retained purchase.
    let body = stream::unfold(observer, |mut state| async move {
        if state.ended {
            return None;
        }
        if let Some(events) = &mut state.events {
            if let Some(event) = events.recv().await {
                let parsed: Value =
                    serde_json::from_slice(event.json_bytes()).expect("controller encoded JSON");
                let mut frame = Event::default();
                if state.endpoint == ProxyEndpoint::Responses {
                    let kind = parsed["type"].as_str().expect("verified Responses event");
                    frame = frame.event(kind);
                    state.sequence = parsed["sequence_number"]
                        .as_u64()
                        .expect("verified sequence")
                        + 1;
                }
                // One bounded frame is copied into Axum; the channel event keeps
                // its byte/accounting permits until this conversion is complete.
                let frame =
                    frame.data(std::str::from_utf8(event.json_bytes()).expect("JSON UTF-8"));
                return Some((Ok::<_, Infallible>(frame), state));
            }
            state.events = None;
            let job = match state.reply.take().expect("one owner reply").await {
                Ok(Ok(job))
                    if job.status == GatewayJobStatus::Completed && job.result.is_some() =>
                {
                    Some(job)
                }
                _ => state.settlement.wait().await,
            };
            match job {
                Some(job) if job.status == GatewayJobStatus::Completed && job.result.is_some() => {
                    // Completed is possible only after verified output, exact
                    // canonical closure and durable key-budget settlement.
                    state.final_events = Some(FinalEvents::new(
                        state.endpoint,
                        job.result.unwrap(),
                        state.sequence,
                    ));
                }
                _ => {
                    state.ended = true;
                    let frame = Event::default().event("error").json_data(json!({"error":{
                        "type":"proxy_recovery_required", "code":"proxy_recovery_required",
                        "message":"The original proxy purchase is not complete; retrieve its job before retrying.",
                        "job_id":state.job, "recovery_url":format!("/v1/jobs/{}", state.job)
                    }})).expect("fixed error JSON");
                    // No [DONE], finish_reason or response.completed on uncertainty.
                    return Some((Ok(frame), state));
                }
            }
        }
        match state.final_events.as_mut().and_then(Iterator::next) {
            Some(frame) => Some((Ok(frame), state)),
            None => None,
        }
    });
    let mut response = Sse::new(body)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    attach_gateway_job_headers(&mut response, id);
    response
}

/// Derive withheld completion metadata from the verified retained response,
/// lazily: no fragment history or terminal-event vector proportional to output.
struct FinalEvents {
    endpoint: ProxyEndpoint,
    result: Value,
    sequence: u64,
    item: usize,
    group: usize,
    part: usize,
    step: usize,
    done: bool,
}
impl FinalEvents {
    fn new(endpoint: ProxyEndpoint, result: Value, sequence: u64) -> Self {
        Self {
            endpoint,
            result,
            sequence,
            item: 0,
            group: 0,
            part: 0,
            step: 0,
            done: false,
        }
    }
    fn event(&mut self, kind: &str, mut fields: Value) -> Event {
        fields["type"] = json!(kind);
        fields["sequence_number"] = json!(self.sequence);
        self.sequence += 1;
        Event::default()
            .event(kind)
            .json_data(fields)
            .expect("retained JSON")
    }
}
impl Iterator for FinalEvents {
    type Item = Event;
    fn next(&mut self) -> Option<Event> {
        if self.done {
            return None;
        }
        if self.endpoint != ProxyEndpoint::Responses {
            if self.step == 1 {
                self.done = true;
                return Some(Event::default().data("[DONE]"));
            }
            self.step = 1;
            let mut result = std::mem::take(&mut self.result);
            if self.endpoint == ProxyEndpoint::Chat {
                result["object"] = json!("chat.completion.chunk");
            }
            if let Some(choices) = result["choices"].as_array_mut() {
                for choice in choices {
                    let index = choice["index"].clone();
                    let reason = choice["finish_reason"].clone();
                    *choice = if self.endpoint == ProxyEndpoint::Chat {
                        json!({"index":index,"delta":{},"finish_reason":reason})
                    } else {
                        json!({"index":index,"text":"","finish_reason":reason})
                    };
                }
            }
            return Some(Event::default().json_data(result).expect("retained JSON"));
        }
        loop {
            let outputs = self.result["output"].as_array().expect("verified output");
            if self.item >= outputs.len() {
                self.done = true;
                let kind = if self.result["status"] == "incomplete" {
                    "response.incomplete"
                } else {
                    "response.completed"
                };
                let result = std::mem::take(&mut self.result);
                return Some(self.event(kind, json!({"response":result})));
            }
            let item = &outputs[self.item];
            let output_index = self.item;
            if item["type"] == "function_call" && self.group == 0 {
                self.group = 2;
                let fields = json!({"output_index":output_index,"item_id":item["id"],"arguments":item["arguments"]});
                return Some(self.event("response.function_call_arguments.done", fields));
            }
            if self.group < 2 {
                let summary = self.group == 1;
                let group = if summary { "summary" } else { "content" };
                let Some(part) = item[group].as_array().and_then(|p| p.get(self.part)) else {
                    self.group += 1;
                    self.part = 0;
                    self.step = 0;
                    continue;
                };
                let mut fields = json!({"output_index":output_index,"item_id":item["id"]});
                fields[if summary {
                    "summary_index"
                } else {
                    "content_index"
                }] = json!(self.part);
                let kind;
                if self.step == 0 {
                    self.step = 1;
                    let (name, key) = match part["type"].as_str() {
                        Some("output_text") => ("response.output_text.done", "text"),
                        Some("refusal") => ("response.refusal.done", "refusal"),
                        Some("summary_text") => ("response.reasoning_summary_text.done", "text"),
                        Some("reasoning_text") => ("response.reasoning_text.done", "text"),
                        _ => unreachable!("verified response part"),
                    };
                    kind = name;
                    fields[key] = part[key].clone();
                    if let Some(logs) = part.get("logprobs") {
                        fields["logprobs"] = logs.clone();
                    }
                } else {
                    self.step = 0;
                    self.part += 1;
                    kind = if summary {
                        "response.reasoning_summary_part.done"
                    } else {
                        "response.content_part.done"
                    };
                    fields["part"] = part.clone();
                }
                return Some(self.event(kind, fields));
            }
            let fields = json!({"output_index":output_index,"item":item});
            self.item += 1;
            self.group = 0;
            self.part = 0;
            self.step = 0;
            return Some(self.event("response.output_item.done", fields));
        }
    }
}

#[cfg(test)]
mod tests;
