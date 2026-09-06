//! Incremental presentation of engine output. Prompt construction, token IDs and
//! the authoritative, schema-checked final tool parser remain unchanged.
use super::{ProviderEngineToolStrategy, ToolSpec, provider_qwen_xml_parameter_value};
use serde_json::{Value, json};

#[derive(Default, Debug)]
pub(super) struct Delta {
    pub text: String,
    pub tools: Vec<Value>,
}

pub(super) struct OutputStream {
    strategy: ProviderEngineToolStrategy,
    tools: Vec<ToolSpec>,
    pending: String,
    tool_text: String,
    in_tools: bool,
    json_probe: bool,
    emitted: Vec<(String, String)>,
    pub text: String,
}

impl OutputStream {
    pub fn new(strategy: ProviderEngineToolStrategy, tools: Vec<ToolSpec>) -> Self {
        Self {
            strategy,
            tools,
            pending: String::new(),
            tool_text: String::new(),
            in_tools: false,
            json_probe: true,
            emitted: Vec::new(),
            text: String::new(),
        }
    }

    pub fn push(&mut self, text: &str) -> Delta {
        let mut delta = Delta::default();
        if !self.in_tools {
            self.pending.push_str(text);
            let trimmed = self.pending.trim_start();
            // JSON tool envelopes are recognizable before their arguments. Hold
            // only that prefix; ordinary prose never waits for generation end.
            if self.json_probe && (trimmed.is_empty() || trimmed.starts_with(['{', '['])) {
                if trimmed.is_empty() {
                    return delta;
                }
                match json_tool_prefix(trimmed) {
                    Some(true) => {
                        self.in_tools = true;
                    }
                    Some(false) => {
                        self.json_probe = false;
                    }
                    None => return delta,
                }
            } else {
                self.json_probe = false;
            }
            if !self.in_tools {
                let marker = match self.strategy {
                    ProviderEngineToolStrategy::QwenFunctionXml => "<tool_call>",
                    ProviderEngineToolStrategy::GemmaFunctionCall => "<|tool_call>call:",
                    _ => "",
                };
                if !marker.is_empty() {
                    if let Some(index) = self.pending.find(marker) {
                        delta.text = self.pending[..index].to_owned();
                        self.pending.drain(..index);
                        self.in_tools = true;
                    } else {
                        let retained = suffix_prefix_len(&self.pending, marker);
                        let end = self.pending.len() - retained;
                        delta.text = self.pending[..end].to_owned();
                        self.pending.drain(..end);
                    }
                } else {
                    delta.text = std::mem::take(&mut self.pending);
                }
            }
            if self.in_tools {
                self.tool_text.push_str(&std::mem::take(&mut self.pending));
            }
        } else {
            self.tool_text.push_str(text);
        }
        self.text.push_str(&delta.text);
        if self.in_tools {
            let previews = if self.tool_text.trim_start().starts_with(['{', '[']) {
                json_previews(&self.tool_text)
            } else {
                match self.strategy {
                    ProviderEngineToolStrategy::QwenFunctionXml => {
                        qwen_previews(&self.tool_text, &self.tools)
                    }
                    ProviderEngineToolStrategy::GemmaFunctionCall => {
                        gemma_previews(&self.tool_text)
                    }
                    _ => Vec::new(),
                }
            };
            for (index, (name, arguments)) in previews.into_iter().enumerate() {
                // Unadvertised names never become public calls. The existing
                // final parser still validates the entire call and its schema.
                if !self.tools.iter().any(|tool| tool.name == name) {
                    break;
                }
                if index == self.emitted.len() {
                    delta
                        .tools
                        .push(json!({"index":index,"name":name,"arguments":arguments}));
                    self.emitted.push((name, arguments));
                } else if let Some((old_name, old_args)) = self.emitted.get_mut(index) {
                    if *old_name == name && arguments.starts_with(old_args.as_str()) {
                        let suffix = &arguments[old_args.len()..];
                        if !suffix.is_empty() {
                            delta.tools.push(json!({"index":index,"arguments":suffix}));
                            *old_args = arguments;
                        }
                    }
                }
            }
        }
        delta
    }

    pub fn finish_text(&mut self, has_tools: bool) -> String {
        let tail = if has_tools {
            String::new()
        } else if self.in_tools && self.emitted.is_empty() {
            std::mem::take(&mut self.tool_text)
        } else {
            std::mem::take(&mut self.pending)
        };
        self.text.push_str(&tail);
        tail
    }

    pub fn emitted_count(&self) -> usize {
        self.emitted.len()
    }
}

fn suffix_prefix_len(text: &str, marker: &str) -> usize {
    (1..marker.len())
        .rev()
        .find(|length| text.ends_with(&marker[..*length]))
        .unwrap_or(0)
}

fn escaped(text: &str) -> String {
    let value = serde_json::to_string(text).expect("string serialization");
    value[1..value.len() - 1].to_owned()
}

fn json_tool_prefix(text: &str) -> Option<bool> {
    let text = text.trim_start_matches(['[', ' ', '\n', '\r', '\t']);
    if !text.is_empty() && !text.starts_with('{') {
        return Some(false);
    }
    let text = text.strip_prefix('{')?.trim_start();
    let (key, _) = json_string(text)?;
    Some(matches!(
        key.as_str(),
        "tool_calls" | "tool" | "name" | "function" | "id" | "arguments"
    ))
}

// Return a complete JSON string and the consumed byte count, respecting escaped
// quotes and UTF-8. Incomplete strings deliberately remain pending.
fn json_string(text: &str) -> Option<(String, usize)> {
    if !text.starts_with('"') {
        return None;
    }
    let mut escaped = false;
    for (index, ch) in text.char_indices().skip(1) {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some((serde_json::from_str(&text[..index + 1]).ok()?, index + 1));
        }
    }
    None
}

fn json_string_prefix(text: &str) -> String {
    if let Some((value, _)) = json_string(text) {
        return value;
    }
    if !text.starts_with('"') {
        return String::new();
    }
    // Retain an incomplete escape (including a surrogate pair) until it can be
    // decoded. At most twelve trailing bytes need to be withheld.
    let mut end = text.len();
    for _ in 0..13 {
        let candidate = format!("{}\"", &text[..end]);
        if let Ok(value) = serde_json::from_str::<String>(&candidate) {
            return value;
        }
        if end <= 1 {
            break;
        }
        end -= 1;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
    }
    String::new()
}

// Scan values without reparsing a partial string as a complete JSON object.
// The returned length is the complete value boundary, or None while incomplete.
fn json_value_end(text: &str) -> Option<usize> {
    if text.starts_with('"') {
        return json_string(text).map(|(_, end)| end);
    }
    let mut depth = 0_i32;
    let mut quoted = false;
    let mut escape = false;
    for (index, ch) in text.char_indices() {
        if quoted {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                quoted = false;
            }
            continue;
        }
        match ch {
            '"' => quoted = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index + 1);
                }
                if depth < 0 {
                    return Some(index);
                }
            }
            ',' | '\n' | '\r' | '\t' | ' ' if depth == 0 => return Some(index),
            _ => {}
        }
    }
    None
}

fn json_field<'a>(text: &'a str, wanted: &str) -> Option<&'a str> {
    let mut rest = text.trim_start().strip_prefix('{')?.trim_start();
    loop {
        let (key, end) = json_string(rest)?;
        rest = rest[end..].trim_start().strip_prefix(':')?.trim_start();
        if key == wanted {
            return Some(rest);
        }
        let end = json_value_end(rest)?;
        rest = rest[end..].trim_start().strip_prefix(',')?.trim_start();
    }
}

fn json_previews(text: &str) -> Vec<(String, String)> {
    let text = text.trim_start();
    let envelope = json_field(text, "tool_calls").unwrap_or(text);
    let mut rest = envelope.strip_prefix('[').unwrap_or(envelope).trim_start();
    let mut calls = Vec::new();
    loop {
        let function = json_field(rest, "function").unwrap_or(rest);
        let Some(name) = json_field(rest, "tool")
            .or_else(|| json_field(function, "name"))
            .and_then(json_string)
            .map(|(name, _)| name)
        else {
            break;
        };
        let arguments = json_field(function, "arguments")
            .map(|raw| {
                if raw.starts_with('"') {
                    json_string_prefix(raw)
                } else {
                    raw[..json_value_end(raw).unwrap_or(raw.len())].to_owned()
                }
            })
            .unwrap_or_default();
        calls.push((name, arguments));
        let Some(end) = json_value_end(rest) else {
            break;
        };
        let Some(next) = rest[end..].trim_start().strip_prefix(',') else {
            break;
        };
        rest = next.trim_start();
    }
    calls
}

fn qwen_previews(text: &str, tools: &[ToolSpec]) -> Vec<(String, String)> {
    let mut rest = text.trim_start();
    let mut calls = Vec::new();
    while let Some(body) = rest.strip_prefix("<tool_call>") {
        let Some(body) = body.trim_start().strip_prefix("<function=") else {
            break;
        };
        let Some(end) = body.find('>') else {
            break;
        };
        let name = body[..end].trim().to_owned();
        let tool = tools.iter().find(|tool| tool.name == name);
        let mut remaining = body[end + 1..].trim_start();
        let mut arguments = String::from("{");
        let mut first = true;
        while let Some(parameter) = remaining.strip_prefix("<parameter=") {
            let Some(end) = parameter.find('>') else {
                break;
            };
            let key = parameter[..end].trim();
            let raw = &parameter[end + 1..];
            if !first {
                arguments.push(',');
            }
            arguments.push_str(&serde_json::to_string(key).unwrap());
            arguments.push(':');
            if let Some(end) = raw.find("</parameter>") {
                arguments.push_str(
                    &provider_qwen_xml_parameter_value(tool, key, &raw[..end]).to_string(),
                );
                remaining = raw[end + "</parameter>".len()..].trim_start();
                first = false;
            } else {
                let schema = tool
                    .and_then(|tool| tool.parameters.get("properties"))
                    .and_then(|p| p.get(key));
                if schema
                    .and_then(|schema| schema.get("type"))
                    .and_then(Value::as_str)
                    == Some("string")
                {
                    let end = raw.len() - suffix_prefix_len(raw, "</parameter>");
                    let raw = &raw[..end];
                    let raw = raw
                        .strip_prefix("\r\n")
                        .or_else(|| raw.strip_prefix('\n'))
                        .unwrap_or(raw);
                    // A native framing newline may still be the final newline.
                    let raw = raw
                        .strip_suffix("\r\n")
                        .or_else(|| raw.strip_suffix('\n'))
                        .or_else(|| raw.strip_suffix('\r'))
                        .unwrap_or(raw);
                    arguments.push('"');
                    arguments.push_str(&escaped(raw));
                }
                break;
            }
        }
        let complete = remaining
            .strip_prefix("</function>")
            .map(str::trim_start)
            .and_then(|remaining| remaining.strip_prefix("</tool_call>"));
        if complete.is_some() {
            arguments.push('}');
        }
        calls.push((name, arguments));
        let Some(next) = complete else {
            break;
        };
        rest = next.trim_start();
    }
    calls
}

fn gemma_previews(text: &str) -> Vec<(String, String)> {
    let mut rest = text.trim_start();
    let mut calls = Vec::new();
    while let Some(body) = rest.strip_prefix("<|tool_call>call:") {
        let Some(start) = body.find('{') else {
            break;
        };
        let name = body[..start].trim().to_owned();
        let end = body.find("<tool_call|>");
        let raw = &body
            [start..end.unwrap_or_else(|| body.len() - suffix_prefix_len(body, "<tool_call|>"))];
        calls.push((name, gemma_json_prefix(raw)));
        let Some(end) = end else {
            break;
        };
        rest = body[end + "<tool_call|>".len()..].trim_start();
    }
    calls
}

fn gemma_json_prefix(mut rest: &str) -> String {
    let mut out = String::new();
    let mut key = false;
    let mut objects = Vec::new();
    while !rest.is_empty() {
        if let Some(raw) = rest.strip_prefix("<|\"|>") {
            out.push('"');
            if let Some(end) = raw.find("<|\"|>") {
                out.push_str(&escaped(&raw[..end]));
                out.push('"');
                rest = &raw[end + 5..];
            } else {
                let end = raw.len() - suffix_prefix_len(raw, "<|\"|>");
                out.push_str(&escaped(&raw[..end]));
                break;
            }
            key = false;
            continue;
        }
        if "<|\"|>".starts_with(rest) {
            break;
        }
        let ch = rest.chars().next().unwrap();
        if ch == '"' {
            if let Some((_, end)) = json_string(rest) {
                out.push_str(&rest[..end]);
                rest = &rest[end..];
                key = false;
                continue;
            }
            out.push_str(rest);
            break;
        }
        if key && !ch.is_whitespace() && ch != '}' && ch != '"' {
            let Some(end) = rest.find(':') else {
                break;
            };
            out.push_str(&serde_json::to_string(rest[..end].trim()).unwrap());
            out.push(':');
            rest = &rest[end + 1..];
            key = false;
            continue;
        }
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
        match ch {
            '{' => {
                objects.push(true);
                key = true;
            }
            '[' => {
                objects.push(false);
                key = false;
            }
            '}' | ']' => {
                objects.pop();
                key = false;
            }
            ',' => key = objects.last() == Some(&true),
            ':' => key = false,
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{provider_engine_tool_call_outputs, validate_provider_engine_tool_call_outputs};

    fn tools() -> Vec<ToolSpec> {
        vec![ToolSpec::new(
            "write",
            json!({"type":"object","properties":{
            "path":{"type":"string"},"content":{"type":"string"},"data":{"type":"array"},
            "count":{"type":["string","integer"]}},"additionalProperties":false}),
        )]
    }

    fn check(strategy: ProviderEngineToolStrategy, raw: &str) {
        let tools = tools();
        let expected = provider_engine_tool_call_outputs(raw, strategy, &tools).unwrap();
        validate_provider_engine_tool_call_outputs(&expected, &tools).unwrap();
        // Every UTF-8 split, and one character per token, must agree with the
        // existing final parser. This catches split markers and escape sequences.
        for split in raw
            .char_indices()
            .map(|(index, _)| index)
            .chain([raw.len()])
        {
            let mut stream = OutputStream::new(strategy, tools.clone());
            let mut calls: Vec<(String, String)> = Vec::new();
            for delta in [stream.push(&raw[..split]), stream.push(&raw[split..])] {
                collect(delta, &mut calls);
            }
            compare(&calls, &expected);
        }
        let mut stream = OutputStream::new(strategy, tools);
        let mut calls = Vec::new();
        let mut first_arguments = None;
        for (index, ch) in raw.char_indices() {
            let delta = stream.push(&ch.to_string());
            if delta.tools.iter().any(|tool| {
                tool["arguments"]
                    .as_str()
                    .is_some_and(|args| !args.is_empty() && args != "{")
            }) {
                first_arguments.get_or_insert(index);
            }
            collect(delta, &mut calls);
        }
        assert!(
            first_arguments.unwrap() < raw.len() - 5,
            "arguments must precede generation completion"
        );
        compare(&calls, &expected);
    }

    fn collect(delta: Delta, calls: &mut Vec<(String, String)>) {
        for tool in delta.tools {
            let index = tool["index"].as_u64().unwrap() as usize;
            if index == calls.len() {
                calls.push((tool["name"].as_str().unwrap().to_owned(), String::new()));
            }
            calls[index].1.push_str(tool["arguments"].as_str().unwrap());
        }
    }

    fn compare(calls: &[(String, String)], expected: &[Value]) {
        assert_eq!(calls.len(), expected.len());
        for ((name, args), expected) in calls.iter().zip(expected) {
            assert_eq!(name, expected["name"].as_str().unwrap());
            assert_eq!(
                serde_json::from_str::<Value>(args).unwrap_or_else(|err| panic!("{err}: {args}")),
                serde_json::from_str::<Value>(expected["arguments"].as_str().unwrap()).unwrap()
            );
        }
    }

    #[test]
    fn all_tool_formats_stream_arguments_before_generation_finishes() {
        check(
            ProviderEngineToolStrategy::QwenFunctionXml,
            "<tool_call>\n<function=write>\n<parameter=path>\nindex.html\n</parameter>\n<parameter=content>\n  <div>é🦀 \\\"hi\\\"</div>\n\n</parameter>\n<parameter=count>\n12\n</parameter>\n</function>\n</tool_call>",
        );
        check(
            ProviderEngineToolStrategy::MayhemJson,
            r#"{"tool_calls":[{"tool":"write","arguments":{"path":"index.html","content":"é \"quoted\" \ud83e\udd80","data":[1,2]}}]}"#,
        );
        check(
            ProviderEngineToolStrategy::OpenAiToolCalls,
            r#"{"tool_calls":[{"id":"native","function":{"name":"write","arguments":"{\"path\":\"index.html\",\"content\":\"hello world\"}"}},{"function":{"name":"write","arguments":{"path":"style.css"}}}]}"#,
        );
        check(
            ProviderEngineToolStrategy::GemmaFunctionCall,
            "<|tool_call>call:write{path:<|\"|>index.html<|\"|>,content:<|\"|>é🦀 \\\"quoted\\\"<|\"|>,data:[1,2,{x:true}]}<tool_call|>",
        );
    }

    #[test]
    fn advertising_tools_does_not_buffer_plain_text_or_ordinary_json() {
        for strategy in [
            ProviderEngineToolStrategy::QwenFunctionXml,
            ProviderEngineToolStrategy::GemmaFunctionCall,
            ProviderEngineToolStrategy::MayhemJson,
            ProviderEngineToolStrategy::OpenAiToolCalls,
        ] {
            for text in [
                "Hello, this is already available.",
                "{\"answer\":42,\"more\":true}",
                "[1,2,3]",
            ] {
                let mut stream = OutputStream::new(strategy, tools());
                let first = stream.push(&text[..text.len() - 2]);
                assert!(!first.text.is_empty(), "{strategy:?}: {text}");
                let mut output = first.text;
                output.push_str(&stream.push(&text[text.len() - 2..]).text);
                output.push_str(&stream.finish_text(false));
                assert_eq!(output, text);
            }
        }
    }
}
