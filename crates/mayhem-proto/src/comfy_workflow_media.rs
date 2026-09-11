//! Signed presentation metadata. All execution permissions remain in workflow policy.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::{ComfyWorkflowCatalogPolicy, ComfyWorkflowDerivationError, ComfyWorkflowInputType};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyWorkflowMedia {
    pub schema_version: u32,
    pub kind: String,
    pub family: String,
    pub loras: Vec<ComfyWorkflowLoraDisplay>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<ComfyWorkflowPreset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_preset: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyWorkflowLoraDisplay {
    /// Exact signed part_id, not a separate website identifier.
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_strength: Option<Number>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyWorkflowPreset {
    pub id: String,
    pub name: String,
    pub inputs: Vec<ComfyWorkflowPresetInput>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyWorkflowPresetInput {
    pub role: String,
    pub input: String,
    pub value: ComfyWorkflowPresetValue,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ComfyWorkflowPresetValue {
    String(String),
    Number(Number),
    Boolean(bool),
}

impl From<&ComfyWorkflowPresetValue> for Value {
    fn from(value: &ComfyWorkflowPresetValue) -> Self {
        match value {
            ComfyWorkflowPresetValue::String(value) => Value::String(value.clone()),
            ComfyWorkflowPresetValue::Number(value) => Value::Number(value.clone()),
            ComfyWorkflowPresetValue::Boolean(value) => Value::Bool(*value),
        }
    }
}

impl ComfyWorkflowMedia {
    pub fn validate(
        &self,
        policy: &ComfyWorkflowCatalogPolicy,
    ) -> Result<(), ComfyWorkflowDerivationError> {
        let invalid = |message: &str| {
            ComfyWorkflowDerivationError::InvalidPolicy(format!("workflow_media {message}"))
        };
        if self.schema_version != 1 || self.kind != "image" || self.family.trim().is_empty() {
            return Err(invalid("requires schema 1, image kind and family"));
        }
        let constraints = policy
            .graph_constraints
            .as_ref()
            .ok_or_else(|| invalid("requires signed graph constraints"))?;
        let mut ids = BTreeSet::new();
        for lora in &self.loras {
            if lora.name.trim().is_empty() || !ids.insert(&lora.id) {
                return Err(invalid("has empty names or duplicate part ids"));
            }
            let part = policy
                .parts
                .iter()
                .find(|part| part.part_id == lora.id && part.part_type == "lora")
                .ok_or_else(|| invalid("LoRA does not reference a signed LoRA part"))?;
            if !constraints.roles.values().any(|role| {
                role.inputs.values().any(|input| {
                    input.value_type == ComfyWorkflowInputType::Part
                        && input.part_type.as_deref() == Some("lora")
                        && input.part_names.contains(&part.name)
                })
            }) {
                return Err(invalid("LoRA is not permitted by signed graph constraints"));
            }
        }
        let parts = policy
            .parts
            .iter()
            .map(|part| (part.name.clone(), part.clone()))
            .collect();
        let mut preset_ids = BTreeSet::new();
        for preset in &self.presets {
            if preset.id.trim().is_empty()
                || preset.name.trim().is_empty()
                || preset.inputs.is_empty()
                || !preset_ids.insert(preset.id.as_str())
            {
                return Err(invalid(
                    "presets require unique non-empty ids, names and inputs",
                ));
            }
            let mut assignments = BTreeSet::new();
            for assignment in &preset.inputs {
                if assignment.role.trim().is_empty()
                    || assignment.input.trim().is_empty()
                    || !assignments.insert((assignment.role.as_str(), assignment.input.as_str()))
                {
                    return Err(invalid(
                        "preset assignments require unique non-empty role/input pairs",
                    ));
                }
                let constraint = constraints
                    .roles
                    .get(&assignment.role)
                    .and_then(|role| role.inputs.get(&assignment.input))
                    .ok_or_else(|| invalid("preset assignment is not a signed role input"))?;
                if constraint.value_type == ComfyWorkflowInputType::Link
                    || !constraint.check(&Value::from(&assignment.value), &parts)
                {
                    return Err(invalid(
                        "preset assignment violates its signed input constraint",
                    ));
                }
            }
        }
        if self
            .default_preset
            .as_ref()
            .is_some_and(|id| id.trim().is_empty() || !preset_ids.contains(id.as_str()))
        {
            return Err(invalid("default_preset must reference a declared preset"));
        }
        Ok(())
    }
}
