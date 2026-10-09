use super::*;
use mayhem_proto::{
    validate_endpoint_attribute_value, validate_endpoint_request, EndpointFamilyContract,
};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    pub field_id: String,
    pub schema_revision: u32,
    pub value: TypedValue,
}

impl Control {
    pub fn validate(&self) -> Result<()> {
        require(
            identifier(&self.field_id)
                && self.schema_revision > 0
                && self.schema_revision <= i32::MAX as u32,
            "invalid registry control reference",
        )?;
        self.value.validate()
    }
}

pub(super) fn safe_path(path: &str) -> bool {
    let parts: Vec<_> = path.split('.').collect();
    !path.is_empty() && path.len() <= 256 && parts.len() <= 8
        && parts.iter().all(|p| !p.is_empty() && p.len() <= 64 && p.as_bytes()[0].is_ascii_lowercase()
            && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            && !matches!(*p, "constructor" | "prototype" | "__proto__"))
        // Transport, identity, conversation content and routing are owned by
        // their normal APIs, not an admin-editable generic control mapping.
        && !matches!(parts[0], "model" | "proxy" | "messages" | "prompt" | "input" | "context"
            | "questions" | "stream" | "stream_options" | "previous_response_id" | "conversation"
            | "metadata" | "user" | "headers" | "authorization" | "api_key" | "base_url" | "url")
}

fn read<'a>(root: &'a Value, path: &str) -> Result<Option<&'a Value>> {
    let mut value = root;
    for part in path.split('.') {
        let object = value
            .as_object()
            .ok_or_else(|| invalid("control path traverses a non-object"))?;
        let Some(next) = object.get(part) else {
            return Ok(None);
        };
        value = next;
    }
    Ok(Some(value))
}
fn write(root: &mut Value, path: &str, value: Value) -> Result<()> {
    let (parent, name) = path.rsplit_once('.').unwrap_or(("", path));
    let mut target = root;
    if !parent.is_empty() {
        for part in parent.split('.') {
            target = target
                .as_object_mut()
                .ok_or_else(|| invalid("control path traverses a non-object"))?
                .entry(part)
                .or_insert_with(|| Value::Object(Map::new()));
        }
    }
    target
        .as_object_mut()
        .ok_or_else(|| invalid("control parent is not an object"))?
        .insert(name.into(), value);
    Ok(())
}

/// Resolve only exact requested definitions through an indexed/cache lookup.
/// No registry-wide scan and no dynamic/native callback. The caller enforces
/// its ordinary request byte limit before passing a parsed JSON body here.
/// Return a new body only after the whole combination and endpoint validate;
/// the original body remains unchanged on every error.
pub fn apply_controls<'a>(
    request: &Value,
    endpoint: ProxyEndpoint,
    contract: &EndpointFamilyContract,
    controls: &[Control],
    lookup: impl Fn(&str, u32) -> Option<&'a Definition>,
) -> Result<Value> {
    require(
        controls.len() <= 32 && request.is_object(),
        "invalid registry controls envelope",
    )?;
    require(
        serde_json::to_value(endpoint)?.as_str() == Some(contract.family.as_str()),
        "control endpoint differs",
    )?;
    let mut definitions: BTreeMap<(String, u32), &'a Definition> = BTreeMap::new();
    let mut values: BTreeMap<(String, u32), TypedValue> = BTreeMap::new();
    let mut paths = BTreeSet::new();
    let mut changes = Vec::new();
    for control in controls {
        control.validate()?;
        let definition = lookup(&control.field_id, control.schema_revision)
            .ok_or_else(|| invalid("required registry field revision is unknown"))?;
        definition.validate()?;
        require(
            definition.field_id == control.field_id
                && definition.schema_revision == control.schema_revision
                && definition.endpoints.contains(&endpoint),
            "registry control definition differs",
        )?;
        definition.value_schema.accepts(&control.value)?;
        let Usage::RequestControl { request_path } = &definition.usage else {
            return Err(invalid("field is not a request control"));
        };
        require(
            contract.request_attributes.contains(request_path),
            "endpoint does not support this control mapping",
        )?;
        let value = control.value.json()?;
        let spec = contract
            .request_attribute_specs
            .get(request_path)
            .ok_or_else(|| invalid("endpoint has no typed contract for this control"))?;
        validate_endpoint_attribute_value(spec, &value)
            .map_err(|_| invalid("control violates the actual endpoint contract"))?;
        require(
            paths.iter().all(|p: &String| {
                p != request_path
                    && !p.starts_with(&format!("{request_path}."))
                    && !request_path.starts_with(&format!("{p}."))
            }),
            "registry controls map to overlapping paths",
        )?;
        paths.insert(request_path.clone());
        if let Some(existing) = read(request, request_path)? {
            require(
                existing == &value,
                "explicit request and saved control conflict",
            )?;
        }
        let key = (control.field_id.clone(), control.schema_revision);
        require(
            values.insert(key.clone(), control.value.clone()).is_none()
                && !values
                    .keys()
                    .any(|(id, rev)| id == &control.field_id && *rev != control.schema_revision),
            "duplicate or mixed registry control revisions",
        )?;
        definitions.insert(key, definition);
        changes.push((request_path.clone(), value));
    }
    // Exact references only. A bounded visited queue handles cycles without
    // recursively evaluating rules or scanning the registry. Explicit raw
    // request controls participate too; absent values never acquire defaults.
    let mut queue = definitions.keys().cloned().collect::<Vec<_>>();
    let mut cursor = 0;
    while cursor < queue.len() {
        let current = queue[cursor].clone();
        cursor += 1;
        if !values.contains_key(&current) {
            continue;
        }
        let definition = definitions[&current];
        for rule in &definition.rules {
            for condition in rule.when.iter().chain(&rule.require).chain(&rule.forbid) {
                let key = (condition.field_id.clone(), condition.schema_revision);
                if !definitions.contains_key(&key) {
                    require(
                        definitions.len() < MAX_FIELDS_PER_REQUEST,
                        "too many referenced registry fields",
                    )?;
                    let referenced = lookup(&condition.field_id, condition.schema_revision)
                        .ok_or_else(|| invalid("conditional registry field revision is unknown"))?;
                    referenced.validate()?;
                    require(
                        !definitions.keys().any(|(id, rev)| {
                            id == &condition.field_id && *rev != condition.schema_revision
                        }),
                        "mixed conditional registry revisions",
                    )?;
                    definitions.insert(key.clone(), referenced);
                    queue.push(key.clone());
                }
                condition.check_reference(definitions[&key], endpoint)?;
                let referenced = definitions[&key];
                let Usage::RequestControl { request_path } = &referenced.usage else {
                    return Err(invalid("control conditions require request-control fields"));
                };
                require(
                    contract.request_attributes.contains(request_path),
                    "endpoint does not support this conditional control mapping",
                )?;
                let spec = contract
                    .request_attribute_specs
                    .get(request_path)
                    .ok_or_else(|| invalid("endpoint has no typed conditional control contract"))?;
                if !values.contains_key(&key) {
                    if let Some(raw) = read(request, request_path)? {
                        validate_endpoint_attribute_value(spec, raw).map_err(|_| {
                            invalid("conditional control violates endpoint contract")
                        })?;
                        values.insert(key.clone(), referenced.value_schema.read_json(raw)?);
                    }
                }
            }
            let matches = |condition: &Condition| -> Result<Option<bool>> {
                values
                    .get(&(condition.field_id.clone(), condition.schema_revision))
                    .map(|v| v.matches(condition.operator, &condition.value))
                    .transpose()
            };
            let when = rule.when.iter().map(matches).collect::<Result<Vec<_>>>()?;
            if when.contains(&Some(false)) {
                continue;
            }
            require(
                !when.contains(&None),
                "conditional control needs an explicit value",
            )?;
            for condition in &rule.require {
                require(
                    matches(condition)? == Some(true),
                    "required control combination is not satisfied",
                )?;
            }
            for condition in &rule.forbid {
                require(
                    matches(condition)? == Some(false),
                    "forbidden control combination cannot be excluded",
                )?;
            }
        }
    }
    let mut result = request.clone();
    for (path, value) in changes {
        write(&mut result, &path, value)?;
    }
    validate_endpoint_request(contract, &result)
        .map_err(|_| invalid("complete request violates the selected endpoint contract"))?;
    Ok(result)
}
