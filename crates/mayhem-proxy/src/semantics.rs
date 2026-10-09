//! Offline semantic verification inside the supervised worker. Compilers and
//! regexes see untrusted schemas, so execution must not run in the parent broker.
//! No network/file reference retrieval, schema projection, repairs or billing.

use crate::{
    attempts::Digest,
    connector::failure::{Code, Execution, Failure, Scope, Stage},
};
use mayhem_proto::proxy::ProxyEndpoint;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fmt};

pub const MAX_POLICY_BYTES: usize = 256 * 1024;
const MAX_SCHEMA_BYTES: usize = 128 * 1024;
const MAX_NODES: usize = 4096;
const MAX_DEPTH: usize = 64;
const MAX_PATTERNS: usize = 64;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Output {
    Text,
    JsonObject,
    JsonSchema { schema: Value },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub endpoint: ProxyEndpoint,
    pub request_hash: Digest,
    pub tools: BTreeMap<String, Value>,
    pub output: Output,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<crate::recipe::Signed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe_response_bytes: Option<usize>,
}
impl fmt::Debug for Policy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Policy")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}
pub(crate) fn bad_schema(parameter: &'static str) -> Failure {
    let mut f = Failure::new(
        Code::InvalidSchema,
        Scope::Request,
        Stage::BeforeDispatch,
        Execution::NotDispatched,
    );
    f.parameter = Some(parameter);
    f
}
fn bad_output() -> Failure {
    Failure::new(
        Code::UpstreamProtocol,
        Scope::Model,
        Stage::ResponseBody,
        Execution::Unknown,
    )
}

impl Policy {
    /// Cheap extraction only. Compilation/format/keyword/reference validation is
    /// performed in the worker and must succeed before an upstream POST.
    pub(crate) fn from_request(
        endpoint: ProxyEndpoint,
        request_hash: Digest,
        request: &Value,
    ) -> Result<Self, Failure> {
        let mut tools = BTreeMap::new();
        if let Some(entries) = request.get("tools").and_then(Value::as_array) {
            for tool in entries {
                let f = if endpoint == ProxyEndpoint::Responses {
                    tool
                } else {
                    &tool["function"]
                };
                let name = f
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| bad_schema("tools"))?;
                if tools
                    .insert(
                        name.to_owned(),
                        f.get("parameters")
                            .cloned()
                            .unwrap_or_else(|| json!({"type":"object"})),
                    )
                    .is_some()
                {
                    return Err(bad_schema("tools"));
                }
            }
        }
        let parameter = if endpoint == ProxyEndpoint::Responses {
            "text"
        } else {
            "response_format"
        };
        let raw = if endpoint == ProxyEndpoint::Responses {
            request.pointer("/text/format")
        } else {
            request.get("response_format")
        };
        let output = match raw {
            None => Output::Text,
            Some(raw) => match raw.get("type").and_then(Value::as_str) {
                Some("text") => Output::Text,
                Some("json_object") => Output::JsonObject,
                Some("json_schema") => {
                    let schema = if endpoint == ProxyEndpoint::Responses {
                        raw.get("schema")
                    } else {
                        raw.pointer("/json_schema/schema")
                    }
                    .cloned()
                    .ok_or_else(|| bad_schema(parameter))?;
                    Output::JsonSchema { schema }
                }
                _ => return Err(bad_schema(parameter)),
            },
        };
        let policy = Self {
            endpoint,
            request_hash,
            tools,
            output,
            recipe: None,
            recipe_response_bytes: None,
        };
        policy.bytes()?;
        Ok(policy)
    }
    pub(crate) fn bytes(&self) -> Result<Vec<u8>, Failure> {
        // Schema bytes already originate in the bounded request. This also bounds
        // total policy transfer/compilation across all tools, not only each schema.
        let bytes = serde_json::to_vec(self).map_err(|_| bad_schema("tools"))?;
        if bytes.len() > MAX_POLICY_BYTES {
            return Err(bad_schema("tools"));
        }
        Ok(bytes)
    }
    pub fn digest(&self) -> Result<Digest, Failure> {
        self.bytes()?;
        let value = serde_json::to_value(self).map_err(|_| bad_schema("tools"))?;
        crate::endpoint::digest("mayhem/proxy/semantic-policy/v1", &value)
            .map_err(|_| bad_schema("tools"))
    }
}

struct NoRetrieval;
impl jsonschema::Retrieve for NoRetrieval {
    fn retrieve(
        &self,
        _uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external schema retrieval is disabled".into())
    }
}
#[derive(Default)]
struct Budget {
    nodes: usize,
    patterns: usize,
}

fn schema_tree(schema: &Value, depth: usize, budget: &mut Budget) -> Result<(), ()> {
    budget.nodes += 1;
    if depth > MAX_DEPTH || budget.nodes > MAX_NODES {
        return Err(());
    }
    if schema.is_boolean() {
        return Ok(());
    }
    let object = schema.as_object().ok_or(())?;
    for (key, value) in object {
        match key.as_str() {
            "$schema" => {
                if !matches!(
                    value.as_str(),
                    Some(
                        "https://json-schema.org/draft/2020-12/schema"
                            | "https://json-schema.org/draft/2020-12/schema#"
                    )
                ) {
                    return Err(());
                }
            }
            // No change of resolution base; fragment-local references are the
            // supported offline subset. Retriever is denied independently below.
            "$id" => return Err(()),
            "$ref" | "$dynamicRef" => {
                if !value.as_str().is_some_and(|r| r.starts_with('#')) {
                    return Err(());
                }
            }
            "$defs" | "definitions" | "properties" | "patternProperties" | "dependentSchemas" => {
                let children = value.as_object().ok_or(())?;
                if key == "patternProperties" {
                    budget.patterns = budget.patterns.checked_add(children.len()).ok_or(())?;
                }
                for child in children.values() {
                    schema_tree(child, depth + 1, budget)?;
                }
            }
            "items"
            | "additionalProperties"
            | "propertyNames"
            | "contains"
            | "not"
            | "if"
            | "then"
            | "else"
            | "unevaluatedProperties"
            | "unevaluatedItems" => schema_tree(value, depth + 1, budget)?,
            "prefixItems" | "allOf" | "anyOf" | "oneOf" => {
                for child in value.as_array().ok_or(())? {
                    schema_tree(child, depth + 1, budget)?;
                }
            }
            "pattern" => budget.patterns += 1,
            "$anchor" | "$dynamicAnchor" | "$comment" | "title" | "description" | "default"
            | "examples" | "deprecated" | "readOnly" | "writeOnly" | "type" | "enum" | "const"
            | "required" | "minProperties" | "maxProperties" | "dependentRequired"
            | "minContains" | "maxContains" | "minItems" | "maxItems" | "uniqueItems"
            | "minLength" | "maxLength" | "format" | "minimum" | "maximum" | "exclusiveMinimum"
            | "exclusiveMaximum" | "multipleOf" => (),
            _ => return Err(()),
        }
        if budget.patterns > MAX_PATTERNS {
            return Err(());
        }
    }
    Ok(())
}
fn compile(
    schema: &Value,
    budget: &mut Budget,
    parameter: &'static str,
) -> Result<jsonschema::Validator, Failure> {
    if serde_json::to_vec(schema)
        .map_err(|_| bad_schema(parameter))?
        .len()
        > MAX_SCHEMA_BYTES
    {
        return Err(bad_schema(parameter));
    }
    schema_tree(schema, 0, budget).map_err(|_| bad_schema(parameter))?;
    jsonschema::draft202012::options()
        .with_retriever(NoRetrieval)
        .should_validate_formats(true)
        .should_ignore_unknown_formats(false)
        .with_pattern_options(
            jsonschema::PatternOptions::fancy_regex()
                .backtrack_limit(20_000)
                .size_limit(128 * 1024)
                .dfa_size_limit(128 * 1024),
        )
        .build(schema)
        .map_err(|_| bad_schema(parameter))
}

pub(crate) struct Verifier {
    endpoint: ProxyEndpoint,
    tools: BTreeMap<String, jsonschema::Validator>,
    output: Option<jsonschema::Validator>,
    json_object: bool,
    recipe: Option<(crate::recipe::Signed, usize)>,
    stream_state: crate::recipe::stream::State,
}
impl Verifier {
    pub(crate) fn new(policy: Policy) -> Result<Self, Failure> {
        policy.bytes()?;
        if policy.recipe.is_some() != policy.recipe_response_bytes.is_some() {
            return Err(bad_schema("input"));
        }
        if let Some(recipe) = &policy.recipe {
            recipe.validate().map_err(|_| bad_schema("input"))?;
            if recipe.recipe.endpoint != policy.endpoint
                || !policy
                    .recipe_response_bytes
                    .is_some_and(|v| (1..=256 * 1024 * 1024).contains(&v))
            {
                return Err(bad_schema("input"));
            }
        }
        let mut budget = Budget::default();
        let mut tools = BTreeMap::new();
        for (name, schema) in policy.tools {
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            {
                return Err(bad_schema("tools"));
            }
            tools.insert(name, compile(&schema, &mut budget, "tools")?);
        }
        let json_object = matches!(policy.output, Output::JsonObject);
        let parameter = if policy.endpoint == ProxyEndpoint::Responses {
            "text"
        } else {
            "response_format"
        };
        let output = match policy.output {
            Output::JsonSchema { schema } => Some(compile(&schema, &mut budget, parameter)?),
            _ => None,
        };
        Ok(Self {
            endpoint: policy.endpoint,
            tools,
            output,
            json_object,
            recipe: policy.recipe.zip(policy.recipe_response_bytes),
            stream_state: Default::default(),
        })
    }
    fn tool(&self, name: &str, args: &str) -> Result<(), Failure> {
        let validator = self.tools.get(name).ok_or_else(bad_output)?;
        let value: Value = serde_json::from_str(args).map_err(|_| bad_output())?;
        if !value.is_object() {
            return Err(bad_output());
        }
        // validate() reports regex execution limits as errors, not successful
        // matches. Never repair values to satisfy the schema.
        validator.validate(&value).map_err(|_| bad_output())
    }
    fn text(&self, text: &str) -> Result<(), Failure> {
        if self.output.is_none() && !self.json_object {
            return Ok(());
        }
        let value: Value = serde_json::from_str(text).map_err(|_| bad_output())?;
        if self.json_object && !value.is_object() {
            return Err(bad_output());
        }
        if let Some(validator) = &self.output {
            validator.validate(&value).map_err(|_| bad_output())?;
        }
        Ok(())
    }
    pub(crate) fn stream_frame(
        &mut self,
        frame: crate::connector::framing::Frame,
    ) -> Result<Option<crate::worker::Decoded>, Failure> {
        let Some((recipe, limit)) = &self.recipe else {
            return Ok(Some(match frame {
                crate::connector::framing::Frame::Sse { event, data, id } => {
                    crate::worker::Decoded::Sse { event, data, id }
                }
                crate::connector::framing::Frame::Ndjson(value) => {
                    crate::worker::Decoded::Ndjson { value }
                }
            }));
        };
        let stream = recipe.recipe.stream.as_ref().ok_or_else(bad_output)?;
        stream.frame(
            frame,
            recipe.recipe.max_json_bytes.min(*limit),
            &mut self.stream_state,
        )
    }
    pub(crate) fn finish_frames(&self) -> Result<(), Failure> {
        if let Some((recipe, _)) = &self.recipe {
            recipe
                .recipe
                .stream
                .as_ref()
                .ok_or_else(bad_output)?
                .finish(&self.stream_state)?;
        }
        Ok(())
    }
    pub(crate) fn normalize(&self, value: Value) -> Result<Value, Failure> {
        match &self.recipe {
            Some((recipe, limit)) => recipe.recipe.map_response(&value, *limit),
            None => Ok(value),
        }
    }
    pub(crate) fn verify(&self, value: &Value) -> Result<(), Failure> {
        match self.endpoint {
            ProxyEndpoint::Chat => {
                let choices = value
                    .get("choices")
                    .and_then(Value::as_array)
                    .ok_or_else(bad_output)?;
                for choice in choices {
                    let message = &choice["message"];
                    let calls = message.get("tool_calls").filter(|v| !v.is_null());
                    let mut has_calls = false;
                    if let Some(calls) = calls {
                        for call in calls.as_array().ok_or_else(bad_output)? {
                            has_calls = true;
                            self.tool(
                                call.pointer("/function/name")
                                    .and_then(Value::as_str)
                                    .ok_or_else(bad_output)?,
                                call.pointer("/function/arguments")
                                    .and_then(Value::as_str)
                                    .ok_or_else(bad_output)?,
                            )?;
                        }
                    }
                    let refusal = message
                        .get("refusal")
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty());
                    if !has_calls
                        && !refusal
                        && choice.get("finish_reason").and_then(Value::as_str) == Some("stop")
                    {
                        self.text(message.get("content").and_then(Value::as_str).unwrap_or(""))?;
                    }
                }
            }
            ProxyEndpoint::Completions => {
                for choice in value
                    .get("choices")
                    .and_then(Value::as_array)
                    .ok_or_else(bad_output)?
                {
                    if choice.get("finish_reason").and_then(Value::as_str) == Some("stop") {
                        self.text(
                            choice
                                .get("text")
                                .and_then(Value::as_str)
                                .ok_or_else(bad_output)?,
                        )?;
                    }
                }
            }
            ProxyEndpoint::Responses => {
                for item in value
                    .get("output")
                    .and_then(Value::as_array)
                    .ok_or_else(bad_output)?
                {
                    match item.get("type").and_then(Value::as_str) {
                        Some("function_call")
                            if !(value.get("status").and_then(Value::as_str)
                                == Some("incomplete")
                                && item.get("status").and_then(Value::as_str)
                                    == Some("incomplete")) =>
                        {
                            self.tool(
                                item.get("name")
                                    .and_then(Value::as_str)
                                    .ok_or_else(bad_output)?,
                                item.get("arguments")
                                    .and_then(Value::as_str)
                                    .ok_or_else(bad_output)?,
                            )?
                        }
                        Some("message")
                            if value.get("status").and_then(Value::as_str) == Some("completed") =>
                        {
                            let parts = item
                                .get("content")
                                .and_then(Value::as_array)
                                .ok_or_else(bad_output)?;
                            let mut text = String::new();
                            let mut refusal = false;
                            for part in parts {
                                match part.get("type").and_then(Value::as_str) {
                                    Some("output_text") => text.push_str(
                                        part.get("text")
                                            .and_then(Value::as_str)
                                            .ok_or_else(bad_output)?,
                                    ),
                                    Some("refusal") => refusal = true,
                                    _ => return Err(bad_output()),
                                }
                            }
                            if !refusal {
                                self.text(&text)?;
                            }
                        }
                        _ => (),
                    }
                }
            }
            ProxyEndpoint::Decisions => (), // Request-bound typed checks remain in endpoint.rs.
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy(schema: Value) -> Policy {
        Policy {
            endpoint: ProxyEndpoint::Chat,
            request_hash: Digest::new("1".repeat(64)).unwrap(),
            tools: BTreeMap::new(),
            output: Output::JsonSchema { schema },
            recipe: None,
            recipe_response_bytes: None,
        }
    }
    fn reply(content: &str) -> Value {
        json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":content}}]})
    }
    #[test]
    fn original_schema_constraints_are_enforced_without_projection_or_repairs() {
        let schema = json!({"type":"object","properties":{"ids":{"type":"array","minItems":2,"maxItems":4,"uniqueItems":true,"items":{"type":"string","pattern":"^E(?:[1-9]|1[0-6])$"}}},"required":["ids"],"additionalProperties":false});
        let v = Verifier::new(policy(schema)).unwrap();
        assert!(v.verify(&reply(r#"{"ids":["E1","E16"]}"#)).is_ok());
        for bad in [
            r#"{"ids":["E1","E1"]}"#,
            r#"{"ids":["E1","E17"]}"#,
            r#"{"ids":["E1"]}"#,
            r#"{"ids":["E1","E2"],"extra":1}"#,
        ] {
            assert!(v.verify(&reply(bad)).is_err());
        }
    }
    #[test]
    fn offline_refs_formats_and_unknown_semantics_fail_before_dispatch() {
        for schema in [
            json!({"$ref":"https://invalid.example/schema"}),
            json!({"$ref":"file:///tmp/not-read"}),
            json!({"$id":"https://invalid.example/root","$ref":"#/x"}),
            json!({"type":"string","format":"invented-format"}),
            json!({"unknown_validation":true}),
            json!({"type":"object","properties":{"x":{"type":"string","pattern":"["}}}),
        ] {
            let error = Verifier::new(policy(schema)).err().unwrap();
            assert_eq!(error.code, Code::InvalidSchema);
            assert_eq!(error.execution, Execution::NotDispatched);
        }
        let v = Verifier::new(policy(
            json!({"$defs":{"id":{"type":"integer"}},"$ref":"#/$defs/id"}),
        ))
        .unwrap();
        assert!(v.verify(&reply("3")).is_ok());
        assert!(v.verify(&reply("3.5")).is_err());
        let v = Verifier::new(policy(
            json!({"const":{"$ref":"https://not-a-schema-reference.invalid"}}),
        ))
        .unwrap();
        assert!(v
            .verify(&reply(
                r#"{"$ref":"https://not-a-schema-reference.invalid"}"#
            ))
            .is_ok());
    }
    #[test]
    fn tool_arguments_are_typed_and_normal_refusals_or_incomplete_text_are_preserved() {
        let mut p = policy(json!({"type":"object"}));
        p.tools.insert("read_file".into(),json!({"type":"object","required":["path"],"properties":{"path":{"type":"string"}},"additionalProperties":false}));
        let v = Verifier::new(p).unwrap();
        let mut r = reply("");
        r["choices"][0]["finish_reason"] = json!("tool_calls");
        r["choices"][0]["message"]["tool_calls"] =
            json!([{"function":{"name":"read_file","arguments":"{\"path\":\"src/game.js\"}"}}]);
        assert!(v.verify(&r).is_ok());
        r["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"] =
            json!("{\"path\":3}");
        assert!(v.verify(&r).is_err());
        let mut refusal = reply("I cannot do that");
        refusal["choices"][0]["message"]["refusal"] = json!("No");
        assert!(v.verify(&refusal).is_ok());
        let mut cut = reply("{\"unfinished\":");
        cut["choices"][0]["finish_reason"] = json!("length");
        assert!(v.verify(&cut).is_ok());
    }
    #[test]
    fn aggregate_schema_depth_nodes_patterns_and_bytes_are_bounded() {
        let mut deep = json!({"type":"integer"});
        for _ in 0..70 {
            deep = json!({"items":deep});
        }
        assert!(Verifier::new(policy(deep)).is_err());
        let branches = vec![json!({"pattern":"x"}); 65];
        assert!(Verifier::new(policy(json!({"allOf":branches}))).is_err());
        assert!(
            Verifier::new(policy(json!({"description":"a".repeat(MAX_SCHEMA_BYTES)}))).is_err()
        );
        assert!(Verifier::new(policy(json!({"allOf":vec![json!(true);MAX_NODES+1]}))).is_err());
    }
}
