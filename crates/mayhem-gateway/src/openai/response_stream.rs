use super::{
    json, make_id, now_secs, stream, AtomicU64, BTreeMap, Ordering, SseEventStream, StreamExt,
    Value, VecDeque,
};

fn response_item_id(prefix: &str) -> String {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        make_id(prefix),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    )
}

pub(super) fn from_chat(events: SseEventStream, model: String) -> SseEventStream {
    let mut adapter = ResponseStream::new(model);
    let pending = adapter.start();
    Box::pin(stream::unfold(
        (events, adapter, pending),
        |(mut events, mut adapter, mut pending)| async move {
            loop {
                if let Some(event) = pending.pop_front() {
                    return Some((Some(event), (events, adapter, pending)));
                }
                if adapter.terminal {
                    return None;
                }
                pending = match events.next().await {
                    Some(Some(chunk)) => adapter.push(chunk),
                    Some(None) => adapter.finish(),
                    None => adapter.fail("stream ended without a terminal event"),
                };
            }
        },
    ))
}

struct ResponseStream {
    response: Value,
    output: Vec<Value>,
    text_index: Option<usize>,
    calls: BTreeMap<u64, usize>,
    finish_reason: Option<String>,
    sequence: u64,
    terminal: bool,
}

impl ResponseStream {
    fn new(model: String) -> Self {
        Self {
            response: json!({
                "id": response_item_id("resp"), "object": "response", "created_at": now_secs(),
                "model": model, "status": "in_progress", "output": [],
                "error": null, "incomplete_details": null, "usage": null,
            }),
            output: Vec::new(),
            text_index: None,
            calls: BTreeMap::new(),
            finish_reason: None,
            sequence: 0,
            terminal: false,
        }
    }

    fn event(&mut self, kind: &str, mut fields: Value) -> Value {
        fields["type"] = json!(kind);
        fields["sequence_number"] = json!(self.sequence);
        self.sequence += 1;
        fields
    }

    fn start(&mut self) -> VecDeque<Value> {
        VecDeque::from([
            self.event("response.created", json!({"response": self.response})),
            self.event("response.in_progress", json!({"response": self.response})),
        ])
    }

    fn push(&mut self, chunk: Value) -> VecDeque<Value> {
        if let Some(error) = chunk.get("error") {
            return self.fail(
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("stream failed"),
            );
        }
        if let Some(usage) = chunk.get("usage").filter(|usage| !usage.is_null()) {
            self.response["usage"] = json!({
                "input_tokens": usage["prompt_tokens"],
                "output_tokens": usage["completion_tokens"],
                "total_tokens": usage["total_tokens"],
                "input_tokens_details": {"cached_tokens": usage.pointer("/prompt_tokens_details/cached_tokens").and_then(Value::as_u64).unwrap_or(0)},
                "output_tokens_details": {"reasoning_tokens": usage.pointer("/completion_tokens_details/reasoning_tokens").and_then(Value::as_u64).unwrap_or(0)},
            });
        }
        if let Some(meta) = chunk.get("mayhem") {
            self.response["mayhem"] = meta.clone();
        }
        let mut events = VecDeque::new();
        for choice in chunk
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                self.finish_reason = Some(reason.to_owned());
            }
            let delta = &choice["delta"];
            if let Some(text) = delta
                .get("content")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                let index = match self.text_index {
                    Some(index) => index,
                    None => {
                        let index = self.output.len();
                        let item = json!({"id": response_item_id("msg"), "type": "message", "role": "assistant", "status": "in_progress", "content": []});
                        events.push_back(self.event(
                            "response.output_item.added",
                            json!({"output_index": index, "item": item}),
                        ));
                        let part = json!({"type": "output_text", "text": "", "annotations": [], "logprobs": []});
                        events.push_back(self.event("response.content_part.added", json!({"output_index": index, "item_id": item["id"], "content_index": 0, "part": part})));
                        let mut item = item;
                        item["content"] = json!([part]);
                        self.output.push(item);
                        self.text_index = Some(index);
                        index
                    }
                };
                if let Value::String(content) = &mut self.output[index]["content"][0]["text"] {
                    content.push_str(text);
                }
                events.push_back(self.event("response.output_text.delta", json!({"output_index": index, "item_id": self.output[index]["id"], "content_index": 0, "delta": text, "logprobs": []})));
            }
            for call in delta
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let call_index = call.get("index").and_then(Value::as_u64).unwrap_or(0);
                let index = match self.calls.get(&call_index).copied() {
                    Some(index) => index,
                    None => {
                        let Some(call_id) = call
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|id| !id.is_empty())
                        else {
                            return self.fail("function call is missing its id");
                        };
                        let Some(name) = call
                            .pointer("/function/name")
                            .and_then(Value::as_str)
                            .filter(|name| !name.is_empty())
                        else {
                            return self.fail("function call is missing its name");
                        };
                        let index = self.output.len();
                        let item = json!({"id": response_item_id("fc"), "type": "function_call", "call_id": call_id, "name": name, "arguments": "", "status": "in_progress"});
                        events.push_back(self.event(
                            "response.output_item.added",
                            json!({"output_index": index, "item": item}),
                        ));
                        self.output.push(item);
                        self.calls.insert(call_index, index);
                        index
                    }
                };
                if let Some(arguments) = call
                    .pointer("/function/arguments")
                    .and_then(Value::as_str)
                    .filter(|arguments| !arguments.is_empty())
                {
                    if let Value::String(prior) = &mut self.output[index]["arguments"] {
                        prior.push_str(arguments);
                    }
                    events.push_back(self.event("response.function_call_arguments.delta", json!({"output_index": index, "item_id": self.output[index]["id"], "delta": arguments})));
                }
            }
        }
        events
    }

    fn finish(&mut self) -> VecDeque<Value> {
        let Some(reason) = self.finish_reason.as_deref() else {
            return self.fail("stream ended before a deliverable result");
        };
        let incomplete = match reason {
            "length" => Some("max_output_tokens"),
            "content_filter" => Some("content_filter"),
            _ => None,
        };
        let status = if incomplete.is_some() {
            "incomplete"
        } else {
            "completed"
        };
        let mut events = VecDeque::new();
        for index in 0..self.output.len() {
            self.output[index]["status"] = json!(status);
            let item = self.output[index].clone();
            if item["type"] == "message" {
                let part = &item["content"][0];
                events.push_back(self.event("response.output_text.done", json!({"output_index": index, "item_id": item["id"], "content_index": 0, "text": part["text"], "logprobs": []})));
                events.push_back(self.event("response.content_part.done", json!({"output_index": index, "item_id": item["id"], "content_index": 0, "part": part})));
            } else {
                events.push_back(self.event("response.function_call_arguments.done", json!({"output_index": index, "item_id": item["id"], "name": item["name"], "arguments": item["arguments"]})));
            }
            events.push_back(self.event(
                "response.output_item.done",
                json!({"output_index": index, "item": item}),
            ));
        }
        self.response["status"] = json!(status);
        self.response["output"] = json!(self.output);
        if let Some(reason) = incomplete {
            self.response["incomplete_details"] = json!({"reason": reason});
        }
        self.terminal = true;
        events.push_back(self.event(
            if incomplete.is_some() {
                "response.incomplete"
            } else {
                "response.completed"
            },
            json!({"response": self.response}),
        ));
        events
    }

    fn fail(&mut self, message: &str) -> VecDeque<Value> {
        self.terminal = true;
        self.response["status"] = json!("failed");
        self.response["error"] = json!({"code": "server_error", "message": message});
        for item in &mut self.output {
            item["status"] = json!("incomplete");
        }
        self.response["output"] = json!(self.output);
        VecDeque::from([self.event("response.failed", json!({"response": self.response}))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(delta: Value, finish: Value) -> Value {
        json!({"choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
    }

    #[test]
    fn text_and_function_events_have_stable_ids_sequences_and_final_billing_metadata() {
        let mut adapter = ResponseStream::new("model".to_owned());
        let mut events = adapter.start();
        events.extend(adapter.push(chunk(json!({"content": "Hello "}), Value::Null)));
        events.extend(adapter.push(chunk(json!({"content": "world"}), Value::Null)));
        events.extend(adapter.push(chunk(json!({"tool_calls": [
            {"index": 0, "id": "call_1", "function": {"name": "weather", "arguments": "{\"city\":"}},
            {"index": 1, "id": "call_2", "function": {"name": "clock", "arguments": "{}"}},
        ]}), Value::Null)));
        events.extend(adapter.push(chunk(
            json!({"tool_calls": [{"index": 0, "function": {"arguments": "\"Berlin\"}"}}]}),
            json!("tool_calls"),
        )));
        let meta = json!({"billable": true, "receipt": {"session_id": "server-session", "au_owed_cum": "123"}});
        events.extend(adapter.push(json!({"choices": [], "usage": {"prompt_tokens": 7, "completion_tokens": 5, "total_tokens": 12}, "mayhem": meta})));
        assert!(!events
            .iter()
            .any(|event| event["type"] == "response.completed"));
        events.extend(adapter.finish());
        for (sequence, event) in events.iter().enumerate() {
            assert_eq!(event["sequence_number"], sequence);
        }
        let response = &events.back().unwrap()["response"];
        assert_eq!(response["status"], "completed");
        assert_eq!(response["mayhem"], meta);
        assert_eq!(response["usage"]["total_tokens"], 12);
        assert_eq!(response["output"][0]["content"][0]["text"], "Hello world");
        assert_eq!(response["output"][1]["arguments"], "{\"city\":\"Berlin\"}");
        assert_eq!(response["output"][1]["call_id"], "call_1");
        assert_eq!(response["output"][2]["call_id"], "call_2");
        assert_ne!(response["output"][1]["id"], response["output"][2]["id"]);
        for event in events.iter().filter(|event| event.get("item_id").is_some()) {
            let index = event["output_index"].as_u64().unwrap() as usize;
            assert_eq!(event["item_id"], response["output"][index]["id"]);
        }
        let text_types = events
            .iter()
            .filter_map(|event| event["type"].as_str())
            .filter(|kind| kind.contains("output_text") || kind.contains("content_part"))
            .collect::<Vec<_>>();
        assert_eq!(
            text_types,
            [
                "response.content_part.added",
                "response.output_text.delta",
                "response.output_text.delta",
                "response.output_text.done",
                "response.content_part.done"
            ]
        );
        let done = events
            .iter()
            .find(|event| event["type"] == "response.function_call_arguments.done")
            .unwrap();
        assert_eq!(done["name"], "weather");
    }

    #[test]
    fn incomplete_and_failed_streams_never_claim_completed() {
        for (reason, expected) in [
            ("length", "max_output_tokens"),
            ("content_filter", "content_filter"),
        ] {
            let mut adapter = ResponseStream::new("model".to_owned());
            adapter.push(chunk(json!({"content": "partial"}), json!(reason)));
            let events = adapter.finish();
            assert_eq!(events.back().unwrap()["type"], "response.incomplete");
            assert_eq!(
                events.back().unwrap()["response"]["incomplete_details"]["reason"],
                expected
            );
            assert!(!events
                .iter()
                .any(|event| event["type"] == "response.completed"));
        }
        let mut adapter = ResponseStream::new("model".to_owned());
        adapter.push(chunk(json!({"content": "partial"}), Value::Null));
        let events = adapter.push(json!({"error": {"message": "receipt acknowledgement failed"}}));
        assert_eq!(events.back().unwrap()["type"], "response.failed");
        assert_eq!(
            events.back().unwrap()["response"]["output"][0]["status"],
            "incomplete"
        );
        assert!(adapter.terminal);
    }

    #[tokio::test]
    async fn source_eof_without_done_is_failed_and_responses_omit_done_sentinel() {
        let source = Box::pin(stream::iter([Some(chunk(
            json!({"content": "partial"}),
            Value::Null,
        ))]));
        let events = from_chat(source, "model".to_owned())
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Option::is_some));
        assert_eq!(
            events.last().unwrap().as_ref().unwrap()["type"],
            "response.failed"
        );
    }
}
