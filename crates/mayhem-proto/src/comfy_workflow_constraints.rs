//! Optional signed constraints for workflows whose metering assumes one output path.
//! Roles belong to policy data, never to provider or request supplied annotations.
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::comfy_workflow::{ComfyWorkflowDerivationError as Error, ComfyWorkflowPartRef};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyWorkflowDimensionBounds {
    pub min_width: u64,
    pub min_height: u64,
    /// Per-output canvas area; batch remains bounded by policy.max_artifacts.
    pub max_pixels: u64,
    pub width_multiple: u64,
    pub height_multiple: u64,
}

impl ComfyWorkflowDimensionBounds {
    pub(crate) fn validate(&self, max_width: u64, max_height: u64) -> Result<(), Error> {
        if self.min_width == 0
            || self.min_height == 0
            || self.max_pixels == 0
            || self.width_multiple == 0
            || self.height_multiple == 0
            || self.min_width > max_width
            || self.min_height > max_height
        {
            return Err(policy_error(
                "dimension bounds must be positive and ordered",
            ));
        }
        let width = self
            .min_width
            .checked_add(self.width_multiple - 1)
            .map(|v| v / self.width_multiple * self.width_multiple);
        let height = self
            .min_height
            .checked_add(self.height_multiple - 1)
            .map(|v| v / self.height_multiple * self.height_multiple);
        if !width.zip(height).is_some_and(|(w, h)| {
            w <= max_width
                && h <= max_height
                && w.checked_mul(h)
                    .is_some_and(|pixels| pixels <= self.max_pixels)
        }) {
            return Err(policy_error("dimension bounds admit no canvas"));
        }
        Ok(())
    }

    pub(crate) fn check(&self, width: Option<u64>, height: Option<u64>) -> Result<(), Error> {
        let (width, height) = width
            .zip(height)
            .ok_or_else(|| graph_error("bounded image requires width and height"))?;
        if width < self.min_width
            || height < self.min_height
            || width % self.width_multiple != 0
            || height % self.height_multiple != 0
            || !width
                .checked_mul(height)
                .is_some_and(|pixels| pixels <= self.max_pixels)
        {
            return Err(graph_error("image dimensions violate signed bounds"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyWorkflowGraphConstraints {
    /// Exactly one node in this role is the sink; every node must feed it.
    pub output_role: String,
    pub roles: BTreeMap<String, ComfyWorkflowNodeRole>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyWorkflowNodeRole {
    pub class_type: String,
    pub min_count: usize,
    pub max_count: usize,
    /// Every accepted input must have a rule. Roles are matched by class and
    /// scalar/part inputs; a node matching zero or multiple roles is rejected.
    pub inputs: BTreeMap<String, ComfyWorkflowInputConstraint>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComfyWorkflowInputType {
    Integer,
    Number,
    String,
    Boolean,
    Part,
    Link,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyWorkflowLinkSource {
    pub role: String,
    pub output: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyWorkflowValueSource {
    pub role: String,
    pub input: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyWorkflowInputConstraint {
    pub value_type: ComfyWorkflowInputType,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiple_of: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub part_names: Vec<String>,
    /// Rejects selecting the same signed part more than once for this role/input.
    #[serde(default)]
    pub distinct: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<ComfyWorkflowLinkSource>,
    /// Pair model/encoder links when either source has a listed role. This
    /// permits distinct base loaders, but prevents splitting an adapter chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_source_as: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paired_source_roles: Vec<String>,
    /// Require this scalar input to equal an input on another uniquely bound role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_value_as: Option<ComfyWorkflowValueSource>,
}

fn policy_error(message: impl Into<String>) -> Error {
    Error::InvalidPolicy(message.into())
}
fn graph_error(message: impl Into<String>) -> Error {
    Error::InvalidGraph(message.into())
}

// Preserve integer precision for seeds and integer bounds, including u64::MAX.
fn number_cmp(left: &Number, right: &Number) -> Option<Ordering> {
    if let (Some(l), Some(r)) = (left.as_u64(), right.as_u64()) {
        return Some(l.cmp(&r));
    }
    if let (Some(l), Some(r)) = (left.as_i64(), right.as_i64()) {
        return Some(l.cmp(&r));
    }
    if left.as_i64().is_some_and(|v| v < 0) && right.as_u64().is_some() {
        return Some(Ordering::Less);
    }
    if right.as_i64().is_some_and(|v| v < 0) && left.as_u64().is_some() {
        return Some(Ordering::Greater);
    }
    left.as_f64()?.partial_cmp(&right.as_f64()?)
}

fn value_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(l), Value::Number(r)) => number_cmp(l, r) == Some(Ordering::Equal),
        _ => left == right,
    }
}

fn link(value: &Value) -> Option<(&str, u64)> {
    let values = value.as_array()?;
    if values.len() != 2 {
        return None;
    }
    Some((values[0].as_str()?, values[1].as_u64()?))
}

impl ComfyWorkflowInputConstraint {
    pub(crate) fn check(
        &self,
        value: &Value,
        parts: &BTreeMap<String, ComfyWorkflowPartRef>,
    ) -> bool {
        use ComfyWorkflowInputType::*;
        let typed = match self.value_type {
            Integer => value.is_i64() || value.is_u64(),
            Number => value.as_f64().is_some_and(f64::is_finite),
            String | Part => value.is_string(),
            Boolean => value.is_boolean(),
            Link => link(value).is_some(),
        };
        if !typed
            || self
                .fixed
                .as_ref()
                .is_some_and(|fixed| !value_equal(value, fixed))
            || (!self.enum_values.is_empty()
                && !self.enum_values.iter().any(|v| value_equal(value, v)))
        {
            return false;
        }
        if let Some(number) = value.as_number() {
            if self
                .minimum
                .as_ref()
                .is_some_and(|min| number_cmp(number, min) == Some(Ordering::Less))
                || self
                    .maximum
                    .as_ref()
                    .is_some_and(|max| number_cmp(number, max) == Some(Ordering::Greater))
            {
                return false;
            }
            if let Some(multiple) = self.multiple_of {
                let divisible = number.as_u64().is_some_and(|value| value % multiple == 0)
                    || number
                        .as_i64()
                        .is_some_and(|value| i128::from(value) % i128::from(multiple) == 0);
                if !divisible {
                    return false;
                }
            }
        }
        if let Some(string) = value.as_str() {
            let length = string.chars().count();
            if self.min_length.is_some_and(|min| length < min)
                || self.max_length.is_some_and(|max| length > max)
            {
                return false;
            }
            if self.value_type == Part {
                let Some(part) = parts.get(string) else {
                    return false;
                };
                if !self.part_names.iter().any(|name| name == string)
                    || self.part_type.as_ref() != Some(&part.part_type)
                {
                    return false;
                }
            }
        }
        true
    }

    fn validate(
        &self,
        role_name: &str,
        input_name: &str,
        role: &ComfyWorkflowNodeRole,
        roles: &BTreeMap<String, ComfyWorkflowNodeRole>,
        parts: &BTreeMap<String, ComfyWorkflowPartRef>,
    ) -> Result<(), Error> {
        use ComfyWorkflowInputType::*;
        if (self.minimum.is_some() || self.maximum.is_some())
            && !matches!(self.value_type, Integer | Number)
            || self.multiple_of.is_some() && self.value_type != Integer
            || (self.min_length.is_some() || self.max_length.is_some())
                && !matches!(self.value_type, String | Part)
            || (self.part_type.is_some() || !self.part_names.is_empty() || self.distinct)
                && self.value_type != Part
            || (!self.sources.is_empty()
                || self.same_source_as.is_some()
                || !self.paired_source_roles.is_empty())
                && self.value_type != Link
            || self.same_value_as.is_some() && self.value_type == Link
        {
            return Err(policy_error(
                "input constraint fields do not match value_type",
            ));
        }
        if self
            .minimum
            .as_ref()
            .zip(self.maximum.as_ref())
            .is_some_and(|(min, max)| number_cmp(min, max) == Some(Ordering::Greater))
            || self
                .min_length
                .zip(self.max_length)
                .is_some_and(|(min, max)| min > max)
        {
            return Err(policy_error("input constraint bounds are reversed"));
        }
        if self.value_type == Integer
            && [self.minimum.as_ref(), self.maximum.as_ref()]
                .into_iter()
                .flatten()
                .any(|n| !n.is_i64() && !n.is_u64())
        {
            return Err(policy_error("integer input requires integer bounds"));
        }
        if self.multiple_of == Some(0) {
            return Err(policy_error("integer multiple_of must be positive"));
        }
        if self.value_type == Part
            && (self.part_names.is_empty()
                || self.part_type.as_ref().is_none_or(|kind| kind.is_empty())
                || self.part_names.iter().any(|name| {
                    parts
                        .get(name)
                        .is_none_or(|part| Some(&part.part_type) != self.part_type.as_ref())
                }))
        {
            return Err(policy_error(
                "part constraint requires signed names with matching type",
            ));
        }
        if self.value_type == Link {
            if self.sources.is_empty()
                || self
                    .sources
                    .iter()
                    .any(|source| !roles.contains_key(&source.role))
            {
                return Err(policy_error("link constraint requires valid source roles"));
            }
            if self.fixed.is_some() || !self.enum_values.is_empty() {
                return Err(policy_error(
                    "link constraints use sources, not fixed/enum node ids",
                ));
            }
            if let Some(other) = self.same_source_as.as_ref() {
                if role
                    .inputs
                    .get(other)
                    .is_none_or(|input| input.value_type != Link || !input.required)
                    || !self.required
                    || self.paired_source_roles.is_empty()
                    || self
                        .paired_source_roles
                        .iter()
                        .any(|name| !roles.contains_key(name))
                {
                    return Err(policy_error(
                        "paired links require required link inputs and valid roles",
                    ));
                }
            } else if !self.paired_source_roles.is_empty() {
                return Err(policy_error("paired_source_roles requires same_source_as"));
            }
        }
        if let Some(source) = &self.same_value_as {
            let source_role = roles
                .get(&source.role)
                .ok_or_else(|| policy_error("same_value_as requires a valid source role"))?;
            let source_input = source_role
                .inputs
                .get(&source.input)
                .ok_or_else(|| policy_error("same_value_as requires a valid source input"))?;
            if !self.required
                || role.min_count != 1
                || role.max_count != 1
                || source_role.min_count != 1
                || source_role.max_count != 1
                || !source_input.required
                || source_input.value_type != self.value_type
                || (source.role == role_name && source.input == input_name)
            {
                return Err(policy_error(
                    "same_value_as requires distinct required inputs on unique roles of the same type",
                ));
            }
        }
        if self
            .fixed
            .as_ref()
            .is_some_and(|value| !self.check(value, parts))
            || self
                .enum_values
                .iter()
                .any(|value| !self.check(value, parts))
        {
            return Err(policy_error(
                "fixed/enum value violates its input constraint",
            ));
        }
        Ok(())
    }
}

impl ComfyWorkflowGraphConstraints {
    pub(crate) fn validate(
        &self,
        whitelist: &BTreeSet<String>,
        parts: &BTreeMap<String, ComfyWorkflowPartRef>,
        max_nodes: usize,
    ) -> Result<(), Error> {
        let output = self
            .roles
            .get(&self.output_role)
            .ok_or_else(|| policy_error("output_role is missing"))?;
        if output.min_count != 1 || output.max_count != 1 {
            return Err(policy_error("output role must occur exactly once"));
        }
        let mut min_nodes = 0usize;
        for (name, role) in &self.roles {
            if name.trim().is_empty()
                || !whitelist.contains(&role.class_type)
                || role.min_count > role.max_count
                || role.max_count == 0
                || role.max_count > max_nodes
            {
                return Err(policy_error(format!("invalid node role {name}")));
            }
            min_nodes = min_nodes
                .checked_add(role.min_count)
                .ok_or_else(|| policy_error("role count overflow"))?;
            for (input, rule) in &role.inputs {
                if input.trim().is_empty() {
                    return Err(policy_error("input name is empty"));
                }
                rule.validate(name, input, role, &self.roles, parts)?;
            }
        }
        if min_nodes > max_nodes {
            return Err(policy_error("required roles exceed max_nodes"));
        }
        Ok(())
    }

    pub(crate) fn check(
        &self,
        nodes: &serde_json::Map<String, Value>,
        parts: &BTreeMap<String, ComfyWorkflowPartRef>,
        required_parts: &mut BTreeMap<String, ComfyWorkflowPartRef>,
    ) -> Result<(), Error> {
        let mut candidates = BTreeMap::<&str, BTreeSet<&str>>::new();
        for (node_id, node) in nodes {
            let class = node
                .get("class_type")
                .and_then(Value::as_str)
                .ok_or_else(|| graph_error("node class_type is missing"))?;
            let inputs = node
                .get("inputs")
                .and_then(Value::as_object)
                .ok_or_else(|| graph_error(format!("node {node_id} requires inputs object")))?;
            let matches = self
                .roles
                .iter()
                .filter(|(_, role)| {
                    role.class_type == class
                        && inputs.keys().all(|key| role.inputs.contains_key(key))
                        && role.inputs.iter().all(|(key, rule)| match inputs.get(key) {
                            Some(value) => rule.check(value, parts),
                            None => !rule.required,
                        })
                })
                .map(|(name, _)| name.as_str())
                .collect::<BTreeSet<_>>();
            if matches.is_empty() {
                return Err(graph_error(format!(
                    "node {node_id} must match a signed role"
                )));
            }
            candidates.insert(node_id.as_str(), matches);
        }

        // A class and its scalar inputs need not identify a role. Positive and
        // negative encoders, for example, intentionally have the same shape.
        // Refine both ends of every signed link until role candidates stabilize.
        loop {
            let previous = candidates.clone();
            for (node_id, node) in nodes {
                let inputs = node["inputs"].as_object().expect("validated inputs");
                let retained = previous[node_id.as_str()]
                    .iter()
                    .copied()
                    .filter(|role_name| {
                        let role = &self.roles[*role_name];
                        role.inputs.iter().all(|(input_name, rule)| {
                            if rule.value_type != ComfyWorkflowInputType::Link {
                                return true;
                            }
                            let Some(value) = inputs.get(input_name) else {
                                return true;
                            };
                            let Some((source_id, output)) = link(value) else {
                                return false;
                            };
                            previous.get(source_id).is_some_and(|source_roles| {
                                source_roles.iter().any(|source_role| {
                                    rule.sources.iter().any(|source| {
                                        source.role == *source_role && source.output == output
                                    })
                                })
                            })
                        })
                    })
                    .collect::<BTreeSet<_>>();
                candidates.insert(node_id.as_str(), retained);
            }
            // Apply every consumer edge in reverse as well. This is what
            // distinguishes otherwise identical producer roles.
            for (node_id, node) in nodes {
                let inputs = node["inputs"].as_object().expect("validated inputs");
                for (input_name, value) in inputs {
                    let Some((source_id, output)) = link(value) else {
                        continue;
                    };
                    let allowed = candidates[node_id.as_str()]
                        .iter()
                        .filter_map(|role_name| self.roles[*role_name].inputs.get(input_name))
                        .filter(|rule| rule.value_type == ComfyWorkflowInputType::Link)
                        .flat_map(|rule| {
                            rule.sources.iter().filter_map(move |source| {
                                (source.output == output).then_some(source.role.as_str())
                            })
                        })
                        .collect::<BTreeSet<_>>();
                    let Some(source_candidates) = candidates.get_mut(source_id) else {
                        return Err(graph_error("link source does not exist"));
                    };
                    source_candidates.retain(|role_name| allowed.contains(role_name));
                }
            }
            if candidates.values().any(BTreeSet::is_empty) {
                return Err(graph_error("workflow links violate signed roles"));
            }
            if candidates == previous {
                break;
            }
        }

        let mut assignments = BTreeMap::new();
        for (node_id, matches) in &candidates {
            if matches.len() != 1 {
                return Err(graph_error(format!(
                    "node {node_id} must match exactly one signed role (matched {})",
                    matches.len()
                )));
            }
            assignments.insert(*node_id, *matches.iter().next().expect("single candidate"));
        }
        let mut counts = BTreeMap::<&str, usize>::new();
        let mut distinct_parts = BTreeMap::<(&str, &str), BTreeSet<&str>>::new();
        for (node_id, node) in nodes {
            let inputs = node
                .get("inputs")
                .and_then(Value::as_object)
                .ok_or_else(|| graph_error(format!("node {node_id} requires inputs object")))?;
            let role_name = assignments[node_id.as_str()];
            let role = &self.roles[role_name];
            for (key, rule) in &role.inputs {
                if rule.value_type == ComfyWorkflowInputType::Part {
                    if rule.distinct {
                        if let Some(value) = inputs.get(key) {
                            let value = value.as_str().expect("validated part string");
                            if !distinct_parts
                                .entry((role_name, key.as_str()))
                                .or_default()
                                .insert(value)
                            {
                                return Err(graph_error(format!(
                                    "role {role_name} input {key} requires distinct signed parts"
                                )));
                            }
                        }
                    }
                    if let Some(part) = inputs
                        .get(key)
                        .and_then(Value::as_str)
                        .and_then(|name| parts.get(name))
                    {
                        required_parts.insert(part.part_id.clone(), part.clone());
                    }
                }
            }
            *counts.entry(role_name).or_default() += 1;
        }
        for (name, role) in &self.roles {
            let count = counts.get(name.as_str()).copied().unwrap_or(0);
            if count < role.min_count || count > role.max_count {
                return Err(graph_error(format!(
                    "role {name} count {count} violates signed bounds"
                )));
            }
        }
        let unique_role_nodes: BTreeMap<_, _> = assignments
            .iter()
            .map(|(node_id, role_name)| (*role_name, *node_id))
            .collect();
        for (node_id, role_name) in &assignments {
            let role = &self.roles[*role_name];
            let inputs = nodes[*node_id]["inputs"]
                .as_object()
                .expect("validated inputs");
            for (input_name, rule) in &role.inputs {
                let Some(source) = &rule.same_value_as else {
                    continue;
                };
                let source_id = unique_role_nodes
                    .get(source.role.as_str())
                    .expect("validated unique source role");
                let source_inputs = nodes[*source_id]["inputs"]
                    .as_object()
                    .expect("validated source inputs");
                if !value_equal(&inputs[input_name], &source_inputs[&source.input]) {
                    return Err(graph_error(format!(
                        "role {role_name} input {input_name} must equal signed source {}.{}",
                        source.role, source.input
                    )));
                }
            }
        }
        let mut dependencies = BTreeMap::<&str, BTreeSet<&str>>::new();
        for (node_id, node) in nodes {
            let role = &self.roles[assignments[node_id.as_str()]];
            let inputs = node["inputs"].as_object().expect("validated inputs");
            let edges = dependencies.entry(node_id).or_default();
            for (input_name, rule) in &role.inputs {
                if rule.value_type != ComfyWorkflowInputType::Link {
                    continue;
                }
                let Some(value) = inputs.get(input_name) else {
                    continue;
                };
                let (source_id, output) = link(value).expect("validated link");
                let source_role = assignments
                    .get(source_id)
                    .ok_or_else(|| graph_error("link source does not exist"))?;
                if !rule
                    .sources
                    .iter()
                    .any(|source| source.role == *source_role && source.output == output)
                {
                    return Err(graph_error(format!(
                        "node {node_id} input {input_name} has forbidden source role/slot"
                    )));
                }
                if let Some(other) = rule.same_source_as.as_ref() {
                    let (other_id, _) =
                        link(&inputs[other]).expect("validated required paired link");
                    let other_role = assignments
                        .get(other_id)
                        .ok_or_else(|| graph_error("paired link source does not exist"))?;
                    if rule
                        .paired_source_roles
                        .iter()
                        .any(|name| name == *source_role || name == *other_role)
                        && source_id != other_id
                    {
                        return Err(graph_error("paired links must use the same source node"));
                    }
                }
                edges.insert(source_id);
            }
        }
        // Iterative traversal bounds stack use even for adversarial max-size graphs.
        let output_id = assignments
            .iter()
            .find(|(_, role)| **role == self.output_role)
            .map(|(id, _)| *id)
            .expect("validated single output");
        let mut reachable = BTreeSet::new();
        let mut pending = vec![output_id];
        while let Some(id) = pending.pop() {
            if reachable.insert(id) {
                pending.extend(dependencies[id].iter().copied());
            }
        }
        let mut remaining = dependencies;
        while !remaining.is_empty() {
            let ready: BTreeSet<_> = remaining
                .iter()
                .filter(|(_, edges)| edges.is_empty())
                .map(|(id, _)| *id)
                .collect();
            if ready.is_empty() {
                return Err(graph_error("workflow links contain a cycle"));
            }
            remaining.retain(|id, _| !ready.contains(id));
            for edges in remaining.values_mut() {
                edges.retain(|id| !ready.contains(id));
            }
        }
        if reachable.len() != nodes.len() {
            return Err(graph_error("every node must contribute to the output"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{derive_comfy_workflow, ComfyWorkflowCatalogPolicy, USAGE_MEGAPIXEL_STEP};
    use serde_json::json;

    fn input(value_type: &str) -> Value {
        json!({"value_type": value_type, "required": true})
    }
    fn fixed(value: Value) -> Value {
        let mut rule = input(if value.is_number() {
            "number"
        } else {
            "string"
        });
        rule["fixed"] = value;
        rule
    }
    fn integer(min: u64, max: u64) -> Value {
        json!({"value_type":"integer","required":true,"minimum":min,"maximum":max,"multiple_of":1})
    }
    fn number(min: i64, max: i64) -> Value {
        json!({"value_type":"number","required":true,"minimum":min,"maximum":max})
    }
    fn source(role: &str, output: u64) -> Value {
        json!({"role":role,"output":output})
    }
    fn linked(sources: Vec<Value>) -> Value {
        json!({"value_type":"link","required":true,"sources":sources})
    }
    fn part_rule(names: &[&str], kind: &str) -> Value {
        json!({"value_type":"part","required":true,"part_type":kind,"part_names":names})
    }
    fn role(class: &str, min: usize, max: usize, inputs: Value) -> Value {
        json!({"class_type":class,"min_count":min,"max_count":max,"inputs":inputs})
    }
    fn policy() -> ComfyWorkflowCatalogPolicy {
        let model_sources = vec![
            source("model", 0),
            source("user_lora", 0),
            source("system_lora", 0),
        ];
        let clip_sources = vec![
            source("clip", 0),
            source("user_lora", 1),
            source("system_lora", 1),
        ];
        let mut paired_clip = linked(clip_sources.clone());
        paired_clip["same_source_as"] = json!("model");
        paired_clip["paired_source_roles"] = json!(["user_lora", "system_lora"]);
        let lora_inputs = |names: &[&str]| {
            let mut part = part_rule(names, "lora");
            part["distinct"] = json!(true);
            json!({
                "lora_name":part, "model":linked(model_sources.clone()),
                "clip":paired_clip, "strength_model":number(-2,2), "strength_clip":number(-2,2)
            })
        };
        let roles = json!({
            "model":role("UNETLoader",1,1,json!({"unet_name":part_rule(&["krea2_turbo_fp8_scaled.safetensors"],"checkpoint"),"weight_dtype":fixed(json!("default"))})),
            "clip":role("CLIPLoader",1,1,json!({"clip_name":part_rule(&["qwen3vl_4b_fp8_scaled.safetensors"],"text-encoder"),"type":fixed(json!("krea2")),"device":fixed(json!("default"))})),
            "vae":role("VAELoader",1,1,json!({"vae_name":part_rule(&["qwen_image_vae.safetensors"],"vae")})),
            "user_lora":role("LoraLoader",0,4,lora_inputs(&["style.safetensors","slider.safetensors","pose.safetensors","detail.safetensors","camera.safetensors"])),
            "system_lora":role("LoraLoader",0,1,lora_inputs(&["turbo.safetensors"])),
            "positive":role("CLIPTextEncode",1,1,json!({"clip":linked(clip_sources),"text":{"value_type":"string","required":true,"min_length":1,"max_length":8192}})),
            "negative":role("ConditioningZeroOut",1,1,json!({"conditioning":linked(vec![source("positive",0)])})),
            "latent":role("EmptyLatentImage",1,1,json!({"width":integer(16,2048),"height":integer(16,2048),"batch_size":integer(1,4)})),
            "sampler":role("KSampler",1,1,json!({
                "seed":integer(0,u64::MAX), "steps":integer(1,50), "cfg":number(1,10),
                "sampler_name":{"value_type":"string","required":true,"enum_values":["euler","heun"]},
                "scheduler":{"value_type":"string","required":true,"enum_values":["simple","normal"]},
                "denoise":fixed(json!(1)), "model":linked(model_sources),
                "positive":linked(vec![source("positive",0)]), "negative":linked(vec![source("negative",0)]),
                "latent_image":linked(vec![source("latent",0)])
            })),
            "decode":role("VAEDecode",1,1,json!({"samples":linked(vec![source("sampler",0)]),"vae":linked(vec![source("vae",0)])})),
            "output":role("SaveImage",1,1,json!({"images":linked(vec![source("decode",0)]),"filename_prefix":{"value_type":"string","required":true,"max_length":128}}))
        });
        let parts: Vec<_> = [("krea2_turbo_fp8_scaled.safetensors","checkpoint"), ("qwen3vl_4b_fp8_scaled.safetensors","text-encoder"), ("qwen_image_vae.safetensors","vae"), ("style.safetensors","lora"), ("slider.safetensors","lora"), ("pose.safetensors","lora"), ("detail.safetensors","lora"), ("camera.safetensors","lora"), ("turbo.safetensors","lora")]
            .into_iter().enumerate().map(|(i,(name,kind))| json!({"part_id":format!("{:064x}",i+1),"name":name,"type":kind,"sha256":format!("{:064x}",i+101)})).collect();
        serde_json::from_value(json!({
            "whitelisted_nodes":["UNETLoader","CLIPLoader","VAELoader","LoraLoader","CLIPTextEncode","ConditioningZeroOut","EmptyLatentImage","KSampler","VAEDecode","SaveImage"],
            "parts":parts,"pricing_unit":"megapixel_step","max_width":2048,"max_height":2048,"max_nodes":32,"max_steps":50,"max_artifacts":4,
            "dimension_bounds":{"min_width":16,"min_height":16,"max_pixels":1200000,"width_multiple":16,"height_multiple":16},
            "graph_constraints":{"output_role":"output","roles":roles}
        })).unwrap()
    }
    fn graph() -> Value {
        let canary: Value = serde_json::from_str(include_str!(
            "../../../catalog/canaries/canary-krea2-workflow-launch-v1.json"
        ))
        .unwrap();
        canary["prompts"][0]["workflow"].clone()
    }
    fn add_loras(graph: &mut Value, names: &[&str]) {
        let mut model = json!(["1", 0]);
        let mut clip = json!(["2", 0]);
        for (i, name) in names.iter().enumerate() {
            let id = format!("lora{i}");
            graph[&id] = json!({"class_type":"LoraLoader","inputs":{"lora_name":name,"model":model,"clip":clip,"strength_model":-2,"strength_clip":2}});
            model = json!([id, 0]);
            clip = json!([id, 1]);
        }
        graph["7"]["inputs"]["model"] = model;
        graph["4"]["inputs"]["clip"] = clip;
    }
    #[test]
    fn strict_workflow_lora_roles_and_batch_preserve_usage() {
        let policy = policy().derivation_policy().unwrap();
        let base = derive_comfy_workflow(&graph(), &policy).unwrap();
        assert_eq!(base.quoted_usage.get(USAGE_MEGAPIXEL_STEP), 16);
        let mut valid_graph = graph();
        // Four user adapters plus one system adapter are distinct signed roles.
        add_loras(
            &mut valid_graph,
            &[
                "style.safetensors",
                "slider.safetensors",
                "pose.safetensors",
                "detail.safetensors",
                "turbo.safetensors",
            ],
        );
        valid_graph["6"]["inputs"]["batch_size"] = json!(4);
        valid_graph["7"]["inputs"]["seed"] = json!(u64::MAX);
        let result = derive_comfy_workflow(&valid_graph, &policy).unwrap();
        assert_eq!(result.quoted_usage.get(USAGE_MEGAPIXEL_STEP), 64);
        assert_eq!(result.outcome_spec.artifact_count, 4);
        assert_eq!(result.parts_required.len(), 8);
        let mut too_many = graph();
        add_loras(
            &mut too_many,
            &[
                "style.safetensors",
                "slider.safetensors",
                "pose.safetensors",
                "detail.safetensors",
                "camera.safetensors",
            ],
        );
        assert!(derive_comfy_workflow(&too_many, &policy)
            .unwrap_err()
            .to_string()
            .contains("user_lora count 5"));

        let mut duplicate = graph();
        add_loras(&mut duplicate, &["style.safetensors", "style.safetensors"]);
        assert!(derive_comfy_workflow(&duplicate, &policy)
            .unwrap_err()
            .to_string()
            .contains("distinct signed parts"));
    }
    #[test]
    fn strict_workflow_rejects_invalid_inputs_before_derivation() {
        let policy = policy().derivation_policy().unwrap();
        for (node, key, value) in [
            ("7", "steps", json!(0)),
            ("7", "steps", json!(51)),
            ("7", "steps", json!(8.5)),
            ("7", "steps", json!("8")),
            ("7", "cfg", json!(10.1)),
            ("7", "cfg", json!(-1)),
            ("7", "sampler_name", json!("unknown")),
            ("7", "scheduler", json!("unknown")),
            ("7", "denoise", json!(0.5)),
            ("7", "seed", json!(-1)),
            ("7", "seed", json!(1.5)),
            ("6", "batch_size", json!(0)),
            ("6", "batch_size", json!(5)),
            ("4", "text", json!("")),
            ("1", "unet_name", json!("unknown.safetensors")),
            ("7", "model", json!(["1", 9])),
            ("7", "model", json!(["missing", 0])),
            ("7", "model", json!(["1", 0, "extra"])),
        ] {
            let mut graph = graph();
            graph[node]["inputs"][key] = value;
            assert!(
                derive_comfy_workflow(&graph, &policy).is_err(),
                "{node}.{key}"
            );
        }
        let mut graph = graph();
        graph["7"]["inputs"]
            .as_object_mut()
            .unwrap()
            .remove("steps");
        assert!(derive_comfy_workflow(&graph, &policy).is_err());
        graph["7"]["inputs"]["steps"] = json!(8);
        graph["7"]["inputs"]["extra"] = json!(true);
        assert!(derive_comfy_workflow(&graph, &policy).is_err());
    }
    #[test]
    fn strict_workflow_rejects_split_cycles_branches_and_unsigned_adapters() {
        let policy = policy().derivation_policy().unwrap();
        let mut chain = graph();
        add_loras(&mut chain, &["style.safetensors", "slider.safetensors"]);
        for (id, key, value) in [
            ("lora0", "lora_name", json!("unsigned.safetensors")),
            ("lora0", "strength_model", json!(-2.1)),
            ("lora0", "strength_clip", json!(2.1)),
            ("lora1", "clip", json!(["2", 0])),
        ] {
            let mut graph = chain.clone();
            graph[id]["inputs"][key] = value;
            assert!(derive_comfy_workflow(&graph, &policy).is_err());
        }
        let mut cycle = chain.clone();
        cycle["lora0"]["inputs"]["model"] = json!(["lora1", 0]);
        cycle["lora0"]["inputs"]["clip"] = json!(["lora1", 1]);
        assert!(derive_comfy_workflow(&cycle, &policy)
            .unwrap_err()
            .to_string()
            .contains("cycle"));
        let mut disconnected = chain.clone();
        disconnected["orphan"] = chain["lora0"].clone();
        disconnected["orphan"]["inputs"]["lora_name"] = json!("pose.safetensors");
        assert!(derive_comfy_workflow(&disconnected, &policy)
            .unwrap_err()
            .to_string()
            .contains("contribute"));
        for id in ["7", "9"] {
            let mut graph = chain.clone();
            graph["duplicate"] = graph[id].clone();
            assert!(derive_comfy_workflow(&graph, &policy).is_err());
        }
    }
    #[test]
    fn strict_workflow_dimensions_check_area_multiples_and_overflow() {
        let policy = policy().derivation_policy().unwrap();
        let mut graph = graph();
        graph["6"]["inputs"]["width"] = json!(768);
        graph["6"]["inputs"]["height"] = json!(1280);
        assert_eq!(
            derive_comfy_workflow(&graph, &policy)
                .unwrap()
                .quoted_usage
                .get(USAGE_MEGAPIXEL_STEP),
            8
        );
        for (width, height) in [(2048, 2048), (769, 1280), (0, 1280), (16, 15)] {
            graph["6"]["inputs"]["width"] = json!(width);
            graph["6"]["inputs"]["height"] = json!(height);
            assert!(derive_comfy_workflow(&graph, &policy).is_err());
        }
        let bounds = ComfyWorkflowDimensionBounds {
            min_width: 1,
            min_height: 1,
            max_pixels: u64::MAX,
            width_multiple: 1,
            height_multiple: 1,
        };
        assert!(bounds.check(Some(u64::MAX), Some(2)).is_err());
        assert!(bounds.check(None, Some(2)).is_err());
    }
    #[test]
    fn strict_workflow_media_only_names_permitted_signed_parts() {
        let policy = policy();
        let media: crate::ComfyWorkflowMedia = serde_json::from_value(json!({
            "schema_version":1,"kind":"image","family":"krea2-turbo",
            "loras":[{"id":policy.parts[3].part_id,"name":"Style","default_strength":1}],
            "presets":[{
                "id":"fast","name":"Fast",
                "inputs":[
                    {"role":"sampler","input":"steps","value":8},
                    {"role":"sampler","input":"cfg","value":1.0}
                ]
            }],
            "default_preset":"fast"
        }))
        .unwrap();
        media.validate(&policy).unwrap();
        let mut unknown = media.clone();
        unknown.loras[0].id = "ff".repeat(32);
        assert!(unknown.validate(&policy).is_err());
        let mut duplicate = media.clone();
        duplicate.loras.push(duplicate.loras[0].clone());
        assert!(duplicate.validate(&policy).is_err());
        let mut new_schema = media.clone();
        new_schema.schema_version = 2;
        assert!(new_schema.validate(&policy).is_err());
        let mut without_permission = policy.clone();
        without_permission
            .graph_constraints
            .as_mut()
            .unwrap()
            .roles
            .remove("user_lora");
        assert!(media.validate(&without_permission).is_err());
        let mut bad_default = media.clone();
        bad_default.default_preset = Some("missing".into());
        assert!(bad_default.validate(&policy).is_err());
        let mut duplicate_assignment = media.clone();
        let repeated = duplicate_assignment.presets[0].inputs[0].clone();
        duplicate_assignment.presets[0].inputs.push(repeated);
        assert!(duplicate_assignment.validate(&policy).is_err());
        let mut out_of_range = media.clone();
        out_of_range.presets[0].inputs[0].value =
            crate::ComfyWorkflowPresetValue::Number(51.into());
        assert!(out_of_range.validate(&policy).is_err());
        let mut link = media.clone();
        link.presets[0].inputs[0].input = "model".into();
        assert!(link.validate(&policy).is_err());
    }

    #[test]
    fn strict_workflow_can_couple_scalar_inputs_between_unique_roles() {
        let mut policy = policy();
        policy
            .graph_constraints
            .as_mut()
            .unwrap()
            .roles
            .get_mut("sampler")
            .unwrap()
            .inputs
            .get_mut("seed")
            .unwrap()
            .same_value_as = Some(ComfyWorkflowValueSource {
            role: "latent".into(),
            input: "width".into(),
        });
        let derivation = policy.derivation_policy().unwrap();
        assert!(derive_comfy_workflow(&graph(), &derivation)
            .unwrap_err()
            .to_string()
            .contains("must equal signed source"));
        let mut equal = graph();
        equal["7"]["inputs"]["seed"] = json!(1024);
        derive_comfy_workflow(&equal, &derivation).unwrap();

        let mut invalid = policy;
        invalid
            .graph_constraints
            .as_mut()
            .unwrap()
            .roles
            .get_mut("sampler")
            .unwrap()
            .inputs
            .get_mut("seed")
            .unwrap()
            .same_value_as
            .as_mut()
            .unwrap()
            .role = "missing".into();
        assert!(invalid.derivation_policy().is_err());
    }

    #[test]
    fn strict_workflow_disambiguates_identical_encoders_from_consumer_links() {
        let mut policy = policy();
        let roles = &mut policy.graph_constraints.as_mut().unwrap().roles;
        let negative = roles["positive"].clone();
        roles.insert("negative".into(), negative);
        roles
            .get_mut("sampler")
            .unwrap()
            .inputs
            .get_mut("negative")
            .unwrap()
            .sources = vec![ComfyWorkflowLinkSource {
            role: "negative".into(),
            output: 0,
        }];

        let mut graph = graph();
        graph["5"] = json!({
            "class_type":"CLIPTextEncode",
            "inputs":{"clip":["2",0],"text":"avoid visible artifacts"}
        });
        graph["7"]["inputs"]["negative"] = json!(["5", 0]);
        derive_comfy_workflow(&graph, &policy.derivation_policy().unwrap()).unwrap();
    }

    #[test]
    fn strict_workflow_allows_an_omitted_optional_distinct_part() {
        let mut policy = policy();
        policy
            .graph_constraints
            .as_mut()
            .unwrap()
            .roles
            .get_mut("user_lora")
            .unwrap()
            .inputs
            .get_mut("lora_name")
            .unwrap()
            .required = false;
        let mut graph = graph();
        add_loras(&mut graph, &["style.safetensors"]);
        graph["lora0"]["inputs"]
            .as_object_mut()
            .unwrap()
            .remove("lora_name");
        derive_comfy_workflow(&graph, &policy.derivation_policy().unwrap()).unwrap();
    }

    #[test]
    fn strict_workflow_rejects_malformed_policy_and_ambiguous_roles() {
        for path in [
            "bad-bound",
            "bad-role",
            "bad-part",
            "bad-fixed",
            "bad-source",
            "bad-type",
        ] {
            let mut policy = policy();
            let roles = &mut policy.graph_constraints.as_mut().unwrap().roles;
            match path {
                "bad-bound" => policy.dimension_bounds.as_mut().unwrap().width_multiple = 0,
                "bad-role" => roles.get_mut("output").unwrap().max_count = 2,
                "bad-part" => {
                    roles
                        .get_mut("user_lora")
                        .unwrap()
                        .inputs
                        .get_mut("lora_name")
                        .unwrap()
                        .part_names = vec!["unsigned".into()]
                }
                "bad-fixed" => {
                    roles
                        .get_mut("sampler")
                        .unwrap()
                        .inputs
                        .get_mut("steps")
                        .unwrap()
                        .fixed = Some(json!(51))
                }
                "bad-source" => {
                    roles
                        .get_mut("output")
                        .unwrap()
                        .inputs
                        .get_mut("images")
                        .unwrap()
                        .sources[0]
                        .role = "missing".into()
                }
                _ => {
                    roles
                        .get_mut("sampler")
                        .unwrap()
                        .inputs
                        .get_mut("steps")
                        .unwrap()
                        .max_length = Some(20)
                }
            }
            assert!(policy.derivation_policy().is_err(), "{path}");
        }
        let mut policy = policy();
        let roles = &mut policy.graph_constraints.as_mut().unwrap().roles;
        let mut duplicate = roles["model"].clone();
        duplicate.min_count = 0;
        roles.insert("ambiguous".into(), duplicate);
        roles
            .get_mut("sampler")
            .unwrap()
            .inputs
            .get_mut("model")
            .unwrap()
            .sources
            .push(ComfyWorkflowLinkSource {
                role: "ambiguous".into(),
                output: 0,
            });
        assert!(
            derive_comfy_workflow(&graph(), &policy.derivation_policy().unwrap())
                .unwrap_err()
                .to_string()
                .contains("matched 2")
        );
        let mut value = serde_json::to_value(policy).unwrap();
        value["graph_constraints"]["unknown"] = json!(true);
        assert!(serde_json::from_value::<ComfyWorkflowCatalogPolicy>(value).is_err());
    }
}
