//! Stateful, bounded Responses SSE verification for the LLM/function profile.
//! Retain the assembled output, not a chunk history. A terminal snapshot cannot
//! replace previously streamed content. Completion events await final validation.

use super::*;
use crate::worker::Decoded;

#[derive(Default)]
struct PartState {
    text_done: bool,
    part_done: bool,
}
struct Item {
    value: Value,
    parts: BTreeMap<(bool, usize), PartState>,
    arguments_done: bool,
    done: bool,
}
pub(super) struct ResponsesStream<'a> {
    request: &'a Request,
    public_id: String,
    created: u64,
    upstream_id: Option<String>,
    sequence: Option<u64>,
    public_sequence: u64,
    items: Vec<Item>,
    ids: BTreeSet<String>,
    bytes: usize,
    terminal: Option<Value>,
    failed: bool,
}
impl<'a> ResponsesStream<'a> {
    pub(super) fn finish_normalized(self, result: &Value) -> Result<Value> {
        require(!self.failed && self.terminal.is_none())?;
        let final_items = result["output"].as_array().ok_or(Error::Protocol)?;
        require(final_items.len() == self.items.len())?;
        for (item, final_item) in self.items.into_iter().zip(final_items) {
            let mut expected = item.value;
            if let Some(status) = final_item.get("status") {
                expected["status"] = status.clone();
            }
            // These fields may occur only in the provider's withheld done
            // frames. Already-delivered text, arguments, annotations and nonempty
            // logprobs must remain identical to the final normalized result.
            if expected["type"] == "reasoning" {
                expected
                    .as_object_mut()
                    .unwrap()
                    .remove("encrypted_content");
                if let Some(value) = final_item.get("encrypted_content") {
                    expected["encrypted_content"] = value.clone();
                }
            }
            if let Some(parts) = expected.get_mut("content").and_then(Value::as_array_mut) {
                let final_parts = final_item["content"].as_array().ok_or(Error::Protocol)?;
                require(parts.len() == final_parts.len())?;
                for (part, final_part) in parts.iter_mut().zip(final_parts) {
                    if part["type"] == "output_text"
                        && part
                            .get("logprobs")
                            .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty))
                    {
                        if let Some(logprobs) = final_part.get("logprobs") {
                            part["logprobs"] = logprobs.clone();
                        }
                    }
                }
            }
            require(&expected == final_item)?;
        }
        Ok(result.clone())
    }
    pub(super) fn new(request: &'a Request, public_id: &str, created: u64) -> Result<Self> {
        if !request.streaming
            || request.endpoint != ProxyEndpoint::Responses
            || !identifier(public_id)
        {
            return Err(Error::Configuration);
        }
        Ok(Self {
            request,
            public_id: public_id.into(),
            created,
            upstream_id: None,
            sequence: None,
            public_sequence: 0,
            items: Vec::new(),
            ids: BTreeSet::new(),
            bytes: 0,
            terminal: None,
            failed: false,
        })
    }
    pub(super) fn is_done(&self) -> bool {
        self.terminal.is_some() && !self.failed
    }
    pub(super) fn upstream_id(&self) -> Option<&str> {
        self.upstream_id.as_deref()
    }
    pub(super) fn push(&mut self, frame: Decoded) -> Result<Option<Value>> {
        if self.failed {
            return Err(Error::Protocol);
        }
        let result = self.push_inner(frame);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn push_inner(&mut self, frame: Decoded) -> Result<Option<Value>> {
        let Decoded::Sse { event, data, .. } = frame else {
            return Err(Error::Protocol);
        };
        self.bytes = self.bytes.checked_add(data.len()).ok_or(Error::Protocol)?;
        require(self.bytes <= self.request.limits.response_bytes && self.terminal.is_none())?;
        let raw: Value = serde_json::from_str(&data).map_err(|_| Error::Protocol)?;
        let kind = string(&raw, "type")?;
        require(event.is_empty() || event == "message" || event == kind)?;
        let sequence = raw
            .get("sequence_number")
            .and_then(Value::as_u64)
            .ok_or(Error::Protocol)?;
        require(
            sequence <= 9_007_199_254_740_991 && self.sequence.is_none_or(|last| sequence > last),
        )?;
        self.sequence = Some(sequence);
        if kind == "response.created" {
            require(self.upstream_id.is_none() && self.items.is_empty())?;
            let response = &raw["response"];
            let id = string(response, "id")?;
            require(identifier(id))?;
            self.upstream_id = Some(id.into());
        } else {
            require(self.upstream_id.is_some())?;
        }

        let output = match kind {
            "response.created" | "response.in_progress" | "response.queued" => {
                let r = &raw["response"];
                self.response_identity(r)?;
                let status = string(r, "status")?;
                require(matches!(status, "in_progress" | "queued"))?;
                require(
                    r.get("output")
                        .and_then(Value::as_array)
                        .is_some_and(Vec::is_empty),
                )?;
                require(self.items.is_empty())?;
                Some(
                    json!({"response":{"id":self.public_id,"object":"response","created_at":self.created,
                    "model":self.request.public_model,"status":status,"output":[],"error":null,"usage":null,"incomplete_details":null}}),
                )
            }
            "response.output_item.added" => {
                let index = index(&raw, "output_index")?;
                require(
                    index == self.items.len()
                        && index < self.request.limits.tools + self.request.limits.choices,
                )?;
                let value = self.request.response_item(&raw["item"], "in_progress")?;
                require(self.ids.insert(string(&value, "id")?.into()))?;
                require(value.get("status").is_none_or(|v| v == "in_progress"))?;
                let tool = value["type"] == "function_call";
                if tool {
                    let count = self
                        .items
                        .iter()
                        .filter(|i| i.value["type"] == "function_call")
                        .count()
                        + 1;
                    require(
                        count <= self.request.limits.tools && (self.request.parallel || count <= 1),
                    )?;
                    require(
                        !self
                            .items
                            .iter()
                            .any(|i| i.value.get("call_id") == value.get("call_id")),
                    )?;
                }
                // Parts are introduced by their own events; accepting hidden initial
                // parts would skip their ordered identities and completion checks.
                for field in ["content", "summary"] {
                    require(
                        value
                            .get(field)
                            .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty)),
                    )?;
                }
                let mut provisional = value.clone();
                if let Some(o) = provisional.as_object_mut() {
                    o.remove("encrypted_content");
                }
                self.items.push(Item {
                    value,
                    parts: BTreeMap::new(),
                    arguments_done: false,
                    done: false,
                });
                Some(json!({"output_index":index,"item":provisional}))
            }
            "response.content_part.added" | "response.reasoning_summary_part.added" => {
                let summary = kind == "response.reasoning_summary_part.added";
                let (item, output_index) = self.item_mut(&raw)?;
                let position = index(
                    &raw,
                    if summary {
                        "summary_index"
                    } else {
                        "content_index"
                    },
                )?;
                let part = response_part(&raw["part"], string(&item.value, "type")?, summary)?;
                let field = if summary { "summary" } else { "content" };
                let parts = item
                    .value
                    .get_mut(field)
                    .and_then(Value::as_array_mut)
                    .ok_or(Error::Protocol)?;
                require(position == parts.len())?;
                parts.push(part.clone());
                item.parts.insert((summary, position), PartState::default());
                let mut fields =
                    json!({"output_index":output_index,"item_id":item.value["id"],"part":part});
                fields[if summary {
                    "summary_index"
                } else {
                    "content_index"
                }] = json!(position);
                Some(fields)
            }
            "response.output_text.delta"
            | "response.reasoning_text.delta"
            | "response.reasoning_summary_text.delta"
            | "response.refusal.delta" => {
                let (summary, expected, key) = part_event(kind)?;
                let (item, output_index) = self.item_mut(&raw)?;
                let position = index(
                    &raw,
                    if summary {
                        "summary_index"
                    } else {
                        "content_index"
                    },
                )?;
                let state = item
                    .parts
                    .get(&(summary, position))
                    .ok_or(Error::Protocol)?;
                require(!state.text_done && !state.part_done)?;
                let part = &mut item.value[if summary { "summary" } else { "content" }][position];
                require(part["type"] == expected)?;
                let delta = string(&raw, "delta")?;
                let Value::String(text) = &mut part[key] else {
                    return Err(Error::Protocol);
                };
                text.push_str(delta);
                let mut fields =
                    json!({"output_index":output_index,"item_id":item.value["id"],"delta":delta});
                fields[if summary {
                    "summary_index"
                } else {
                    "content_index"
                }] = json!(position);
                // Logprobs are data, not independent token metering.
                if let Some(logprobs) = raw.get("logprobs") {
                    require(expected == "output_text")?;
                    let logs = response_logprobs(logprobs)?;
                    let part = &mut item.value["content"][position];
                    part.as_object_mut()
                        .unwrap()
                        .entry("logprobs")
                        .or_insert_with(|| json!([]))
                        .as_array_mut()
                        .ok_or(Error::Protocol)?
                        .extend(logs.as_array().unwrap().iter().cloned());
                    fields["logprobs"] = logs;
                }
                Some(fields)
            }
            "response.output_text.done"
            | "response.reasoning_text.done"
            | "response.reasoning_summary_text.done"
            | "response.refusal.done" => {
                let (summary, expected, key) = part_event(kind)?;
                let (item, _) = self.item_mut(&raw)?;
                let position = index(
                    &raw,
                    if summary {
                        "summary_index"
                    } else {
                        "content_index"
                    },
                )?;
                let state = item
                    .parts
                    .get_mut(&(summary, position))
                    .ok_or(Error::Protocol)?;
                require(!state.text_done && !state.part_done)?;
                let part = &mut item.value[if summary { "summary" } else { "content" }][position];
                require(
                    part["type"] == expected && part[key].as_str() == Some(string(&raw, key)?),
                )?;
                if let Some(logprobs) = raw.get("logprobs") {
                    let logs = response_logprobs(logprobs)?;
                    // Some upstreams emit logprobs only with the final text event.
                    require(
                        part.get("logprobs")
                            .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty) || v == &logs),
                    )?;
                    part["logprobs"] = logs;
                }
                state.text_done = true;
                None
            }
            "response.content_part.done" | "response.reasoning_summary_part.done" => {
                let summary = kind == "response.reasoning_summary_part.done";
                let (item, _) = self.item_mut(&raw)?;
                let position = index(
                    &raw,
                    if summary {
                        "summary_index"
                    } else {
                        "content_index"
                    },
                )?;
                let part = response_part(&raw["part"], string(&item.value, "type")?, summary)?;
                let state = item
                    .parts
                    .get_mut(&(summary, position))
                    .ok_or(Error::Protocol)?;
                require(state.text_done && !state.part_done)?;
                require(item.value[if summary { "summary" } else { "content" }][position] == part)?;
                state.part_done = true;
                None
            }
            "response.output_text.annotation.added" => {
                let (item, output_index) = self.item_mut(&raw)?;
                let position = index(&raw, "content_index")?;
                let state = item.parts.get(&(false, position)).ok_or(Error::Protocol)?;
                require(!state.part_done)?;
                let part = &mut item.value["content"][position];
                require(part["type"] == "output_text")?;
                let annotation = response_annotation(&raw["annotation"])?;
                let annotations = part["annotations"].as_array_mut().ok_or(Error::Protocol)?;
                let annotation_index = index(&raw, "annotation_index")?;
                require(annotation_index == annotations.len())?;
                annotations.push(annotation.clone());
                Some(
                    json!({"output_index":output_index,"item_id":item.value["id"],"content_index":position,"annotation_index":annotation_index,"annotation":annotation}),
                )
            }
            "response.function_call_arguments.delta" | "response.function_call_arguments.done" => {
                let (item, output_index) = self.item_mut(&raw)?;
                require(item.value["type"] == "function_call" && !item.arguments_done)?;
                if let Some(name) = raw.get("name") {
                    require(name == &item.value["name"])?;
                }
                if kind.ends_with(".delta") {
                    let delta = string(&raw, "delta")?;
                    let Value::String(arguments) = &mut item.value["arguments"] else {
                        return Err(Error::Protocol);
                    };
                    arguments.push_str(delta);
                    Some(
                        json!({"output_index":output_index,"item_id":item.value["id"],"delta":delta}),
                    )
                } else {
                    require(string(&raw, "arguments")? == string(&item.value, "arguments")?)?;
                    item.arguments_done = true;
                    None
                }
            }
            "response.output_item.done" => {
                let index = index(&raw, "output_index")?;
                let value = self.request.response_item(&raw["item"], "incomplete")?;
                let item = self.items.get_mut(index).ok_or(Error::Protocol)?;
                require(!item.done)?;
                let status = value.get("status").and_then(Value::as_str);
                require(status.is_none_or(|s| matches!(s, "completed" | "incomplete")))?;
                require(item.parts.values().all(|p| p.text_done && p.part_done))?;
                require(value["type"] != "function_call" || item.arguments_done)?;
                let mut expected = item.value.clone();
                if let Some(status) = value.get("status") {
                    expected["status"] = status.clone();
                }
                // This opaque continuation is finalized at item.done, not streamed.
                if value["type"] == "reasoning" {
                    expected
                        .as_object_mut()
                        .unwrap()
                        .remove("encrypted_content");
                    if let Some(encrypted) = value.get("encrypted_content") {
                        expected["encrypted_content"] = encrypted.clone();
                    }
                }
                require(expected == value)?;
                item.value = value;
                item.done = true;
                None
            }
            "response.completed" | "response.incomplete" => {
                let response = &raw["response"];
                self.response_identity(response)?;
                let incomplete = kind == "response.incomplete";
                require(
                    response["status"]
                        == if incomplete {
                            "incomplete"
                        } else {
                            "completed"
                        },
                )?;
                let normalized = self.request.decode_responses(response)?;
                let final_items = normalized["output"].as_array().ok_or(Error::Protocol)?;
                require(final_items.len() == self.items.len())?;
                for (item, final_item) in self.items.iter_mut().zip(final_items) {
                    if !item.done {
                        require(incomplete)?;
                        if item.value.get("status").is_some() {
                            item.value["status"] = json!("incomplete");
                        }
                    }
                    require(item.value == *final_item)?;
                }
                usage(response.get("usage"), ProxyEndpoint::Responses)?;
                let mut result = normalized;
                result["id"] = response["id"].clone();
                if let Some(u) = response.get("usage") {
                    result["usage"] = u.clone();
                }
                self.terminal = Some(result);
                None
            }
            // The worker extracts safe failure fields before these reach us. A
            // missing/malformed error body must still never become successful EOF.
            "response.failed" | "error" => return Err(Error::Protocol),
            _ => return Err(Error::Protocol),
        };
        Ok(output.map(|mut fields| {
            fields["type"] = json!(kind);
            fields["sequence_number"] = json!(self.public_sequence);
            self.public_sequence += 1;
            fields
        }))
    }
    fn response_identity(&self, value: &Value) -> Result<()> {
        require(value["object"] == "response" && value.get("error").is_none_or(Value::is_null))?;
        require(self.upstream_id.as_deref() == Some(string(value, "id")?))
    }
    fn item_mut(&mut self, event: &Value) -> Result<(&mut Item, usize)> {
        let index = index(event, "output_index")?;
        let item = self.items.get_mut(index).ok_or(Error::Protocol)?;
        require(!item.done && string(&item.value, "id")? == string(event, "item_id")?)?;
        Ok((item, index))
    }
    pub(super) fn finish(self) -> Result<Value> {
        require(!self.failed)?;
        self.terminal.ok_or(Error::Protocol)
    }
}
fn index(value: &Value, key: &str) -> Result<usize> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|v| usize::try_from(v).ok())
        .ok_or(Error::Protocol)
}
fn part_event(kind: &str) -> Result<(bool, &'static str, &'static str)> {
    if kind.starts_with("response.output_text.") {
        Ok((false, "output_text", "text"))
    } else if kind.starts_with("response.reasoning_text.") {
        Ok((false, "reasoning_text", "text"))
    } else if kind.starts_with("response.reasoning_summary_text.") {
        Ok((true, "summary_text", "text"))
    } else if kind.starts_with("response.refusal.") {
        Ok((false, "refusal", "refusal"))
    } else {
        Err(Error::Protocol)
    }
}
