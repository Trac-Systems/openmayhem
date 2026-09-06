use super::{ChatCompletionRequest, GatewaySessionError, ToolCallOutput, Value, json};

/// Presentation only: receipt accounting continues to use the exact evidence
/// bytes, including native delimiters, and never charges this second view twice.
#[derive(Default)]
pub(super) struct ReasoningStream {
    pending: String,
    ended: bool,
}

impl ReasoningStream {
    pub fn push(&mut self, text: &str) -> String {
        if self.ended {
            return String::new();
        }
        self.pending.push_str(text);
        let mut out = String::new();
        const MARKERS: [&str; 4] = ["<think>", "</think>", "<|channel>thought\n", "<channel|>"];
        loop {
            let found = MARKERS
                .iter()
                .filter_map(|marker| self.pending.find(marker).map(|index| (index, *marker)))
                .min_by_key(|(index, _)| *index);
            if let Some((index, marker)) = found {
                out.push_str(&self.pending[..index]);
                self.pending.drain(..index + marker.len());
                if marker == "</think>" || marker == "<channel|>" {
                    self.pending.clear();
                    self.ended = true;
                    break;
                }
            } else {
                let retained = MARKERS
                    .iter()
                    .flat_map(|marker| (1..marker.len()).map(move |n| &marker[..n]))
                    .filter(|prefix| self.pending.ends_with(prefix))
                    .map(str::len)
                    .max()
                    .unwrap_or(0);
                let end = self.pending.len() - retained;
                out.push_str(&self.pending[..end]);
                self.pending.drain(..end);
                break;
            }
        }
        out
    }

    pub fn finish(&mut self) -> String {
        std::mem::take(&mut self.pending)
    }
}

pub(super) fn reasoning_text(evidence: &str) -> String {
    let mut stream = ReasoningStream::default();
    let mut text = stream.push(evidence);
    text.push_str(&stream.finish());
    text
}

#[derive(Default)]
pub(super) struct ToolStream {
    calls: Vec<ToolCallOutput>,
    bytes: usize,
    finalized: bool,
}

impl ToolStream {
    pub fn push(
        &mut self,
        frame: &Value,
        request: &ChatCompletionRequest,
        limit: usize,
    ) -> Result<Vec<Value>, GatewaySessionError> {
        let Some(raw) = frame.get("tool_calls_delta") else {
            return Ok(Vec::new());
        };
        let deltas = raw
            .as_array()
            .ok_or_else(|| error("tool_calls_delta must be an array"))?;
        if self.finalized && !deltas.is_empty() {
            return Err(error("tool deltas followed final tool calls"));
        }
        let mut output = Vec::new();
        for delta in deltas {
            let index = delta
                .get("index")
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| error("tool delta missing index"))?;
            if index > self.calls.len() {
                return Err(error("tool delta skipped a call index"));
            }
            let arguments = delta
                .get("arguments")
                .and_then(Value::as_str)
                .ok_or_else(|| error("tool delta missing arguments"))?;
            let mut public = json!({"index":index,"function":{"arguments":arguments}});
            if index == self.calls.len() {
                if index >= 1024 || (index > 0 && request.parallel_tool_calls == Some(false)) {
                    return Err(error("too many streamed tool calls"));
                }
                let id = delta
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| error("first tool delta missing id"))?;
                let name = delta
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| error("first tool delta missing name"))?;
                if !request.tools.as_ref().into_iter().flatten().any(|tool| {
                    tool.pointer("/function/name")
                        .or_else(|| tool.get("name"))
                        .and_then(Value::as_str)
                        == Some(name)
                }) {
                    return Err(error("provider streamed an unadvertised tool"));
                }
                if self.calls.iter().any(|call| call.id == id) {
                    return Err(error("duplicate streamed tool id"));
                }
                self.bytes = self
                    .bytes
                    .saturating_add(id.len())
                    .saturating_add(name.len());
                public["id"] = json!(id);
                public["type"] = json!("function");
                public["function"]["name"] = json!(name);
                self.calls.push(ToolCallOutput {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    arguments: String::new(),
                });
            } else if delta.get("id").is_some() || delta.get("name").is_some() {
                return Err(error("tool delta attempted to replace call identity"));
            }
            self.bytes = self.bytes.saturating_add(arguments.len());
            if self.bytes > limit {
                return Err(error("streamed tool arguments exceeded session text limit"));
            }
            self.calls[index].arguments.push_str(arguments);
            output.push(public);
        }
        Ok(output)
    }

    pub fn finish(&mut self, calls: &[ToolCallOutput]) -> Result<Vec<Value>, GatewaySessionError> {
        if self.calls.len() > calls.len() {
            return Err(error("streamed tool call missing from final output"));
        }
        let mut deltas = Vec::new();
        for (index, call) in calls.iter().enumerate() {
            if let Some(prior) = self.calls.get(index) {
                if prior.id != call.id || prior.name != call.name {
                    return Err(error("final tool identity differs from streamed call"));
                }
                if let Some(tail) = call.arguments.strip_prefix(&prior.arguments) {
                    if !tail.is_empty() {
                        deltas.push(json!({"index":index,"function":{"arguments":tail}}));
                    }
                } else {
                    let prior = serde_json::from_str::<Value>(&prior.arguments)
                        .map_err(|_| error("streamed tool arguments are incomplete"))?;
                    let final_args = serde_json::from_str::<Value>(&call.arguments)
                        .map_err(|_| error("final tool arguments are invalid"))?;
                    if prior != final_args {
                        return Err(error("final tool arguments differ from streamed call"));
                    }
                }
            } else {
                deltas.push(json!({"index":index,"id":call.id,"type":"function","function":{"name":call.name,"arguments":call.arguments}}));
            }
        }
        self.finalized = true;
        Ok(deltas)
    }
}

fn error(message: &str) -> GatewaySessionError {
    GatewaySessionError::new(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_is_available_before_the_closing_marker_without_changing_evidence() {
        for raw in [
            "<think>Consider é🦀 step by step.</think>\n\n",
            "Consider é🦀 step by step.</think>\n\n",
            "<|channel>thought\nConsider é🦀 step by step.<channel|>\n",
        ] {
            for split in raw.char_indices().map(|(index, _)| index) {
                let mut stream = ReasoningStream::default();
                let first = stream.push(&raw[..split]);
                let second = stream.push(&raw[split..]);
                assert_eq!(
                    format!("{first}{second}{}", stream.finish()),
                    "Consider é🦀 step by step."
                );
            }
            let mut stream = ReasoningStream::default();
            let until = raw.find("step by").unwrap();
            assert!(stream.push(&raw[..until]).contains("Consider"));
            assert_eq!(reasoning_text(raw), "Consider é🦀 step by step.");
            assert_eq!(
                super::super::metered_output_units("", raw, &[]),
                (raw.len() as u64).div_ceil(4)
            );
        }
    }

    fn request() -> ChatCompletionRequest {
        let mut request = super::super::tests::test_chat_request("model");
        request.tools = Some(vec![json!({"type":"function","function":{"name":"write"}})]);
        request
    }

    #[test]
    fn tool_arguments_stream_once_and_are_bound_to_the_final_call() {
        let mut stream = ToolStream::default();
        let request = request();
        let first = stream.push(&json!({"tool_calls_delta":[{"index":0,"id":"call-x","name":"write","arguments":"{\"content\":\"hello"}]}), &request, 1024).unwrap();
        assert_eq!(first[0]["function"]["arguments"], "{\"content\":\"hello");
        let second = stream
            .push(
                &json!({"tool_calls_delta":[{"index":0,"arguments":" world\"}"}]}),
                &request,
                1024,
            )
            .unwrap();
        assert!(second[0].get("id").is_none());
        let call = ToolCallOutput {
            id: "call-x".into(),
            name: "write".into(),
            arguments: "{\"content\":\"hello world\"}".into(),
        };
        assert!(stream.finish(&[call.clone()]).unwrap().is_empty());
        let mut wrong = call;
        wrong.arguments = "{}".into();
        assert!(stream.finish(&[wrong]).is_err());
    }

    #[test]
    fn tool_previews_enforce_bounds_identity_and_advertised_names() {
        let request = request();
        for delta in [
            json!({"index":1,"id":"x","name":"write","arguments":"{"}),
            json!({"index":0,"id":"x","name":"unknown","arguments":"{"}),
            json!({"index":0,"name":"write","arguments":"{"}),
            json!({"index":0,"id":"x","name":"write","arguments":"x".repeat(1024)}),
        ] {
            assert!(
                ToolStream::default()
                    .push(&json!({"tool_calls_delta":[delta]}), &request, 1024)
                    .is_err()
            );
        }
        let mut stream = ToolStream::default();
        stream
            .push(
                &json!({"tool_calls_delta":[{"index":0,"id":"x","name":"write","arguments":"{"}]}),
                &request,
                1024,
            )
            .unwrap();
        assert!(stream.finish(&[]).is_err());
    }
}
