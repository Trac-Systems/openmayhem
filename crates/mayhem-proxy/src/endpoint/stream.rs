//! Incremental Chat/Completions response assembly. Deltas are provisional; finish
//! reasons and [DONE] are withheld until terminal framing, complete tool JSON and semantic
//! verification succeed. Never retain a history of chunks or splice providers.

use super::*;
use crate::worker::Decoded;

#[derive(Default)]
struct Tool {
    id: String,
    name: String,
    arguments: String,
}
#[derive(Default)]
struct Choice {
    content: Option<String>,
    reasoning: Option<String>,
    reasoning_content: Option<String>,
    refusal: Option<String>,
    tools: BTreeMap<u64, Tool>,
    finish: Option<String>,
}
pub struct Stream<'a> {
    kind: Kind<'a>,
}
enum Kind<'a> {
    Completion(CompletionStream<'a>),
    Responses(super::responses_stream::ResponsesStream<'a>),
}
impl<'a> Stream<'a> {
    pub fn new(request: &'a Request, public_id: &str, created: u64) -> Result<Self> {
        Ok(Self {
            kind: if request.endpoint == ProxyEndpoint::Responses {
                Kind::Responses(super::responses_stream::ResponsesStream::new(
                    request, public_id, created,
                )?)
            } else {
                Kind::Completion(CompletionStream::new(request, public_id, created)?)
            },
        })
    }
    pub fn is_done(&self) -> bool {
        match &self.kind {
            Kind::Completion(s) => s.is_done(),
            Kind::Responses(s) => s.is_done(),
        }
    }
    pub fn upstream_id(&self) -> Option<&str> {
        match &self.kind {
            Kind::Completion(s) => s.upstream_id.as_deref(),
            Kind::Responses(s) => s.upstream_id(),
        }
    }
    /// Deltas are provisional, including function arguments. Completion events
    /// belong to the final verified/durable reply, not this consumer callback.
    pub fn push(&mut self, frame: Decoded) -> Result<Option<Value>> {
        match &mut self.kind {
            Kind::Completion(s) => s.push(frame),
            Kind::Responses(s) => s.push(frame),
        }
    }
    pub fn finish(self) -> Result<Value> {
        match self.kind {
            Kind::Completion(s) => s.finish(),
            Kind::Responses(s) => s.finish(),
        }
    }
    pub(super) fn finish_normalized(self, result: &Value) -> Result<Value> {
        match self.kind {
            Kind::Completion(s) => s.finish_normalized(result),
            Kind::Responses(s) => s.finish_normalized(result),
        }
    }
}
struct CompletionStream<'a> {
    request: &'a Request,
    public_id: String,
    created: u64,
    upstream_id: Option<String>,
    choices: BTreeMap<u64, Choice>,
    usage: Option<Value>,
    bytes: usize,
    done: bool,
    failed: bool,
}
impl<'a> CompletionStream<'a> {
    fn finish_normalized(mut self, result: &Value) -> Result<Value> {
        require(!self.failed && !self.done)?;
        let choices = result["choices"].as_array().ok_or(Error::Protocol)?;
        require(choices.len() == self.request.choices)?;
        for (index, final_choice) in choices.iter().enumerate() {
            require(final_choice["index"].as_u64() == Some(index as u64))?;
            let choice = self.choices.entry(index as u64).or_default();
            require(choice.finish.is_none())?;
            choice.finish = Some(string(final_choice, "finish_reason")?.into());
        }
        self.done = true;
        self.finish()
    }
    pub fn new(request: &'a Request, public_id: &str, created: u64) -> Result<Self> {
        if !request.streaming
            || !identifier(public_id)
            || !matches!(
                request.endpoint,
                ProxyEndpoint::Chat | ProxyEndpoint::Completions
            )
        {
            return Err(Error::Configuration);
        }
        Ok(Self {
            request,
            public_id: public_id.into(),
            created,
            upstream_id: None,
            choices: BTreeMap::new(),
            usage: None,
            bytes: 0,
            done: false,
            failed: false,
        })
    }
    /// Only provisional deltas. A caller must not execute a tool from a fragment.
    pub fn is_done(&self) -> bool {
        self.done && !self.failed
    }
    pub fn push(&mut self, frame: Decoded) -> Result<Option<Value>> {
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
        require(matches!(event.as_str(), "" | "message"))?;
        self.bytes = self.bytes.checked_add(data.len()).ok_or(Error::Protocol)?;
        require(self.bytes <= self.request.limits.response_bytes && !self.done)?;
        if data.trim() == "[DONE]" {
            require(
                self.choices.len() == self.request.choices
                    && self.choices.values().all(|c| c.finish.is_some()),
            )?;
            self.done = true;
            return Ok(None);
        }
        let chunk: Value = serde_json::from_str(&data).map_err(|_| Error::Protocol)?;
        require(chunk.is_object() && chunk.get("error").is_none_or(Value::is_null))?;
        if let Some(id) = chunk.get("id") {
            let id = id
                .as_str()
                .filter(|s| identifier(s))
                .ok_or(Error::Protocol)?;
            require(self.upstream_id.as_ref().is_none_or(|prior| prior == id))?;
            self.upstream_id = Some(id.into());
        }
        let chat = self.request.endpoint == ProxyEndpoint::Chat;
        require(chunk.get("object").is_none_or(|v| {
            v == if chat {
                "chat.completion.chunk"
            } else {
                "text_completion"
            }
        }))?;
        let raw = chunk
            .get("choices")
            .and_then(Value::as_array)
            .ok_or(Error::Protocol)?;
        require(raw.len() <= self.request.choices)?;
        let mut indices = BTreeSet::new();
        let mut deltas = Vec::with_capacity(raw.len());
        for item in raw {
            let index = item
                .get("index")
                .and_then(Value::as_u64)
                .ok_or(Error::Protocol)?;
            require(index < self.request.choices as u64 && indices.insert(index))?;
            let choice = self.choices.entry(index).or_default();
            require(choice.finish.is_none())?;
            let mut delta = json!({});
            if chat {
                let d = item
                    .get("delta")
                    .filter(|v| v.is_object())
                    .ok_or(Error::Protocol)?;
                if let Some(role) = d.get("role") {
                    require(role == "assistant")?;
                    delta["role"] = role.clone();
                }
                for (key, target) in [
                    ("content", &mut choice.content),
                    ("reasoning", &mut choice.reasoning),
                    ("reasoning_content", &mut choice.reasoning_content),
                    ("refusal", &mut choice.refusal),
                ] {
                    if let Some(v) = d.get(key).filter(|v| !v.is_null()) {
                        let text = v.as_str().ok_or(Error::Protocol)?;
                        target.get_or_insert_with(String::new).push_str(text);
                        delta[key] = v.clone();
                    }
                }
                if let Some(raw) = d.get("tool_calls").filter(|v| !v.is_null()) {
                    let calls = raw.as_array().ok_or(Error::Protocol)?;
                    require(
                        !self.request.forbid_tools
                            && !self.request.tools.is_empty()
                            && calls.len() <= self.request.limits.tools,
                    )?;
                    let mut call_indices = BTreeSet::new();
                    let mut cleaned = Vec::with_capacity(calls.len());
                    for call in calls {
                        let i = call
                            .get("index")
                            .and_then(Value::as_u64)
                            .ok_or(Error::Protocol)?;
                        require(
                            i < self.request.limits.tools as u64
                                && (self.request.parallel || i == 0)
                                && call_indices.insert(i),
                        )?;
                        let tool = choice.tools.entry(i).or_default();
                        let mut out = json!({"index":i});
                        if let Some(id) = call.get("id").filter(|v| !v.is_null()) {
                            let id = id
                                .as_str()
                                .filter(|s| identifier(s))
                                .ok_or(Error::Protocol)?;
                            require(tool.id.is_empty() || tool.id == id)?;
                            tool.id = id.into();
                            out["id"] = json!(id);
                        }
                        if let Some(kind) = call.get("type").filter(|v| !v.is_null()) {
                            require(kind == "function")?;
                            out["type"] = json!("function");
                        }
                        if let Some(f) = call.get("function").filter(|v| !v.is_null()) {
                            require(f.is_object())?;
                            let mut function = json!({});
                            for (key, target) in
                                [("name", &mut tool.name), ("arguments", &mut tool.arguments)]
                            {
                                if let Some(v) = f.get(key).filter(|v| !v.is_null()) {
                                    let text = v.as_str().ok_or(Error::Protocol)?;
                                    target.push_str(text);
                                    function[key] = json!(text);
                                }
                            }
                            require(
                                tool.name.len() <= 64
                                    && self.request.tools.iter().any(|n| n.starts_with(&tool.name)),
                            )?;
                            out["function"] = function;
                        }
                        cleaned.push(out);
                    }
                    delta["tool_calls"] = json!(cleaned);
                }
            } else {
                let text = item
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or(Error::Protocol)?;
                choice
                    .content
                    .get_or_insert_with(String::new)
                    .push_str(text);
                delta = json!(text);
            }
            if let Some(finish) = item.get("finish_reason").filter(|v| !v.is_null()) {
                let finish = finish.as_str().ok_or(Error::Protocol)?;
                require(
                    matches!(finish, "stop" | "length" | "content_filter" | "tool_calls")
                        && (chat || finish != "tool_calls"),
                )?;
                choice.finish = Some(finish.into());
            }
            if (chat && !delta.as_object().unwrap().is_empty())
                || (!chat && delta.as_str().is_some_and(|s| !s.is_empty()))
            {
                deltas.push(if chat {
                    json!({"index":index,"delta":delta,"finish_reason":null})
                } else {
                    json!({"index":index,"text":delta,"finish_reason":null})
                });
            }
        }
        if let Some(u) = chunk.get("usage").filter(|v| !v.is_null()) {
            usage(Some(u), self.request.endpoint)?;
            require(
                self.choices.len() == self.request.choices
                    && self.choices.values().all(|c| c.finish.is_some()),
            )?;
            require(self.usage.as_ref().is_none_or(|old| old == u))?;
            self.usage = Some(u.clone());
        }
        if deltas.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            json!({"id":self.public_id,"object":if chat{"chat.completion.chunk"}else{"text_completion"},"created":self.created,"model":self.request.public_model,"choices":deltas}),
        ))
    }
    /// Complete raw endpoint object for the worker's semantic verifier and the
    /// existing non-streaming protocol validator. No synthetic finish on EOF.
    pub fn finish(self) -> Result<Value> {
        require(!self.failed && self.done && self.choices.len() == self.request.choices)?;
        let chat = self.request.endpoint == ProxyEndpoint::Chat;
        let mut choices = Vec::with_capacity(self.choices.len());
        for (index, c) in self.choices {
            let reason = c.finish.ok_or(Error::Protocol)?;
            if chat {
                let mut message = json!({"role":"assistant","content":c.content});
                for (key, value) in [
                    ("reasoning", c.reasoning),
                    ("reasoning_content", c.reasoning_content),
                    ("refusal", c.refusal),
                ] {
                    if let Some(value) = value {
                        message[key] = json!(value);
                    }
                }
                if !c.tools.is_empty() {
                    let mut calls = Vec::with_capacity(c.tools.len());
                    for (expected, (actual, t)) in c.tools.into_iter().enumerate() {
                        require(expected as u64 == actual)?;
                        calls.push(json!({"id":t.id,"type":"function","function":{"name":t.name,"arguments":t.arguments}}));
                    }
                    message["tool_calls"] = json!(calls);
                }
                choices.push(json!({"index":index,"message":message,"finish_reason":reason}));
            } else {
                choices.push(json!({"index":index,"text":c.content.unwrap_or_default(),"finish_reason":reason}));
            }
        }
        let mut result =
            json!({"object":if chat{"chat.completion"}else{"text_completion"},"choices":choices});
        if let Some(id) = self.upstream_id {
            result["id"] = json!(id);
        }
        if let Some(usage) = self.usage {
            result["usage"] = usage;
        }
        Ok(result)
    }
}
