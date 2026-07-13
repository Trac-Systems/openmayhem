use anyhow::{bail, ensure, Context, Result};
use mayhem_engine::{GenerateSpecialityParameter, MTMD_MEDIA_MARKER};
use serde_json::{Map, Number, Value};
use std::fmt::Write as _;
use std::str::FromStr;

pub(crate) const THOUGHT_OPEN: &str = "<|channel>thought\n";
pub(crate) const THOUGHT_CLOSE: &str = "<channel|>";
const STRING_MARKER: &str = "<|\"|>";
const TOOL_CALL_OPEN: &str = "<|tool_call>call:";
const TOOL_CALL_CLOSE: &str = "<tool_call|>";

pub(crate) fn render_prompt(
    messages: &[Value],
    tools: &[Value],
    specialities: &[GenerateSpecialityParameter],
) -> Result<String> {
    ensure!(
        !messages.is_empty(),
        "Gemma 4 chat requires at least one message"
    );
    let enable_thinking = specialities
        .iter()
        .find(|speciality| speciality.native_path == "enable_thinking")
        .map(|speciality| {
            speciality.value.as_bool().with_context(|| {
                format!(
                    "Gemma 4 speciality {} must map enable_thinking to a boolean",
                    speciality.name
                )
            })
        })
        .transpose()?
        .unwrap_or(false);

    let mut prompt = String::new();
    let has_initial_system = messages
        .first()
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        .is_some_and(|role| matches!(role, "system" | "developer"));
    let mut first_message = 0;
    if enable_thinking || !tools.is_empty() || has_initial_system {
        prompt.push_str("<|turn>system\n");
        if enable_thinking {
            prompt.push_str("<|think|>\n");
        }
        if has_initial_system {
            prompt.push_str(&render_content(
                messages[0].get("content").unwrap_or(&Value::Null),
                false,
            )?);
            first_message = 1;
        }
        for tool in tools {
            prompt.push_str("<|tool>");
            prompt.push_str(&render_tool_declaration(tool)?);
            prompt.push_str("<tool|>");
        }
        prompt.push_str("<turn|>\n");
    }

    let remaining = &messages[first_message..];
    let last_user_index = remaining
        .iter()
        .rposition(|message| message.get("role").and_then(Value::as_str) == Some("user"));
    let mut previous_non_tool_role: Option<&str> = None;
    let mut previous_message_type: Option<&str> = None;

    for (index, message) in remaining.iter().enumerate() {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .context("Gemma 4 message is missing role")?;
        if role == "tool" {
            continue;
        }
        ensure!(
            matches!(role, "system" | "developer" | "user" | "assistant"),
            "Gemma 4 does not support message role {role}"
        );
        let gemma_role = if role == "assistant" { "model" } else { role };
        let continuing_model = role == "assistant" && previous_non_tool_role == Some("assistant");
        if !continuing_model {
            write!(prompt, "<|turn>{gemma_role}\n").expect("writing to String cannot fail");
        }

        if role == "assistant" {
            if let Some(reasoning) = message
                .get("reasoning")
                .or_else(|| message.get("reasoning_content"))
                .and_then(Value::as_str)
                .filter(|reasoning| !reasoning.is_empty())
            {
                if last_user_index.is_some_and(|last_user| index > last_user)
                    && message.get("tool_calls").is_some_and(Value::is_array)
                {
                    prompt.push_str(THOUGHT_OPEN);
                    prompt.push_str(reasoning);
                    prompt.push('\n');
                    prompt.push_str(THOUGHT_CLOSE);
                }
            }
        }

        let tool_calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for call in tool_calls {
            prompt.push_str(&render_tool_call(call)?);
        }
        if !tool_calls.is_empty() {
            previous_message_type = Some("tool_call");
        }

        let mut rendered_tool_response = false;
        if !tool_calls.is_empty() {
            for follow in remaining.iter().skip(index + 1) {
                if follow.get("role").and_then(Value::as_str) != Some("tool") {
                    break;
                }
                let name = resolve_tool_name(tool_calls, follow).unwrap_or("unknown");
                prompt.push_str(&render_tool_response(
                    name,
                    follow.get("content").unwrap_or(&Value::Null),
                ));
                rendered_tool_response = true;
                previous_message_type = Some("tool_response");
            }
        }

        let content = render_content(
            message.get("content").unwrap_or(&Value::Null),
            role == "assistant",
        )?;
        prompt.push_str(&content);
        let has_content = !content.trim().is_empty();
        if previous_message_type == Some("tool_call") && !rendered_tool_response {
            prompt.push_str("<|tool_response>");
        } else if rendered_tool_response && !has_content {
            // Gemma continues generation directly after the tool-response block.
        } else {
            prompt.push_str("<turn|>\n");
        }
        previous_non_tool_role = Some(role);
    }

    if !matches!(previous_message_type, Some("tool_response" | "tool_call")) {
        prompt.push_str("<|turn>model\n");
    }
    Ok(prompt)
}

fn render_content(content: &Value, strip_historical_thoughts: bool) -> Result<String> {
    match content {
        Value::Null => Ok(String::new()),
        Value::String(text) => Ok(if strip_historical_thoughts {
            strip_thought_channels(text)
        } else {
            text.trim().to_owned()
        }),
        Value::Array(parts) => {
            let mut rendered = String::new();
            for part in parts {
                match part.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
                        if strip_historical_thoughts {
                            rendered.push_str(&strip_thought_channels(text));
                        } else {
                            rendered.push_str(text.trim());
                        }
                    }
                    Some("image" | "image_url" | "audio" | "input_audio") => {
                        rendered.push_str(MTMD_MEDIA_MARKER);
                    }
                    Some("video") => {
                        let frame_count = part
                            .get("video")
                            .and_then(|video| video.get("frames"))
                            .and_then(Value::as_array)
                            .map_or(1, Vec::len);
                        ensure!(frame_count > 0, "Gemma 4 video.frames must not be empty");
                        for _ in 0..frame_count {
                            rendered.push_str(MTMD_MEDIA_MARKER);
                        }
                    }
                    Some(other) => bail!("Gemma 4 does not support content part type {other}"),
                    None => bail!("Gemma 4 content part is missing type"),
                }
            }
            Ok(rendered)
        }
        other => Ok(other.to_string()),
    }
}

fn render_tool_declaration(tool: &Value) -> Result<String> {
    let function = tool
        .get("function")
        .and_then(Value::as_object)
        .context("Gemma 4 tool is missing function")?;
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| valid_tool_name(name))
        .context("Gemma 4 tool has an invalid function name")?;
    let description = function
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let parameters = function
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({"type":"object"}));
    Ok(format!(
        "declaration:{name}{{description:{},parameters:{}}}",
        marker_string(description),
        render_schema(&parameters)
    ))
}

fn render_schema(value: &Value) -> String {
    match value {
        Value::Object(object) => {
            let mut rendered = String::from("{");
            for (index, (key, value)) in object.iter().enumerate() {
                if index > 0 {
                    rendered.push(',');
                }
                rendered.push_str(key);
                rendered.push(':');
                if key == "type" {
                    match value {
                        Value::String(value) => {
                            rendered.push_str(&marker_string(&value.to_uppercase()))
                        }
                        Value::Array(values) => {
                            let values = values
                                .iter()
                                .map(|value| {
                                    value.as_str().map_or_else(
                                        || render_argument(value, true),
                                        |value| marker_string(&value.to_uppercase()),
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(",");
                            write!(rendered, "[{values}]").expect("writing to String cannot fail");
                        }
                        _ => rendered.push_str(&render_argument(value, true)),
                    }
                } else if key == "properties" {
                    rendered.push_str(&render_properties(value));
                } else {
                    rendered.push_str(&render_argument(value, true));
                }
            }
            rendered.push('}');
            rendered
        }
        _ => render_argument(value, true),
    }
}

fn render_properties(value: &Value) -> String {
    let Some(properties) = value.as_object() else {
        return render_argument(value, false);
    };
    let mut rendered = String::from("{");
    for (index, (name, definition)) in properties.iter().enumerate() {
        if index > 0 {
            rendered.push(',');
        }
        rendered.push_str(name);
        rendered.push(':');
        rendered.push_str(&render_schema(definition));
    }
    rendered.push('}');
    rendered
}

fn render_tool_call(call: &Value) -> Result<String> {
    let function = call
        .get("function")
        .and_then(Value::as_object)
        .context("Gemma 4 assistant tool call is missing function")?;
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| valid_tool_name(name))
        .context("Gemma 4 assistant tool call has an invalid function name")?;
    let arguments = function
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Object(Map::new()));
    let arguments = match arguments {
        Value::String(encoded) => serde_json::from_str(&encoded)
            .context("Gemma 4 assistant tool-call arguments are not valid JSON")?,
        value => value,
    };
    ensure!(
        arguments.is_object(),
        "Gemma 4 tool-call arguments must be an object"
    );
    Ok(format!(
        "{TOOL_CALL_OPEN}{name}{}{TOOL_CALL_CLOSE}",
        render_argument(&arguments, false)
    ))
}

fn resolve_tool_name<'a>(tool_calls: &'a [Value], response: &Value) -> Option<&'a str> {
    let id = response.get("tool_call_id").and_then(Value::as_str)?;
    tool_calls.iter().find_map(|call| {
        (call.get("id").and_then(Value::as_str) == Some(id))
            .then(|| call.get("function")?.get("name")?.as_str())
            .flatten()
    })
}

fn render_tool_response(name: &str, response: &Value) -> String {
    let value = match response {
        Value::String(value) => Value::String(value.clone()),
        Value::Array(parts) => Value::String(
            parts
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<String>(),
        ),
        other => other.clone(),
    };
    let body = match value {
        Value::Object(object) => render_argument(&Value::Object(object), false),
        value => format!("{{value:{}}}", render_argument(&value, false)),
    };
    format!("<|tool_response>response:{name}{body}<tool_response|>")
}

fn render_argument(value: &Value, quote_keys: bool) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => marker_string(value),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(|value| render_argument(value, quote_keys))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(object) => {
            let entries = object
                .iter()
                .map(|(key, value)| {
                    let key = if quote_keys {
                        marker_string(key)
                    } else {
                        key.clone()
                    };
                    format!("{key}:{}", render_argument(value, quote_keys))
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{entries}}}")
        }
    }
}

fn marker_string(value: &str) -> String {
    format!("{STRING_MARKER}{value}{STRING_MARKER}")
}

fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

pub(crate) fn strip_thought_channels(text: &str) -> String {
    let mut remaining = text;
    let mut visible = String::new();
    while let Some(start) = remaining.find(THOUGHT_OPEN) {
        visible.push_str(&remaining[..start]);
        let after_open = &remaining[start + THOUGHT_OPEN.len()..];
        let Some(end) = after_open.find(THOUGHT_CLOSE) else {
            return visible.trim().to_owned();
        };
        remaining = &after_open[end + THOUGHT_CLOSE.len()..];
    }
    visible.push_str(remaining);
    visible.trim_start_matches(['\r', '\n']).to_owned()
}

pub(crate) fn parse_tool_calls(text: &str) -> Option<Vec<(String, Value)>> {
    let visible = strip_thought_channels(text);
    let first = visible.find(TOOL_CALL_OPEN)?;
    if !visible[..first].trim().is_empty() {
        return None;
    }
    let mut remaining = &visible[first..];
    let mut calls = Vec::new();
    while !remaining.trim().is_empty() {
        remaining = remaining.trim_start();
        let body = remaining.strip_prefix(TOOL_CALL_OPEN)?;
        let body_end = body.find(TOOL_CALL_CLOSE)?;
        calls.push(parse_tool_call_body(body[..body_end].trim())?);
        remaining = &body[body_end + TOOL_CALL_CLOSE.len()..];
    }
    (!calls.is_empty()).then_some(calls)
}

fn parse_tool_call_body(body: &str) -> Option<(String, Value)> {
    let args_start = body.find('{')?;
    let name = body[..args_start].trim();
    if !valid_tool_name(name) {
        return None;
    }
    let mut parser = PseudoJsonParser::new(&body[args_start..]);
    let arguments = parser.parse_value(0).ok()?;
    parser.skip_whitespace();
    if !parser.is_eof() || !arguments.is_object() {
        return None;
    }
    Some((name.to_owned(), arguments))
}

struct PseudoJsonParser<'a> {
    input: &'a str,
    offset: usize,
}

impl<'a> PseudoJsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, offset: 0 }
    }

    fn is_eof(&self) -> bool {
        self.offset == self.input.len()
    }

    fn remaining(&self) -> &'a str {
        &self.input[self.offset..]
    }

    fn skip_whitespace(&mut self) {
        let count = self
            .remaining()
            .chars()
            .take_while(|character| character.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();
        self.offset += count;
    }

    fn consume(&mut self, expected: &str) -> bool {
        self.skip_whitespace();
        if self.remaining().starts_with(expected) {
            self.offset += expected.len();
            true
        } else {
            false
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<Value, ()> {
        if depth > 64 {
            return Err(());
        }
        self.skip_whitespace();
        if self.remaining().starts_with(STRING_MARKER) {
            return self.parse_marker_string().map(Value::String);
        }
        match self.remaining().as_bytes().first().copied() {
            Some(b'{') => self.parse_object(depth + 1),
            Some(b'[') => self.parse_array(depth + 1),
            Some(b'"') => self.parse_json_string().map(Value::String),
            Some(_) => self.parse_scalar(),
            None => Err(()),
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<Value, ()> {
        if !self.consume("{") {
            return Err(());
        }
        let mut object = Map::new();
        if self.consume("}") {
            return Ok(Value::Object(object));
        }
        loop {
            let key = self.parse_key()?;
            if key.is_empty() || object.contains_key(&key) || !self.consume(":") {
                return Err(());
            }
            object.insert(key, self.parse_value(depth)?);
            if self.consume("}") {
                break;
            }
            if !self.consume(",") {
                return Err(());
            }
        }
        Ok(Value::Object(object))
    }

    fn parse_array(&mut self, depth: usize) -> Result<Value, ()> {
        if !self.consume("[") {
            return Err(());
        }
        let mut values = Vec::new();
        if self.consume("]") {
            return Ok(Value::Array(values));
        }
        loop {
            values.push(self.parse_value(depth)?);
            if self.consume("]") {
                break;
            }
            if !self.consume(",") {
                return Err(());
            }
        }
        Ok(Value::Array(values))
    }

    fn parse_key(&mut self) -> Result<String, ()> {
        self.skip_whitespace();
        if self.remaining().starts_with(STRING_MARKER) {
            return self.parse_marker_string();
        }
        if self.remaining().starts_with('"') {
            return self.parse_json_string();
        }
        let length = self
            .remaining()
            .find(|character: char| character == ':' || character.is_whitespace())
            .unwrap_or(self.remaining().len());
        if length == 0 {
            return Err(());
        }
        let key = self.remaining()[..length].to_owned();
        self.offset += length;
        Ok(key)
    }

    fn parse_marker_string(&mut self) -> Result<String, ()> {
        if !self.consume(STRING_MARKER) {
            return Err(());
        }
        let end = self.remaining().find(STRING_MARKER).ok_or(())?;
        let value = self.remaining()[..end].to_owned();
        self.offset += end + STRING_MARKER.len();
        Ok(value)
    }

    fn parse_json_string(&mut self) -> Result<String, ()> {
        self.skip_whitespace();
        let mut escaped = false;
        let mut end = None;
        for (index, character) in self.remaining().char_indices().skip(1) {
            if character == '"' && !escaped {
                end = Some(index + 1);
                break;
            }
            escaped = character == '\\' && !escaped;
            if character != '\\' {
                escaped = false;
            }
        }
        let end = end.ok_or(())?;
        let encoded = &self.remaining()[..end];
        let value = serde_json::from_str::<String>(encoded).map_err(|_| ())?;
        self.offset += end;
        Ok(value)
    }

    fn parse_scalar(&mut self) -> Result<Value, ()> {
        self.skip_whitespace();
        let length = self
            .remaining()
            .find(|character: char| {
                matches!(character, ',' | '}' | ']') || character.is_whitespace()
            })
            .unwrap_or(self.remaining().len());
        if length == 0 {
            return Err(());
        }
        let scalar = &self.remaining()[..length];
        self.offset += length;
        match scalar {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            "null" => Ok(Value::Null),
            _ => Number::from_str(scalar)
                .map(Value::Number)
                .or_else(|_| Ok(Value::String(scalar.to_owned()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mayhem_engine::GenerateSpecialityTarget;
    use serde_json::json;

    fn thinking(enabled: bool) -> GenerateSpecialityParameter {
        GenerateSpecialityParameter {
            name: "thinking_mode".to_owned(),
            level: if enabled { "enabled" } else { "disabled" }.to_owned(),
            target: GenerateSpecialityTarget::ChatTemplateKwarg,
            native_path: "enable_thinking".to_owned(),
            value: Value::Bool(enabled),
        }
    }

    #[test]
    fn renders_native_roles_thinking_media_and_tools() {
        let messages = json!([
            {"role":"system","content":"Be concise."},
            {"role":"user","content":[
                {"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}},
                {"type":"text","text":"Inspect it"},
                {"type":"input_audio","input_audio":{"format":"wav","data":"AA=="}},
                {"type":"video","video":{"frames":["a","b"]}}
            ]}
        ]);
        let tools = json!([{"type":"function","function":{
            "name":"lookup",
            "description":"Look up an item",
            "parameters":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}
        }}]);

        let prompt = render_prompt(
            messages.as_array().unwrap(),
            tools.as_array().unwrap(),
            &[thinking(true)],
        )
        .unwrap();

        assert!(prompt.starts_with("<|turn>system\n<|think|>\nBe concise."));
        assert!(prompt.contains("<|tool>declaration:lookup"));
        assert_eq!(prompt.matches(MTMD_MEDIA_MARKER).count(), 4);
        assert!(prompt.ends_with("<|turn>model\n"));
    }

    #[test]
    fn renders_and_parses_native_tool_round_trip() {
        let first = json!({
            "id":"call-1",
            "type":"function",
            "function":{"name":"lookup","arguments":"{\"id\":7,\"tags\":[\"a\",\"b\"]}"}
        });
        let second = json!({
            "id":"call-2",
            "type":"function",
            "function":{"name":"read","arguments":"{\"path\":\"README.md\"}"}
        });
        let output = format!(
            "{THOUGHT_OPEN}check{THOUGHT_CLOSE}{}{}",
            render_tool_call(&first).unwrap(),
            render_tool_call(&second).unwrap()
        );

        let calls = parse_tool_calls(&output).unwrap();

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "lookup");
        assert_eq!(calls[0].1, json!({"id":7,"tags":["a","b"]}));
        assert_eq!(calls[1].0, "read");
        assert_eq!(calls[1].1, json!({"path":"README.md"}));
    }

    #[test]
    fn parser_refuses_visible_text_around_tool_call() {
        assert!(parse_tool_calls("answer <|tool_call>call:lookup{id:7}<tool_call|>").is_none());
    }
}
