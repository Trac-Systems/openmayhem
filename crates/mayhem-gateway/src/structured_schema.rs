//! The public JSON-schema contract is independent of the grammar compiler used
//! by any one model backend. Compile the original before dispatch, feed a safe
//! projection to the grammar compiler, and check the original after generation.

use serde_json::Value;

const MAX_SCHEMA_BYTES: usize = 128 * 1024;
const MAX_SCHEMA_DEPTH: usize = 64;
const MAX_SCHEMA_NODES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaError(pub String);

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SchemaError {}

/// Check the real contract before a buyer reserves credit or selects a route.
/// The returned schema is only a generation hint; it must never replace the
/// original schema in the request or in final validation.
pub fn prepare(schema: &Value) -> Result<Value, SchemaError> {
    if !schema.is_object() {
        return Err(SchemaError("JSON schema must be an object".to_owned()));
    }
    if serde_json::to_vec(schema)
        .map_err(|error| SchemaError(format!("invalid JSON schema: {error}")))?
        .len()
        > MAX_SCHEMA_BYTES
    {
        return Err(SchemaError("JSON schema exceeds 128 KiB".to_owned()));
    }
    let mut grammar = schema.clone();
    let mut nodes = 0;
    check_and_project(schema, &mut grammar, "$", 0, &mut nodes)?;
    jsonschema::draft202012::options()
        .should_validate_formats(true)
        .build(schema)
        .map_err(|error| SchemaError(format!("invalid JSON schema: {error}")))?;
    Ok(grammar)
}

pub fn validate_output(schema: &Value, output: &str) -> Result<(), SchemaError> {
    // Prepare also rejects unknown keywords, remote references and excessive
    // complexity. This call is a defensive boundary for provider-side use.
    prepare(schema)?;
    let value: Value = serde_json::from_str(output)
        .map_err(|error| SchemaError(format!("output is not valid JSON: {error}")))?;
    let validator = jsonschema::draft202012::options()
        .should_validate_formats(true)
        .build(schema)
        .map_err(|error| SchemaError(format!("invalid JSON schema: {error}")))?;
    validator
        .validate(&value)
        .map_err(|error| SchemaError(format!("output violates JSON schema: {error}")))
}

fn check_and_project(
    original: &Value,
    grammar: &mut Value,
    path: &str,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), SchemaError> {
    *nodes += 1;
    if depth > MAX_SCHEMA_DEPTH || *nodes > MAX_SCHEMA_NODES {
        return Err(SchemaError("JSON schema is too complex".to_owned()));
    }
    if original.is_boolean() {
        return Ok(());
    }
    let Some(object) = original.as_object() else {
        return Err(SchemaError(format!("{path} must be a JSON schema object")));
    };
    let projected = grammar
        .as_object_mut()
        .expect("schema projection is an object");
    for (key, value) in object {
        if !known_keyword(key) {
            return Err(SchemaError(format!(
                "unsupported JSON schema keyword {key:?} at {path}"
            )));
        }
        if key == "$ref" {
            let reference = value
                .as_str()
                .ok_or_else(|| SchemaError(format!("{path}.$ref must be a string")))?;
            if !reference.starts_with("#/") && reference != "#" {
                return Err(SchemaError(format!(
                    "non-local JSON schema reference at {path}"
                )));
            }
        }
        if key == "$schema" {
            let draft = value
                .as_str()
                .ok_or_else(|| SchemaError(format!("{path}.$schema must be a string")))?;
            if !matches!(
                draft,
                "https://json-schema.org/draft/2020-12/schema"
                    | "https://json-schema.org/draft/2020-12/schema#"
            ) {
                return Err(SchemaError(format!(
                    "unsupported JSON schema draft at {path}"
                )));
            }
        }
        // Stateful or semantic constraints do not belong in a stateless token
        // grammar. The full schema is retained and validated after generation.
        if matches!(
            key.as_str(),
            "uniqueItems"
                | "multipleOf"
                | "format"
                | "contains"
                | "minContains"
                | "maxContains"
                | "patternProperties"
                | "propertyNames"
                | "dependentRequired"
                | "dependentSchemas"
                | "unevaluatedProperties"
                | "unevaluatedItems"
                | "if"
                | "then"
                | "else"
                | "not"
                | "title"
                | "description"
                | "default"
                | "examples"
                | "deprecated"
                | "readOnly"
                | "writeOnly"
                | "$comment"
        ) {
            match key.as_str() {
                "patternProperties" | "dependentSchemas" => {
                    let children = value
                        .as_object()
                        .ok_or_else(|| SchemaError(format!("{path}.{key} must be an object")))?;
                    for (name, child) in children {
                        let mut ignored = child.clone();
                        check_and_project(
                            child,
                            &mut ignored,
                            &format!("{path}.{key}.{name}"),
                            depth + 1,
                            nodes,
                        )?;
                    }
                }
                "contains"
                | "unevaluatedProperties"
                | "unevaluatedItems"
                | "propertyNames"
                | "if"
                | "then"
                | "else"
                | "not" => {
                    let mut ignored = value.clone();
                    check_and_project(
                        value,
                        &mut ignored,
                        &format!("{path}.{key}"),
                        depth + 1,
                        nodes,
                    )?;
                }
                _ => {}
            }
            projected.remove(key);
            continue;
        }
        match key.as_str() {
            "properties" | "$defs" | "definitions" | "patternProperties" | "dependentSchemas" => {
                let children = value
                    .as_object()
                    .ok_or_else(|| SchemaError(format!("{path}.{key} must be an object")))?;
                for (name, child) in children {
                    check_and_project(
                        child,
                        projected
                            .get_mut(key)
                            .and_then(|v| v.get_mut(name))
                            .unwrap(),
                        &format!("{path}.{key}.{name}"),
                        depth + 1,
                        nodes,
                    )?;
                }
            }
            "items"
            | "additionalProperties"
            | "unevaluatedProperties"
            | "unevaluatedItems"
            | "contains"
            | "propertyNames"
            | "not"
            | "if"
            | "then"
            | "else" => {
                check_and_project(
                    value,
                    projected.get_mut(key).unwrap(),
                    &format!("{path}.{key}"),
                    depth + 1,
                    nodes,
                )?;
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                let children = value
                    .as_array()
                    .ok_or_else(|| SchemaError(format!("{path}.{key} must be an array")))?;
                for (index, child) in children.iter().enumerate() {
                    check_and_project(
                        child,
                        &mut projected
                            .get_mut(key)
                            .and_then(Value::as_array_mut)
                            .unwrap()[index],
                        &format!("{path}.{key}[{index}]"),
                        depth + 1,
                        nodes,
                    )?;
                }
            }
            _ => {}
        }
    }
    if object.contains_key("patternProperties")
        && projected.get("additionalProperties") == Some(&Value::Bool(false))
    {
        projected.insert("additionalProperties".to_owned(), Value::Bool(true));
    }
    Ok(())
}

fn known_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        "$schema"
            | "$id"
            | "$comment"
            | "$defs"
            | "$ref"
            | "definitions"
            | "title"
            | "description"
            | "default"
            | "examples"
            | "deprecated"
            | "readOnly"
            | "writeOnly"
            | "type"
            | "enum"
            | "const"
            | "properties"
            | "patternProperties"
            | "required"
            | "additionalProperties"
            | "propertyNames"
            | "minProperties"
            | "maxProperties"
            | "dependentRequired"
            | "dependentSchemas"
            | "unevaluatedProperties"
            | "items"
            | "prefixItems"
            | "contains"
            | "minContains"
            | "maxContains"
            | "minItems"
            | "maxItems"
            | "uniqueItems"
            | "unevaluatedItems"
            | "minLength"
            | "maxLength"
            | "pattern"
            | "format"
            | "minimum"
            | "maximum"
            | "exclusiveMinimum"
            | "exclusiveMaximum"
            | "multipleOf"
            | "allOf"
            | "anyOf"
            | "oneOf"
            | "not"
            | "if"
            | "then"
            | "else"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nested_uniqueness_is_projected_but_finally_enforced() {
        let schema = json!({"type":"object","properties":{"signals":{"type":"array","items":{
            "type":"object","properties":{"evidenceIds":{"type":"array","items":{"type":"string"},
                "minItems":2,"maxItems":4,"uniqueItems":true}}
        }}}});
        let projected = prepare(&schema).unwrap();
        assert!(
            projected["properties"]["signals"]["items"]["properties"]["evidenceIds"]
                .get("uniqueItems")
                .is_none()
        );
        assert!(
            schema["properties"]["signals"]["items"]["properties"]["evidenceIds"]
                .get("uniqueItems")
                .is_some()
        );
        assert!(validate_output(&schema, r#"{"signals":[{"evidenceIds":["E1","E2"]}]}"#).is_ok());
        assert!(validate_output(&schema, r#"{"signals":[{"evidenceIds":["E1","E1"]}]}"#).is_err());
    }

    #[test]
    fn schema_keywords_in_const_data_are_not_changed() {
        let schema = json!({"type":"object","properties":{"value":{"const":{"uniqueItems":true}}}});
        assert_eq!(prepare(&schema).unwrap(), schema);
    }

    #[test]
    fn invalid_and_unknown_constraints_fail_before_dispatch() {
        assert!(prepare(&json!({"type":"array","uniqueItems":"true"})).is_err());
        assert!(prepare(&json!({"type":"array","madeUpConstraint":true})).is_err());
        assert!(prepare(&json!({"$ref":"https://example.org/schema"})).is_err());
        assert!(prepare(&json!({"type":"array","contains":{"madeUpConstraint":true}})).is_err());
        assert!(prepare(&json!({"$schema":"http://json-schema.org/draft-07/schema#"})).is_err());
        assert!(prepare(&json!({"type":"object","additionalProperties":false})).is_ok());
    }
}
