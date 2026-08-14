use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::{
    stable_json_bytes, ReceiptUsage, WorkflowOutputBinding, USAGE_AUDIO_SECOND,
    USAGE_COMPUTE_SECOND, USAGE_FRAME, USAGE_IMAGE, USAGE_INPUT_CHARACTER, USAGE_MEGAPIXEL,
    USAGE_MEGAPIXEL_STEP, USAGE_STEP, USAGE_VIDEO_SECOND,
};

pub const COMFY_WORKFLOW_DERIVATION_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_COMFY_WORKFLOW_RUNTIME_ID: &str = "comfyui-v0.30.1";

const GRAPH_HASH_DOMAIN: &[u8] = b"mayhem:comfy-workflow-graph:v1";
const MAX_COMFY_WORKFLOW_PART_NAME_BYTES: usize = 512;
const DEFAULT_COMFY_WORKFLOW_MAX_NODES: usize = 4_096;
const DEFAULT_COMFY_WORKFLOW_MAX_WIDTH: u64 = 8_192;
const DEFAULT_COMFY_WORKFLOW_MAX_HEIGHT: u64 = 8_192;
const DEFAULT_COMFY_WORKFLOW_MAX_FRAMES: u64 = 4_096;
const DEFAULT_COMFY_WORKFLOW_MAX_DURATION_SECONDS: u64 = 600;
const DEFAULT_COMFY_WORKFLOW_MAX_STEPS: u64 = 1_000;
const DEFAULT_COMFY_WORKFLOW_MAX_ARTIFACTS: u64 = 64;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ComfyWorkflowPartRef {
    pub part_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub part_type: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyWorkflowCatalogPolicy {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub whitelisted_nodes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<ComfyWorkflowPartRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_class_definition: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_nodes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_frames: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_steps: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_artifacts: Option<u64>,
}

pub fn comfy_outcome_class_definition_hash(
    definition: &Value,
) -> Result<String, serde_json::Error> {
    let envelope = serde_json::json!({
        "domain": "mayhem-comfy-outcome-class-definition-v1",
        "value": definition,
    });
    Ok(blake3::hash(&stable_json_bytes(&envelope)?)
        .to_hex()
        .to_string())
}

impl ComfyWorkflowCatalogPolicy {
    pub fn derivation_policy(
        &self,
    ) -> Result<ComfyWorkflowDerivationPolicy, ComfyWorkflowDerivationError> {
        if self.whitelisted_nodes.is_empty() {
            return Err(ComfyWorkflowDerivationError::InvalidPolicy(
                "node whitelist is empty".to_owned(),
            ));
        }
        let mut whitelisted_nodes = BTreeSet::new();
        for node in &self.whitelisted_nodes {
            let node = node.trim();
            if node.is_empty() {
                return Err(ComfyWorkflowDerivationError::InvalidPolicy(
                    "node whitelist contains an empty class".to_owned(),
                ));
            }
            if !whitelisted_nodes.insert(node.to_owned()) {
                return Err(ComfyWorkflowDerivationError::InvalidPolicy(format!(
                    "duplicate whitelisted node {node}"
                )));
            }
        }
        let mut parts_by_name = BTreeMap::new();
        for part in &self.parts {
            if part.name.trim().is_empty() {
                return Err(ComfyWorkflowDerivationError::InvalidPolicy(
                    "part name is empty".to_owned(),
                ));
            }
            if parts_by_name
                .insert(part.name.clone(), part.clone())
                .is_some()
            {
                return Err(ComfyWorkflowDerivationError::InvalidPolicy(format!(
                    "duplicate part name {}",
                    part.name
                )));
            }
            if let Some(scale) = part.scale {
                if part.part_type != "upscaler" {
                    return Err(ComfyWorkflowDerivationError::InvalidPolicy(format!(
                        "part {} declares scale but is not an upscaler",
                        part.name
                    )));
                }
                if scale == 0 || scale > 64 {
                    return Err(ComfyWorkflowDerivationError::InvalidPolicy(format!(
                        "part {} declares unsupported upscaler scale {scale}",
                        part.name
                    )));
                }
            }
        }
        if let Some(unit) = self.pricing_unit.as_deref() {
            if !valid_comfy_pricing_unit(unit) {
                return Err(ComfyWorkflowDerivationError::InvalidPolicy(format!(
                    "unsupported pricing_unit {unit}"
                )));
            }
        }
        let defaults = ComfyWorkflowDerivationPolicy::default();
        let max_steps = self.max_steps.unwrap_or(defaults.max_steps).max(1);
        let mut allowed_steps = BTreeSet::new();
        for step in &self.allowed_steps {
            if *step == 0 {
                return Err(ComfyWorkflowDerivationError::InvalidPolicy(
                    "allowed_steps contains zero".to_owned(),
                ));
            }
            if *step > max_steps {
                return Err(ComfyWorkflowDerivationError::InvalidPolicy(format!(
                    "allowed_steps value {step} exceeds max_steps {max_steps}"
                )));
            }
            if !allowed_steps.insert(*step) {
                return Err(ComfyWorkflowDerivationError::InvalidPolicy(format!(
                    "duplicate allowed_steps value {step}"
                )));
            }
        }
        Ok(ComfyWorkflowDerivationPolicy {
            whitelisted_nodes,
            parts_by_name,
            pricing_unit: self.pricing_unit.clone(),
            max_nodes: self.max_nodes.unwrap_or(defaults.max_nodes).max(1),
            max_width: self.max_width.unwrap_or(defaults.max_width).max(1),
            max_height: self.max_height.unwrap_or(defaults.max_height).max(1),
            max_frames: self.max_frames.unwrap_or(defaults.max_frames).max(1),
            max_duration_seconds: self
                .max_duration_seconds
                .unwrap_or(defaults.max_duration_seconds)
                .max(1),
            max_steps,
            allowed_steps,
            max_artifacts: self.max_artifacts.unwrap_or(defaults.max_artifacts).max(1),
        })
    }

    pub fn runtime_id(&self) -> &str {
        self.runtime_id
            .as_deref()
            .unwrap_or(DEFAULT_COMFY_WORKFLOW_RUNTIME_ID)
    }

    pub fn outcome_class_for(&self, output_modality: &str) -> String {
        self.outcome_class
            .clone()
            .unwrap_or_else(|| format!("{output_modality}.workflow"))
    }
}

pub fn valid_comfy_pricing_unit(unit: &str) -> bool {
    matches!(
        unit,
        USAGE_MEGAPIXEL_STEP
            | USAGE_MEGAPIXEL
            | USAGE_COMPUTE_SECOND
            | USAGE_AUDIO_SECOND
            | USAGE_INPUT_CHARACTER
            | USAGE_FRAME
            | USAGE_IMAGE
            | USAGE_STEP
            | USAGE_VIDEO_SECOND
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComfyWorkflowDerivationPolicy {
    pub whitelisted_nodes: BTreeSet<String>,
    pub parts_by_name: BTreeMap<String, ComfyWorkflowPartRef>,
    pub pricing_unit: Option<String>,
    pub max_nodes: usize,
    pub max_width: u64,
    pub max_height: u64,
    pub max_frames: u64,
    pub max_duration_seconds: u64,
    pub max_steps: u64,
    pub allowed_steps: BTreeSet<u64>,
    pub max_artifacts: u64,
}

impl Default for ComfyWorkflowDerivationPolicy {
    fn default() -> Self {
        Self {
            whitelisted_nodes: BTreeSet::new(),
            parts_by_name: BTreeMap::new(),
            pricing_unit: None,
            max_nodes: DEFAULT_COMFY_WORKFLOW_MAX_NODES,
            max_width: DEFAULT_COMFY_WORKFLOW_MAX_WIDTH,
            max_height: DEFAULT_COMFY_WORKFLOW_MAX_HEIGHT,
            max_frames: DEFAULT_COMFY_WORKFLOW_MAX_FRAMES,
            max_duration_seconds: DEFAULT_COMFY_WORKFLOW_MAX_DURATION_SECONDS,
            max_steps: DEFAULT_COMFY_WORKFLOW_MAX_STEPS,
            allowed_steps: BTreeSet::new(),
            max_artifacts: DEFAULT_COMFY_WORKFLOW_MAX_ARTIFACTS,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ComfyWorkflowOutcomeSpec {
    pub output_modalities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u64>,
    pub artifact_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ComfyWorkflowDerivation {
    pub schema_version: u32,
    pub graph_hash: String,
    pub parts_required: Vec<ComfyWorkflowPartRef>,
    pub node_set: Vec<String>,
    pub outcome_spec: ComfyWorkflowOutcomeSpec,
    pub quoted_usage: ReceiptUsage,
    pub workflow_output: WorkflowOutputBinding,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComfyWorkflowDerivationError {
    InvalidPolicy(String),
    InvalidGraph(String),
    NonWhitelistedNode(String),
    MissingPart(String),
    OutcomeOverflow(String),
    Hash(String),
}

impl fmt::Display for ComfyWorkflowDerivationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPolicy(reason) => write!(f, "invalid Comfy workflow policy: {reason}"),
            Self::InvalidGraph(reason) => write!(f, "invalid Comfy workflow graph: {reason}"),
            Self::NonWhitelistedNode(node) => {
                write!(f, "Comfy workflow node class {node} is not whitelisted")
            }
            Self::MissingPart(part) => write!(f, "Comfy workflow requires unknown part {part}"),
            Self::OutcomeOverflow(reason) => {
                write!(f, "Comfy workflow outcome exceeds caps: {reason}")
            }
            Self::Hash(reason) => write!(f, "Comfy workflow graph hash failed: {reason}"),
        }
    }
}

impl std::error::Error for ComfyWorkflowDerivationError {}

pub fn derive_comfy_workflow(
    graph: &Value,
    policy: &ComfyWorkflowDerivationPolicy,
) -> Result<ComfyWorkflowDerivation, ComfyWorkflowDerivationError> {
    let nodes = graph.as_object().ok_or_else(|| {
        ComfyWorkflowDerivationError::InvalidGraph("graph must be a JSON object".to_owned())
    })?;
    if nodes.is_empty() {
        return Err(ComfyWorkflowDerivationError::InvalidGraph(
            "graph must contain at least one node".to_owned(),
        ));
    }
    if nodes.len() > policy.max_nodes {
        return Err(ComfyWorkflowDerivationError::OutcomeOverflow(format!(
            "node count {} exceeds maximum {}",
            nodes.len(),
            policy.max_nodes
        )));
    }
    if policy.whitelisted_nodes.is_empty() {
        return Err(ComfyWorkflowDerivationError::InvalidGraph(
            "node whitelist is empty".to_owned(),
        ));
    }

    let mut node_set = BTreeSet::new();
    let mut parts_required = BTreeMap::<String, ComfyWorkflowPartRef>::new();
    let mut metrics = OutcomeMetrics::default();

    for (node_id, node) in nodes {
        let node_object = node.as_object().ok_or_else(|| {
            ComfyWorkflowDerivationError::InvalidGraph(format!("node {node_id} must be an object"))
        })?;
        let class_type = node_object
            .get("class_type")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ComfyWorkflowDerivationError::InvalidGraph(format!(
                    "node {node_id} is missing class_type"
                ))
            })?;
        if !policy.whitelisted_nodes.contains(class_type) {
            return Err(ComfyWorkflowDerivationError::NonWhitelistedNode(
                class_type.to_owned(),
            ));
        }
        node_set.insert(class_type.to_owned());
        classify_output_node(class_type, &mut metrics);
        scan_workflow_value(
            node,
            policy,
            &mut parts_required,
            &mut metrics,
            Some(class_type),
        )?;
    }
    infer_linked_image_dimensions(nodes, policy, &mut metrics)?;

    let outcome_spec = metrics.into_outcome_spec(policy)?;
    let quoted_usage = workflow_usage_from_outcome(&outcome_spec, policy.pricing_unit.as_deref());
    let workflow_output = workflow_output_from_outcome(&outcome_spec);
    let graph_hash = graph_hash(graph)?;
    Ok(ComfyWorkflowDerivation {
        schema_version: COMFY_WORKFLOW_DERIVATION_SCHEMA_VERSION,
        graph_hash,
        parts_required: parts_required.into_values().collect(),
        node_set: node_set.into_iter().collect(),
        outcome_spec,
        quoted_usage,
        workflow_output,
    })
}

pub fn validate_comfy_workflow_media_input_file_bindings(
    request: &Value,
) -> Result<(), ComfyWorkflowDerivationError> {
    let Some(graph) = request.get("workflow") else {
        return Ok(());
    };
    let nodes = graph.as_object().ok_or_else(|| {
        ComfyWorkflowDerivationError::InvalidGraph("graph must be a JSON object".to_owned())
    })?;
    let mut input_files = BTreeSet::new();
    if let Some(files) = request.get("input_files") {
        let files = files.as_array().ok_or_else(|| {
            ComfyWorkflowDerivationError::InvalidGraph(
                "workflow input_files must be an array".to_owned(),
            )
        })?;
        for (index, file) in files.iter().enumerate() {
            let filename = file
                .as_object()
                .and_then(|object| object.get("filename"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ComfyWorkflowDerivationError::InvalidGraph(format!(
                        "workflow input_files[{index}].filename is required"
                    ))
                })?;
            if !comfy_workflow_input_filename_is_safe(filename) {
                return Err(ComfyWorkflowDerivationError::InvalidGraph(format!(
                    "workflow input_files[{index}].filename is not a safe relative path"
                )));
            }
            if !input_files.insert(filename.to_owned()) {
                return Err(ComfyWorkflowDerivationError::InvalidGraph(format!(
                    "duplicate workflow input_files filename {filename}"
                )));
            }
        }
    }
    for (node_id, node) in nodes {
        let Some(node_object) = node.as_object() else {
            continue;
        };
        let Some(class_type) = node_object.get("class_type").and_then(Value::as_str) else {
            continue;
        };
        if !comfy_node_can_load_request_media(class_type) {
            continue;
        }
        let Some(inputs) = node_object.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        for key in [
            "image",
            "audio",
            "video",
            "video_path",
            "file",
            "filename",
            "path",
        ] {
            let Some(filename) = inputs.get(key).and_then(Value::as_str) else {
                continue;
            };
            if !comfy_workflow_input_filename_is_safe(filename) {
                return Err(ComfyWorkflowDerivationError::InvalidGraph(format!(
                    "node {node_id} {class_type}.{key} references unsafe media input filename {filename}"
                )));
            }
            if !input_files.contains(filename) {
                return Err(ComfyWorkflowDerivationError::InvalidGraph(format!(
                    "node {node_id} {class_type}.{key} references media input {filename} but it is not supplied in input_files"
                )));
            }
        }
    }
    Ok(())
}

fn graph_hash(graph: &Value) -> Result<String, ComfyWorkflowDerivationError> {
    let graph = comfy_graph_hash_value(graph);
    let graph_bytes = stable_json_bytes(&graph)
        .map_err(|err| ComfyWorkflowDerivationError::Hash(err.to_string()))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(GRAPH_HASH_DOMAIN);
    hasher.update(&(graph_bytes.len() as u64).to_le_bytes());
    hasher.update(&graph_bytes);
    Ok(hasher.finalize().to_hex().to_string())
}

fn comfy_node_can_load_request_media(class_type: &str) -> bool {
    let class_type = class_type.to_ascii_lowercase();
    class_type.contains("load")
        && (class_type.contains("image")
            || class_type.contains("audio")
            || class_type.contains("video"))
}

fn comfy_workflow_input_filename_is_safe(filename: &str) -> bool {
    if filename.is_empty()
        || filename.len() > 240
        || filename.starts_with('/')
        || filename.starts_with('\\')
        || filename.contains('\\')
    {
        return false;
    }
    filename.split('/').all(|part| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    })
}

fn comfy_graph_hash_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(comfy_graph_hash_value).collect()),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), comfy_graph_hash_value(value)))
                .collect(),
        ),
        Value::Number(number) => comfy_graph_hash_number(number)
            .map(Value::Number)
            .unwrap_or_else(|| value.clone()),
        _ => value.clone(),
    }
}

fn comfy_graph_hash_number(number: &Number) -> Option<Number> {
    if number.as_i64().is_some() || number.as_u64().is_some() {
        return None;
    }
    let value = number.as_f64()?;
    if !value.is_finite() || value.fract() != 0.0 {
        return None;
    }
    if value >= 0.0 && value <= u64::MAX as f64 {
        return Some(Number::from(value as u64));
    }
    if value >= i64::MIN as f64 && value <= i64::MAX as f64 {
        return Some(Number::from(value as i64));
    }
    None
}

#[derive(Clone, Copy, Debug, Default)]
struct OutcomeMetrics {
    has_image: bool,
    has_video: bool,
    has_audio: bool,
    width: Option<u64>,
    height: Option<u64>,
    frames: Option<u64>,
    fps: Option<u64>,
    duration_seconds: Option<u64>,
    steps: Option<u64>,
    artifact_count: Option<u64>,
}

impl OutcomeMetrics {
    fn into_outcome_spec(
        self,
        policy: &ComfyWorkflowDerivationPolicy,
    ) -> Result<ComfyWorkflowOutcomeSpec, ComfyWorkflowDerivationError> {
        let mut output_modalities = Vec::new();
        if self.has_video {
            output_modalities.push("video".to_owned());
        }
        if self.has_audio {
            output_modalities.push("audio".to_owned());
        }
        if self.has_image || output_modalities.is_empty() {
            output_modalities.push("image".to_owned());
        }
        output_modalities.sort();
        output_modalities.dedup();

        let width = self.width;
        let height = self.height;
        let frames = self.frames.or_else(|| {
            self.duration_seconds
                .zip(self.fps)
                .map(|(seconds, fps)| seconds.saturating_mul(fps.max(1)))
        });
        let duration_seconds = self.duration_seconds.or_else(|| {
            frames.zip(self.fps).map(|(frames, fps)| {
                let fps = fps.max(1);
                frames.saturating_add(fps - 1) / fps
            })
        });
        let steps = self.steps;
        let artifact_count = self.artifact_count.unwrap_or(1).max(1);

        if width.is_some_and(|value| value > policy.max_width) {
            return Err(ComfyWorkflowDerivationError::OutcomeOverflow(format!(
                "width exceeds {}",
                policy.max_width
            )));
        }
        if height.is_some_and(|value| value > policy.max_height) {
            return Err(ComfyWorkflowDerivationError::OutcomeOverflow(format!(
                "height exceeds {}",
                policy.max_height
            )));
        }
        if frames.is_some_and(|value| value > policy.max_frames) {
            return Err(ComfyWorkflowDerivationError::OutcomeOverflow(format!(
                "frames exceeds {}",
                policy.max_frames
            )));
        }
        if duration_seconds.is_some_and(|value| value > policy.max_duration_seconds) {
            return Err(ComfyWorkflowDerivationError::OutcomeOverflow(format!(
                "duration_seconds exceeds {}",
                policy.max_duration_seconds
            )));
        }
        if steps.is_some_and(|value| value > policy.max_steps) {
            return Err(ComfyWorkflowDerivationError::OutcomeOverflow(format!(
                "steps exceeds {}",
                policy.max_steps
            )));
        }
        if let Some(value) = steps {
            if !policy.allowed_steps.is_empty() && !policy.allowed_steps.contains(&value) {
                let allowed = policy
                    .allowed_steps
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(ComfyWorkflowDerivationError::OutcomeOverflow(format!(
                    "steps must be one of {allowed}"
                )));
            }
        }
        if artifact_count > policy.max_artifacts {
            return Err(ComfyWorkflowDerivationError::OutcomeOverflow(format!(
                "artifact_count exceeds {}",
                policy.max_artifacts
            )));
        }

        Ok(ComfyWorkflowOutcomeSpec {
            output_modalities,
            width,
            height,
            frames,
            duration_seconds,
            steps,
            artifact_count,
        })
    }
}

fn classify_output_node(class_type: &str, metrics: &mut OutcomeMetrics) {
    let class = class_type.to_ascii_lowercase();
    if class.contains("video") || class.contains("vhs") {
        metrics.has_video = true;
    }
    if class.contains("audio") || class.contains("sound") {
        metrics.has_audio = true;
    }
    if class.contains("saveimage")
        || class.contains("previewimage")
        || class.contains("imageoutput")
        || (class.contains("image") && (class.contains("save") || class.contains("preview")))
    {
        metrics.has_image = true;
    }
}

fn scan_workflow_value(
    value: &Value,
    policy: &ComfyWorkflowDerivationPolicy,
    parts_required: &mut BTreeMap<String, ComfyWorkflowPartRef>,
    metrics: &mut OutcomeMetrics,
    parent_class: Option<&str>,
) -> Result<(), ComfyWorkflowDerivationError> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if let Some(part_name) = comfy_part_name_for_key(key, value)? {
                    let part = policy
                        .parts_by_name
                        .get(part_name)
                        .cloned()
                        .ok_or_else(|| {
                            ComfyWorkflowDerivationError::MissingPart(part_name.to_owned())
                        })?;
                    parts_required.insert(part.part_id.clone(), part);
                }
                update_metric_from_key(key, value, metrics);
                scan_workflow_value(value, policy, parts_required, metrics, parent_class)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                scan_workflow_value(item, policy, parts_required, metrics, parent_class)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn comfy_part_name_for_key<'a>(
    key: &str,
    value: &'a Value,
) -> Result<Option<&'a str>, ComfyWorkflowDerivationError> {
    let normalized = key.to_ascii_lowercase();
    let looks_like_part = matches!(
        normalized.as_str(),
        "ckpt_name"
            | "checkpoint"
            | "checkpoint_name"
            | "model_name"
            | "unet_name"
            | "vae"
            | "vae_name"
            | "clip_name"
            | "clip_vision_name"
            | "lora"
            | "lora_name"
            | "control_net_name"
            | "controlnet_name"
            | "upscale_model"
            | "upscale_model_name"
            | "audio_encoder"
            | "audio_encoder_name"
            | "audio_encoder_vocal"
            | "audio_encoder_vocal_name"
            | "audio_model"
            | "audio_model_name"
            | "video_model"
            | "video_model_name"
    ) || normalized
        .strip_prefix("clip_name")
        .is_some_and(|suffix| suffix.is_empty() || suffix.chars().all(|ch| ch.is_ascii_digit()))
        || normalized
            .strip_prefix("text_encoder_name")
            .is_some_and(|suffix| {
                suffix.is_empty() || suffix.chars().all(|ch| ch.is_ascii_digit())
            });
    if !looks_like_part {
        return Ok(None);
    }
    let Some(value) = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    if value.len() > MAX_COMFY_WORKFLOW_PART_NAME_BYTES {
        return Err(ComfyWorkflowDerivationError::InvalidGraph(format!(
            "part name for key {key} exceeds {MAX_COMFY_WORKFLOW_PART_NAME_BYTES} bytes"
        )));
    }
    Ok(Some(value))
}

fn update_metric_from_key(key: &str, value: &Value, metrics: &mut OutcomeMetrics) {
    let Some(number) = numeric_u64(value) else {
        return;
    };
    match key.to_ascii_lowercase().as_str() {
        "width" => metrics.width = Some(metrics.width.unwrap_or(0).max(number)),
        "height" => metrics.height = Some(metrics.height.unwrap_or(0).max(number)),
        "steps" | "step_count" | "num_steps" | "inference_steps" => {
            metrics.steps = Some(metrics.steps.unwrap_or(0).max(number))
        }
        "frames" | "frame_count" | "num_frames" | "length" => {
            metrics.frames = Some(metrics.frames.unwrap_or(0).max(number))
        }
        "fps" | "frame_rate" => metrics.fps = Some(metrics.fps.unwrap_or(0).max(number)),
        "duration" | "duration_seconds" | "seconds" => {
            metrics.duration_seconds = Some(metrics.duration_seconds.unwrap_or(0).max(number))
        }
        "batch" | "batch_size" | "n" | "amount" => {
            metrics.artifact_count = Some(metrics.artifact_count.unwrap_or(0).max(number))
        }
        _ => {}
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ImageDimensions {
    width: u64,
    height: u64,
}

fn infer_linked_image_dimensions(
    nodes: &serde_json::Map<String, Value>,
    policy: &ComfyWorkflowDerivationPolicy,
    metrics: &mut OutcomeMetrics,
) -> Result<(), ComfyWorkflowDerivationError> {
    let mut dimensions = BTreeMap::<String, ImageDimensions>::new();
    let mut upscaler_scales = BTreeMap::<String, u64>::new();
    let mut changed = true;
    let mut passes = 0_usize;
    while changed && passes <= nodes.len() {
        changed = false;
        passes += 1;
        for (node_id, node) in nodes {
            let Some(object) = node.as_object() else {
                continue;
            };
            let class_type = object
                .get("class_type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let inputs = object.get("inputs").and_then(Value::as_object);
            if class_type == "UpscaleModelLoader" {
                if let Some(part_name) = inputs
                    .and_then(|inputs| inputs.get("model_name"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    let part = policy.parts_by_name.get(part_name).ok_or_else(|| {
                        ComfyWorkflowDerivationError::MissingPart(part_name.to_owned())
                    })?;
                    if let Some(scale) = part.scale {
                        if upscaler_scales.insert(node_id.clone(), scale) != Some(scale) {
                            changed = true;
                        }
                    }
                }
            }
            let Some(inputs) = inputs else {
                continue;
            };
            let inferred =
                infer_node_image_dimensions(class_type, inputs, &dimensions, &upscaler_scales)?;
            if let Some(inferred) = inferred {
                update_metrics_from_dimensions(metrics, inferred);
                if dimensions.insert(node_id.clone(), inferred) != Some(inferred) {
                    changed = true;
                }
            }
        }
    }
    Ok(())
}

fn infer_node_image_dimensions(
    class_type: &str,
    inputs: &serde_json::Map<String, Value>,
    dimensions: &BTreeMap<String, ImageDimensions>,
    upscaler_scales: &BTreeMap<String, u64>,
) -> Result<Option<ImageDimensions>, ComfyWorkflowDerivationError> {
    if let Some(dimensions) = explicit_dimensions(inputs) {
        return Ok(Some(dimensions));
    }
    match class_type {
        "ImageScale" => Ok(linked_dimensions(inputs, dimensions, &["image", "images"])),
        "ImageScaleBy" => {
            let Some(source) = linked_dimensions(inputs, dimensions, &["image", "images"]) else {
                return Ok(None);
            };
            let Some(scale) = inputs.get("scale_by").and_then(numeric_f64) else {
                return Ok(Some(source));
            };
            if !scale.is_finite() || scale <= 0.0 || scale > 64.0 {
                return Err(ComfyWorkflowDerivationError::InvalidGraph(
                    "ImageScaleBy scale_by must be finite and between 0 and 64".to_owned(),
                ));
            }
            Ok(Some(scale_dimensions(source, scale)))
        }
        "ResizeImageMaskNode" => {
            let Some(source) = linked_dimensions(inputs, dimensions, &["input", "image", "images"])
            else {
                return Ok(None);
            };
            let resize_type = inputs
                .get("resize_type")
                .and_then(Value::as_str)
                .map(|value| value.to_ascii_lowercase());
            let scale = inputs
                .get("resize_type.multiplier")
                .and_then(numeric_f64)
                .or_else(|| inputs.get("multiplier").and_then(numeric_f64));
            let Some(scale) = scale else {
                return Ok(Some(source));
            };
            if let Some(resize_type) = resize_type.as_deref() {
                if !resize_type.contains("multiplier") {
                    return Ok(Some(source));
                }
            }
            if !scale.is_finite() || scale <= 0.0 || scale > 64.0 {
                return Err(ComfyWorkflowDerivationError::InvalidGraph(
                    "ResizeImageMaskNode multiplier must be finite and between 0 and 64".to_owned(),
                ));
            }
            Ok(Some(scale_dimensions(source, scale)))
        }
        "ImageUpscaleWithModel" => {
            let Some(source) = linked_dimensions(inputs, dimensions, &["image", "images"]) else {
                return Ok(None);
            };
            let Some(upscaler_node) = link_node_id(inputs.get("upscale_model")) else {
                return Err(ComfyWorkflowDerivationError::InvalidGraph(
                    "ImageUpscaleWithModel is missing linked upscale_model".to_owned(),
                ));
            };
            let Some(scale) = upscaler_scales.get(upscaler_node) else {
                return Err(ComfyWorkflowDerivationError::InvalidGraph(
                    "ImageUpscaleWithModel requires signed upscaler part scale".to_owned(),
                ));
            };
            Ok(Some(scale_dimensions(source, *scale as f64)))
        }
        _ => Ok(linked_dimensions(
            inputs,
            dimensions,
            &[
                "image",
                "images",
                "input",
                "pixels",
                "resized_images",
                "original_resized_images",
                "samples",
                "latent_image",
            ],
        )),
    }
}

fn explicit_dimensions(inputs: &serde_json::Map<String, Value>) -> Option<ImageDimensions> {
    Some(ImageDimensions {
        width: inputs.get("width").and_then(numeric_u64)?.max(1),
        height: inputs.get("height").and_then(numeric_u64)?.max(1),
    })
}

fn linked_dimensions(
    inputs: &serde_json::Map<String, Value>,
    dimensions: &BTreeMap<String, ImageDimensions>,
    keys: &[&str],
) -> Option<ImageDimensions> {
    for key in keys {
        let Some(node_id) = link_node_id(inputs.get(*key)) else {
            continue;
        };
        if let Some(dimensions) = dimensions.get(node_id) {
            return Some(*dimensions);
        }
    }
    None
}

fn link_node_id(value: Option<&Value>) -> Option<&str> {
    let value = value?.as_array()?;
    value.first()?.as_str()
}

fn scale_dimensions(dimensions: ImageDimensions, scale: f64) -> ImageDimensions {
    ImageDimensions {
        width: ((dimensions.width as f64) * scale).ceil().max(1.0) as u64,
        height: ((dimensions.height as f64) * scale).ceil().max(1.0) as u64,
    }
}

fn update_metrics_from_dimensions(metrics: &mut OutcomeMetrics, dimensions: ImageDimensions) {
    metrics.width = Some(metrics.width.unwrap_or(0).max(dimensions.width));
    metrics.height = Some(metrics.height.unwrap_or(0).max(dimensions.height));
}

fn numeric_u64(value: &Value) -> Option<u64> {
    if let Some(value) = value.as_u64() {
        return Some(value);
    }
    let value = value.as_f64()?;
    if value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= u64::MAX as f64 {
        Some(value as u64)
    } else {
        None
    }
}

fn numeric_f64(value: &Value) -> Option<f64> {
    value.as_f64()
}

fn workflow_usage_from_outcome(
    outcome: &ComfyWorkflowOutcomeSpec,
    pricing_unit: Option<&str>,
) -> ReceiptUsage {
    if let Some(unit) = pricing_unit {
        return workflow_usage_for_pricing_unit(outcome, unit);
    }
    let artifact_count = outcome.artifact_count.max(1);
    let mut units = BTreeMap::new();
    if outcome
        .output_modalities
        .iter()
        .any(|value| value == "image")
    {
        units.insert(USAGE_IMAGE.to_owned(), artifact_count);
    }
    if outcome
        .output_modalities
        .iter()
        .any(|value| value == "video")
    {
        units.insert(
            USAGE_VIDEO_SECOND.to_owned(),
            outcome.duration_seconds.unwrap_or(1).max(1) * artifact_count,
        );
        units.insert(
            USAGE_FRAME.to_owned(),
            outcome.frames.unwrap_or(1).max(1) * artifact_count,
        );
    }
    if outcome
        .output_modalities
        .iter()
        .any(|value| value == "audio")
    {
        units.insert(
            USAGE_AUDIO_SECOND.to_owned(),
            outcome.duration_seconds.unwrap_or(1).max(1) * artifact_count,
        );
    }
    if let Some(steps) = outcome.steps {
        units.insert(USAGE_STEP.to_owned(), steps.max(1) * artifact_count);
    }
    ReceiptUsage::from_units(units)
}

fn workflow_usage_for_pricing_unit(outcome: &ComfyWorkflowOutcomeSpec, unit: &str) -> ReceiptUsage {
    let artifact_count = outcome.artifact_count.max(1);
    let count = match unit {
        USAGE_MEGAPIXEL_STEP => ceil_megapixels(outcome)
            .saturating_mul(
                if outcome
                    .output_modalities
                    .iter()
                    .any(|value| value == "video")
                {
                    outcome.frames.unwrap_or(1).max(1)
                } else {
                    outcome.steps.unwrap_or(1).max(1)
                },
            )
            .saturating_mul(artifact_count),
        USAGE_MEGAPIXEL => ceil_megapixels(outcome).saturating_mul(artifact_count),
        USAGE_FRAME => outcome
            .frames
            .unwrap_or(1)
            .max(1)
            .saturating_mul(artifact_count),
        USAGE_AUDIO_SECOND => outcome
            .duration_seconds
            .unwrap_or(1)
            .max(1)
            .saturating_mul(artifact_count),
        USAGE_COMPUTE_SECOND => outcome
            .duration_seconds
            .or(outcome.steps)
            .or(outcome.frames)
            .unwrap_or(1)
            .max(1)
            .saturating_mul(artifact_count),
        USAGE_STEP => outcome
            .steps
            .unwrap_or(1)
            .max(1)
            .saturating_mul(artifact_count),
        USAGE_IMAGE => artifact_count,
        USAGE_VIDEO_SECOND => outcome
            .duration_seconds
            .unwrap_or(1)
            .max(1)
            .saturating_mul(artifact_count),
        _ => 1_u64.saturating_mul(artifact_count),
    };
    ReceiptUsage::from_units([(unit.to_owned(), count.max(1))])
}

fn ceil_megapixels(outcome: &ComfyWorkflowOutcomeSpec) -> u64 {
    let pixels = outcome
        .width
        .unwrap_or(1)
        .max(1)
        .saturating_mul(outcome.height.unwrap_or(1).max(1));
    pixels.saturating_add(999_999) / 1_000_000
}

fn workflow_output_from_outcome(outcome: &ComfyWorkflowOutcomeSpec) -> WorkflowOutputBinding {
    let mut metrics = BTreeMap::from([("artifact_count".to_owned(), outcome.artifact_count)]);
    if let Some(width) = outcome.width {
        metrics.insert("width".to_owned(), width);
    }
    if let Some(height) = outcome.height {
        metrics.insert("height".to_owned(), height);
    }
    if let Some(frames) = outcome.frames {
        metrics.insert("frame".to_owned(), frames);
    }
    if let Some(duration) = outcome.duration_seconds {
        metrics.insert("duration_second".to_owned(), duration);
    }
    if let Some(steps) = outcome.steps {
        metrics.insert("step".to_owned(), steps);
    }
    WorkflowOutputBinding {
        output_modalities: outcome.output_modalities.clone(),
        metrics,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn policy() -> ComfyWorkflowDerivationPolicy {
        ComfyWorkflowDerivationPolicy {
            whitelisted_nodes: [
                "CheckpointLoaderSimple",
                "KSampler",
                "EmptyLatentImage",
                "VAEDecode",
                "SaveImage",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            parts_by_name: BTreeMap::from([(
                "sdxl.safetensors".to_owned(),
                ComfyWorkflowPartRef {
                    part_id: "11".repeat(32),
                    name: "sdxl.safetensors".to_owned(),
                    part_type: "checkpoint".to_owned(),
                    sha256: "22".repeat(32),
                    scale: None,
                },
            )]),
            max_width: 1_024,
            max_height: 1_024,
            max_frames: 128,
            max_duration_seconds: 30,
            max_steps: 50,
            max_artifacts: 4,
            ..ComfyWorkflowDerivationPolicy::default()
        }
    }

    fn image_graph() -> Value {
        json!({
            "3": {"class_type": "KSampler", "inputs": {"steps": 20, "latent_image": ["5", 0]}},
            "4": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "sdxl.safetensors"}},
            "5": {"class_type": "EmptyLatentImage", "inputs": {"width": 512, "height": 768, "batch_size": 2}},
            "8": {"class_type": "VAEDecode", "inputs": {"samples": ["3", 0], "vae": ["4", 2]}},
            "9": {"class_type": "SaveImage", "inputs": {"images": ["8", 0]}}
        })
    }

    fn image_graph_with_steps(steps: u64) -> Value {
        let mut graph = image_graph();
        graph["3"]["inputs"]["steps"] = json!(steps);
        graph
    }

    #[test]
    fn media_loader_filenames_must_come_from_input_files() {
        let request = json!({
            "workflow": {
                "1": {"class_type": "LoadImage", "inputs": {"image": "refs/hero.png"}},
                "2": {"class_type": "LoadAudio", "inputs": {"audio": "dialogue/line.wav"}}
            },
            "input_files": [
                {"filename": "refs/hero.png"},
                {"filename": "dialogue/line.wav"}
            ]
        });
        validate_comfy_workflow_media_input_file_bindings(&request).unwrap();

        let mut missing = request.clone();
        missing["input_files"] = json!([{"filename": "refs/hero.png"}]);
        let err = validate_comfy_workflow_media_input_file_bindings(&missing)
            .expect_err("audio loader must not use provider-local media");
        assert!(err
            .to_string()
            .contains("dialogue/line.wav but it is not supplied"));

        let unsafe_ref = json!({
            "workflow": {
                "1": {"class_type": "LoadImage", "inputs": {"image": "../secret.png"}}
            }
        });
        let err = validate_comfy_workflow_media_input_file_bindings(&unsafe_ref)
            .expect_err("unsafe media loader filenames must be rejected");
        assert!(err.to_string().contains("unsafe media input filename"));

        let extensionless = json!({
            "workflow": {
                "1": {"class_type": "LoadImage", "inputs": {"image": "refs/hero"}}
            }
        });
        let err = validate_comfy_workflow_media_input_file_bindings(&extensionless)
            .expect_err("extensionless media loader filenames must still be request-bound");
        assert!(err.to_string().contains("refs/hero but it is not supplied"));
    }

    #[test]
    fn h3_r2v_audio_reference_graph_binds_request_audio() {
        let request = json!({
            "workflow": {
                "1": {"class_type": "UNETLoader", "inputs": {"unet_name": "minimax_h3_ref2va_pruned_int8_convrot.safetensors"}},
                "2": {"class_type": "CLIPLoader", "inputs": {"clip_name": "qwen3vl_32b_minimax_h3_nvfp4_awq.safetensors"}},
                "3": {"class_type": "VAELoader", "inputs": {"vae_name": "minimax_h3_video_vae_fp16.safetensors"}},
                "4": {"class_type": "VAELoader", "inputs": {"vae_name": "minimax_h3_audio_vae_fp32.safetensors"}},
                "5": {"class_type": "LoadAudio", "inputs": {"audio": "dialogue/fight-lines.wav"}},
                "6": {"class_type": "MiniMaxH3ReferenceToVideo", "inputs": {
                    "clip": ["2", 0],
                    "vae": ["3", 0],
                    "audio_vae": ["4", 0],
                    "prompt": "<Audio 1> supplies the two fighters' voices while the shot shows fast anime combat.",
                    "width": 896,
                    "height": 512,
                    "length": 124,
                    "ref_audios": {"ref_audio_1": ["5", 0]}
                }},
                "7": {"class_type": "CreateVideo", "inputs": {"images": ["6", 0], "audio": ["6", 1], "fps": 24}},
                "8": {"class_type": "SaveVideo", "inputs": {"video": ["7", 0], "filename_prefix": "mayhem-h3-r2v-audio-ref"}}
            },
            "input_files": [
                {"filename": "dialogue/fight-lines.wav"}
            ]
        });
        validate_comfy_workflow_media_input_file_bindings(&request).unwrap();

        let policy = ComfyWorkflowDerivationPolicy {
            whitelisted_nodes: [
                "UNETLoader",
                "CLIPLoader",
                "VAELoader",
                "LoadAudio",
                "MiniMaxH3ReferenceToVideo",
                "CreateVideo",
                "SaveVideo",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            parts_by_name: BTreeMap::from([
                (
                    "minimax_h3_ref2va_pruned_int8_convrot.safetensors".to_owned(),
                    part(
                        "minimax_h3_ref2va_pruned_int8_convrot.safetensors",
                        "video-model",
                        "71",
                    ),
                ),
                (
                    "qwen3vl_32b_minimax_h3_nvfp4_awq.safetensors".to_owned(),
                    part(
                        "qwen3vl_32b_minimax_h3_nvfp4_awq.safetensors",
                        "text-encoder",
                        "72",
                    ),
                ),
                (
                    "minimax_h3_video_vae_fp16.safetensors".to_owned(),
                    part("minimax_h3_video_vae_fp16.safetensors", "vae", "73"),
                ),
                (
                    "minimax_h3_audio_vae_fp32.safetensors".to_owned(),
                    part("minimax_h3_audio_vae_fp32.safetensors", "vae", "74"),
                ),
            ]),
            pricing_unit: Some(USAGE_MEGAPIXEL_STEP.to_owned()),
            max_width: 1_344,
            max_height: 768,
            max_frames: 362,
            max_duration_seconds: 15,
            max_steps: 24,
            ..ComfyWorkflowDerivationPolicy::default()
        };
        let derivation = derive_comfy_workflow(request.get("workflow").unwrap(), &policy).unwrap();
        assert_eq!(
            derivation.outcome_spec.output_modalities,
            vec!["audio".to_owned(), "video".to_owned()]
        );
        assert_eq!(derivation.outcome_spec.frames, Some(124));
        assert_eq!(derivation.quoted_usage.get(USAGE_MEGAPIXEL_STEP), 124);

        let mut unbound = request.clone();
        unbound["input_files"] = json!([]);
        let err = validate_comfy_workflow_media_input_file_bindings(&unbound)
            .expect_err("H3 reference audio must be request-carried");
        assert!(err
            .to_string()
            .contains("dialogue/fight-lines.wav but it is not supplied"));
    }

    #[test]
    fn allowed_steps_constrains_workflow_step_values() {
        let mut policy = policy();
        policy.max_steps = 8;
        policy.allowed_steps = [4, 6, 8].into_iter().collect();

        for steps in [4, 6, 8] {
            let derivation = derive_comfy_workflow(&image_graph_with_steps(steps), &policy)
                .expect("declared low-step values should pass");
            assert_eq!(derivation.outcome_spec.steps, Some(steps));
        }

        for steps in [5, 20] {
            let err = derive_comfy_workflow(&image_graph_with_steps(steps), &policy)
                .expect_err("undeclared step value should fail");
            assert!(
                err.to_string().contains("steps must be one of 4, 6, 8")
                    || err.to_string().contains("steps exceeds 8"),
                "{err}"
            );
        }
    }

    #[test]
    fn catalog_policy_rejects_invalid_allowed_steps() {
        let mut catalog_policy = ComfyWorkflowCatalogPolicy {
            whitelisted_nodes: vec!["KSampler".to_owned()],
            max_steps: Some(8),
            allowed_steps: vec![4, 4],
            ..ComfyWorkflowCatalogPolicy::default()
        };
        let err = catalog_policy
            .derivation_policy()
            .expect_err("duplicate allowed steps must be invalid");
        assert!(err.to_string().contains("duplicate allowed_steps value 4"));

        catalog_policy.allowed_steps = vec![4, 9];
        let err = catalog_policy
            .derivation_policy()
            .expect_err("allowed steps cannot exceed max_steps");
        assert!(err.to_string().contains("exceeds max_steps 8"));

        catalog_policy.allowed_steps = vec![0];
        let err = catalog_policy
            .derivation_policy()
            .expect_err("zero step value must be invalid");
        assert!(err.to_string().contains("contains zero"));
    }

    fn part(name: &str, part_type: &str, byte: &str) -> ComfyWorkflowPartRef {
        ComfyWorkflowPartRef {
            part_id: byte.repeat(32),
            name: name.to_owned(),
            part_type: part_type.to_owned(),
            sha256: byte.repeat(32),
            scale: None,
        }
    }

    fn av_policy() -> ComfyWorkflowDerivationPolicy {
        ComfyWorkflowDerivationPolicy {
            whitelisted_nodes: [
                "UNETLoader",
                "CLIPLoader",
                "VAELoader",
                "LoraLoader",
                "ControlNetLoader",
                "AudioModelLoader",
                "KSamplerAdvanced",
                "VHS_VideoCombine",
                "SaveAudio",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            parts_by_name: BTreeMap::from([
                (
                    "wan.safetensors".to_owned(),
                    part("wan.safetensors", "video-model", "31"),
                ),
                (
                    "t5xxl_fp16.safetensors".to_owned(),
                    part("t5xxl_fp16.safetensors", "text-encoder", "32"),
                ),
                (
                    "clip_l.safetensors".to_owned(),
                    part("clip_l.safetensors", "text-encoder", "33"),
                ),
                (
                    "wan.vae.safetensors".to_owned(),
                    part("wan.vae.safetensors", "vae", "34"),
                ),
                (
                    "style.safetensors".to_owned(),
                    part("style.safetensors", "lora", "35"),
                ),
                (
                    "control.safetensors".to_owned(),
                    part("control.safetensors", "controlnet", "36"),
                ),
                (
                    "stable-audio.safetensors".to_owned(),
                    part("stable-audio.safetensors", "audio-model", "37"),
                ),
            ]),
            max_width: 1_024,
            max_height: 1_024,
            max_frames: 128,
            max_duration_seconds: 30,
            max_steps: 64,
            max_artifacts: 2,
            ..ComfyWorkflowDerivationPolicy::default()
        }
    }

    fn av_graph() -> Value {
        json!({
            "1": {"class_type": "UNETLoader", "inputs": {"unet_name": "wan.safetensors"}},
            "2": {"class_type": "CLIPLoader", "inputs": {"clip_name1": "t5xxl_fp16.safetensors", "clip_name2": "clip_l.safetensors"}},
            "3": {"class_type": "VAELoader", "inputs": {"vae_name": "wan.vae.safetensors"}},
            "4": {"class_type": "LoraLoader", "inputs": {"lora_name": "style.safetensors", "model": ["1", 0], "clip": ["2", 0]}},
            "5": {"class_type": "ControlNetLoader", "inputs": {"control_net_name": "control.safetensors"}},
            "6": {"class_type": "AudioModelLoader", "inputs": {"audio_model_name": "stable-audio.safetensors"}},
            "7": {"class_type": "KSamplerAdvanced", "inputs": {"steps": 32, "model": ["4", 0], "positive": ["2", 0], "negative": ["2", 1]}},
            "8": {"class_type": "VHS_VideoCombine", "inputs": {"images": ["7", 0], "width": 768, "height": 512, "frame_rate": 24, "length": 96}},
            "9": {"class_type": "SaveAudio", "inputs": {"audio": ["6", 0], "duration_seconds": 12}}
        })
    }

    #[test]
    fn derives_stable_hash_parts_nodes_and_usage() {
        let derivation = derive_comfy_workflow(&image_graph(), &policy()).unwrap();
        assert_eq!(
            derivation.node_set,
            vec![
                "CheckpointLoaderSimple",
                "EmptyLatentImage",
                "KSampler",
                "SaveImage",
                "VAEDecode"
            ]
        );
        assert_eq!(derivation.parts_required.len(), 1);
        assert_eq!(derivation.parts_required[0].name, "sdxl.safetensors");
        assert_eq!(
            derivation.outcome_spec.output_modalities,
            vec!["image".to_owned()]
        );
        assert_eq!(derivation.outcome_spec.width, Some(512));
        assert_eq!(derivation.outcome_spec.height, Some(768));
        assert_eq!(derivation.outcome_spec.steps, Some(20));
        assert_eq!(derivation.outcome_spec.artifact_count, 2);
        assert_eq!(derivation.quoted_usage.get(USAGE_IMAGE), 2);
        assert_eq!(derivation.quoted_usage.get(USAGE_STEP), 40);

        let reordered = json!({
            "9": {"inputs": {"images": ["8", 0]}, "class_type": "SaveImage"},
            "8": {"inputs": {"vae": ["4", 2], "samples": ["3", 0]}, "class_type": "VAEDecode"},
            "5": {"inputs": {"batch_size": 2, "height": 768, "width": 512}, "class_type": "EmptyLatentImage"},
            "4": {"inputs": {"ckpt_name": "sdxl.safetensors"}, "class_type": "CheckpointLoaderSimple"},
            "3": {"inputs": {"latent_image": ["5", 0], "steps": 20}, "class_type": "KSampler"}
        });
        assert_eq!(
            derivation.graph_hash,
            derive_comfy_workflow(&reordered, &policy())
                .unwrap()
                .graph_hash
        );
    }

    #[test]
    fn graph_hash_canonicalizes_integer_valued_json_floats() {
        let mut float_graph = image_graph();
        float_graph["3"]["inputs"]["cfg"] = json!(1.0);
        float_graph["3"]["inputs"]["denoise"] = json!(1.0);
        let mut int_graph = image_graph();
        int_graph["3"]["inputs"]["cfg"] = json!(1);
        int_graph["3"]["inputs"]["denoise"] = json!(1);

        let float_derivation = derive_comfy_workflow(&float_graph, &policy()).unwrap();
        let int_derivation = derive_comfy_workflow(&int_graph, &policy()).unwrap();

        assert_eq!(float_derivation.graph_hash, int_derivation.graph_hash);
        assert_eq!(float_derivation.quoted_usage, int_derivation.quoted_usage);
        assert_eq!(
            float_derivation.workflow_output,
            int_derivation.workflow_output
        );
    }

    #[test]
    fn pricing_unit_derives_grid_units_from_declared_outcome() {
        let mut image_policy = policy();
        image_policy.pricing_unit = Some(USAGE_MEGAPIXEL_STEP.to_owned());
        let image = derive_comfy_workflow(&image_graph(), &image_policy).unwrap();
        assert_eq!(image.quoted_usage.get(USAGE_MEGAPIXEL_STEP), 40);
        assert_eq!(image.quoted_usage.units().len(), 1);

        image_policy.pricing_unit = Some(USAGE_MEGAPIXEL.to_owned());
        let upscaler = derive_comfy_workflow(&image_graph(), &image_policy).unwrap();
        assert_eq!(upscaler.quoted_usage.get(USAGE_MEGAPIXEL), 2);
        assert_eq!(upscaler.quoted_usage.units().len(), 1);

        let mut video_policy = av_policy();
        video_policy.pricing_unit = Some(USAGE_MEGAPIXEL_STEP.to_owned());
        let video = derive_comfy_workflow(&av_graph(), &video_policy).unwrap();
        assert_eq!(video.quoted_usage.get(USAGE_MEGAPIXEL_STEP), 96);
        assert_eq!(video.quoted_usage.units().len(), 1);

        video_policy.pricing_unit = Some(USAGE_AUDIO_SECOND.to_owned());
        let audio = derive_comfy_workflow(&av_graph(), &video_policy).unwrap();
        assert_eq!(audio.quoted_usage.get(USAGE_AUDIO_SECOND), 12);
        assert_eq!(audio.quoted_usage.units().len(), 1);

        assert!(valid_comfy_pricing_unit(USAGE_INPUT_CHARACTER));
    }

    #[test]
    fn custom_node_selector_names_are_required_parts() {
        let policy = ComfyWorkflowDerivationPolicy {
            whitelisted_nodes: [
                "LongCat_Video_SM_Model",
                "LongCat_Video_SM_WhisperModel",
                "LongCat_Video_SM_Sampler",
                "CreateVideo",
                "SaveVideo",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            parts_by_name: BTreeMap::from([
                (
                    "LongCat-Video-Avatar-vae.safetensors".to_owned(),
                    part("LongCat-Video-Avatar-vae.safetensors", "vae", "51"),
                ),
                (
                    "longcat-avatar-dmd_lora.safetensors".to_owned(),
                    part("longcat-avatar-dmd_lora.safetensors", "lora", "52"),
                ),
                (
                    "whisper-large-v3.safetensors".to_owned(),
                    part("whisper-large-v3.safetensors", "audio-model", "53"),
                ),
            ]),
            max_width: 720,
            max_height: 480,
            max_frames: 125,
            max_duration_seconds: 5,
            max_steps: 8,
            ..ComfyWorkflowDerivationPolicy::default()
        };
        let graph = json!({
            "1": {"class_type": "LongCat_Video_SM_Model", "inputs": {
                "inference_weight_mode": "official_int8_sharded",
                "vae": "LongCat-Video-Avatar-vae.safetensors",
                "lora": "longcat-avatar-dmd_lora.safetensors"
            }},
            "2": {"class_type": "LongCat_Video_SM_WhisperModel", "inputs": {
                "audio_encoder": "whisper-large-v3.safetensors"
            }},
            "3": {"class_type": "LongCat_Video_SM_Sampler", "inputs": {
                "model": ["1", 0],
                "audio_encoder": ["2", 0],
                "width": 720,
                "height": 480,
                "length": 125,
                "steps": 8
            }},
            "4": {"class_type": "CreateVideo", "inputs": {
                "images": ["3", 0],
                "fps": 25
            }},
            "5": {"class_type": "SaveVideo", "inputs": {
                "video": ["4", 0],
                "filename_prefix": "mayhem-longcat/avatar"
            }}
        });

        let required = derive_comfy_workflow(&graph, &policy)
            .unwrap()
            .parts_required
            .into_iter()
            .map(|part| part.name)
            .collect::<Vec<_>>();

        assert_eq!(
            required,
            vec![
                "LongCat-Video-Avatar-vae.safetensors",
                "longcat-avatar-dmd_lora.safetensors",
                "whisper-large-v3.safetensors"
            ]
        );

        let mut missing_whisper = policy.clone();
        missing_whisper
            .parts_by_name
            .remove("whisper-large-v3.safetensors");
        assert_eq!(
            derive_comfy_workflow(&graph, &missing_whisper).unwrap_err(),
            ComfyWorkflowDerivationError::MissingPart("whisper-large-v3.safetensors".to_owned())
        );
    }

    #[test]
    fn signed_upscaler_scale_expands_derived_image_usage() {
        let mut policy = ComfyWorkflowDerivationPolicy {
            whitelisted_nodes: [
                "UNETLoader",
                "CLIPLoader",
                "VAELoader",
                "CLIPTextEncode",
                "ConditioningZeroOut",
                "EmptyLatentImage",
                "KSampler",
                "VAEDecode",
                "UpscaleModelLoader",
                "ImageUpscaleWithModel",
                "SaveImage",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            parts_by_name: BTreeMap::from([
                (
                    "krea2_turbo_fp8_scaled.safetensors".to_owned(),
                    part("krea2_turbo_fp8_scaled.safetensors", "checkpoint", "41"),
                ),
                (
                    "qwen3vl_4b_fp8_scaled.safetensors".to_owned(),
                    part("qwen3vl_4b_fp8_scaled.safetensors", "text-encoder", "42"),
                ),
                (
                    "qwen_image_vae.safetensors".to_owned(),
                    part("qwen_image_vae.safetensors", "vae", "43"),
                ),
                (
                    "4x-spanx4-ch48.safetensors".to_owned(),
                    ComfyWorkflowPartRef {
                        scale: Some(4),
                        ..part("4x-spanx4-ch48.safetensors", "upscaler", "44")
                    },
                ),
            ]),
            pricing_unit: Some(USAGE_MEGAPIXEL_STEP.to_owned()),
            max_width: 4_096,
            max_height: 4_096,
            max_steps: 8,
            ..ComfyWorkflowDerivationPolicy::default()
        };
        let graph = json!({
            "1": {"class_type": "UNETLoader", "inputs": {"unet_name": "krea2_turbo_fp8_scaled.safetensors", "weight_dtype": "default"}},
            "2": {"class_type": "CLIPLoader", "inputs": {"clip_name": "qwen3vl_4b_fp8_scaled.safetensors", "type": "krea2", "device": "default"}},
            "3": {"class_type": "VAELoader", "inputs": {"vae_name": "qwen_image_vae.safetensors"}},
            "4": {"class_type": "CLIPTextEncode", "inputs": {"clip": ["2", 0], "text": "commercial product shot"}},
            "5": {"class_type": "ConditioningZeroOut", "inputs": {"conditioning": ["4", 0]}},
            "6": {"class_type": "EmptyLatentImage", "inputs": {"width": 1024, "height": 1024, "batch_size": 1}},
            "7": {"class_type": "KSampler", "inputs": {"seed": 7, "steps": 8, "cfg": 1, "sampler_name": "euler", "scheduler": "simple", "denoise": 1, "model": ["1", 0], "positive": ["4", 0], "negative": ["5", 0], "latent_image": ["6", 0]}},
            "8": {"class_type": "VAEDecode", "inputs": {"samples": ["7", 0], "vae": ["3", 0]}},
            "9": {"class_type": "UpscaleModelLoader", "inputs": {"model_name": "4x-spanx4-ch48.safetensors"}},
            "10": {"class_type": "ImageUpscaleWithModel", "inputs": {"upscale_model": ["9", 0], "image": ["8", 0]}},
            "11": {"class_type": "SaveImage", "inputs": {"images": ["10", 0], "filename_prefix": "mayhem-krea2-4x"}}
        });

        let derivation = derive_comfy_workflow(&graph, &policy).unwrap();
        let mut required_names = derivation
            .parts_required
            .iter()
            .map(|part| part.name.as_str())
            .collect::<Vec<_>>();
        required_names.sort_unstable();

        assert_eq!(derivation.outcome_spec.width, Some(4_096));
        assert_eq!(derivation.outcome_spec.height, Some(4_096));
        assert_eq!(derivation.quoted_usage.get(USAGE_MEGAPIXEL_STEP), 136);
        assert_eq!(
            required_names,
            vec![
                "4x-spanx4-ch48.safetensors",
                "krea2_turbo_fp8_scaled.safetensors",
                "qwen3vl_4b_fp8_scaled.safetensors",
                "qwen_image_vae.safetensors"
            ]
        );

        policy.max_width = 1_024;
        let err = derive_comfy_workflow(&graph, &policy).unwrap_err();
        assert_eq!(
            err,
            ComfyWorkflowDerivationError::OutcomeOverflow("width exceeds 1024".to_owned())
        );
    }

    #[test]
    fn resize_image_mask_branch_expands_video_workflow_usage() {
        let policy = ComfyWorkflowDerivationPolicy {
            whitelisted_nodes: [
                "UNETLoader",
                "CLIPLoader",
                "VAELoader",
                "MiniMaxH3ImageToVideo",
                "BasicGuider",
                "RandomNoise",
                "KSamplerSelect",
                "BasicScheduler",
                "SamplerCustomAdvanced",
                "VAEDecode",
                "VAEDecodeAudio",
                "ResizeImageMaskNode",
                "SeedVR2Preprocess",
                "VAEEncodeTiled",
                "SeedVR2Conditioning",
                "KSampler",
                "VAEDecodeTiled",
                "SeedVR2PostProcessing",
                "CreateVideo",
                "SaveVideo",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            parts_by_name: BTreeMap::from([
                (
                    "minimax_h3_fl2va_pruned_int8_convrot.safetensors".to_owned(),
                    part(
                        "minimax_h3_fl2va_pruned_int8_convrot.safetensors",
                        "video-model",
                        "61",
                    ),
                ),
                (
                    "qwen3vl_32b_minimax_h3_nvfp4_awq.safetensors".to_owned(),
                    part(
                        "qwen3vl_32b_minimax_h3_nvfp4_awq.safetensors",
                        "text-encoder",
                        "62",
                    ),
                ),
                (
                    "minimax_h3_video_vae_fp16.safetensors".to_owned(),
                    part("minimax_h3_video_vae_fp16.safetensors", "vae", "63"),
                ),
                (
                    "minimax_h3_audio_vae_fp32.safetensors".to_owned(),
                    part("minimax_h3_audio_vae_fp32.safetensors", "vae", "64"),
                ),
                (
                    "seedvr2_3b_int8_convrot.safetensors".to_owned(),
                    part("seedvr2_3b_int8_convrot.safetensors", "video-model", "65"),
                ),
                (
                    "seedvr2_ema_vae_fp16.safetensors".to_owned(),
                    part("seedvr2_ema_vae_fp16.safetensors", "vae", "66"),
                ),
            ]),
            pricing_unit: Some(USAGE_MEGAPIXEL_STEP.to_owned()),
            max_width: 1_792,
            max_height: 1_024,
            max_frames: 124,
            max_steps: 20,
            max_artifacts: 1,
            ..ComfyWorkflowDerivationPolicy::default()
        };
        let graph = json!({
            "1": {"class_type": "UNETLoader", "inputs": {"unet_name": "minimax_h3_fl2va_pruned_int8_convrot.safetensors", "weight_dtype": "default"}},
            "2": {"class_type": "CLIPLoader", "inputs": {"clip_name": "qwen3vl_32b_minimax_h3_nvfp4_awq.safetensors", "type": "minimax", "device": "default"}},
            "3": {"class_type": "VAELoader", "inputs": {"vae_name": "minimax_h3_video_vae_fp16.safetensors"}},
            "4": {"class_type": "VAELoader", "inputs": {"vae_name": "minimax_h3_audio_vae_fp32.safetensors"}},
            "5": {"class_type": "MiniMaxH3ImageToVideo", "inputs": {"clip": ["2", 0], "vae": ["3", 0], "prompt": "anime fight", "width": 896, "height": 512, "length": 124}},
            "6": {"class_type": "BasicGuider", "inputs": {"model": ["1", 0], "conditioning": ["5", 0]}},
            "7": {"class_type": "RandomNoise", "inputs": {"noise_seed": 17}},
            "8": {"class_type": "KSamplerSelect", "inputs": {"sampler_name": "res_multistep"}},
            "9": {"class_type": "BasicScheduler", "inputs": {"model": ["1", 0], "scheduler": "simple", "steps": 20, "denoise": 1}},
            "10": {"class_type": "SamplerCustomAdvanced", "inputs": {"noise": ["7", 0], "guider": ["6", 0], "sampler": ["8", 0], "sigmas": ["9", 0], "latent_image": ["5", 1]}},
            "11": {"class_type": "VAEDecode", "inputs": {"samples": ["10", 0], "vae": ["3", 0]}},
            "12": {"class_type": "VAEDecodeAudio", "inputs": {"samples": ["10", 0], "vae": ["4", 0]}},
            "13": {"class_type": "ResizeImageMaskNode", "inputs": {"input": ["11", 0], "resize_type": "scale by multiplier", "resize_type.multiplier": 2, "scale_method": "lanczos"}},
            "14": {"class_type": "VAELoader", "inputs": {"vae_name": "seedvr2_ema_vae_fp16.safetensors"}},
            "15": {"class_type": "UNETLoader", "inputs": {"unet_name": "seedvr2_3b_int8_convrot.safetensors", "weight_dtype": "default"}},
            "16": {"class_type": "SeedVR2Preprocess", "inputs": {"resized_images": ["13", 0]}},
            "17": {"class_type": "VAEEncodeTiled", "inputs": {"pixels": ["16", 0], "vae": ["14", 0], "tile_size": 512, "overlap": 128, "temporal_size": 64, "temporal_overlap": 8}},
            "18": {"class_type": "SeedVR2Conditioning", "inputs": {"model": ["15", 0], "vae_conditioning": ["17", 0]}},
            "19": {"class_type": "KSampler", "inputs": {"seed": 7, "steps": 1, "cfg": 1, "sampler_name": "euler", "scheduler": "simple", "denoise": 1, "model": ["15", 0], "positive": ["18", 0], "negative": ["18", 1], "latent_image": ["17", 0]}},
            "20": {"class_type": "VAEDecodeTiled", "inputs": {"samples": ["19", 0], "vae": ["14", 0], "tile_size": 512, "overlap": 128, "temporal_size": 64, "temporal_overlap": 8}},
            "21": {"class_type": "SeedVR2PostProcessing", "inputs": {"images": ["20", 0], "original_resized_images": ["13", 0], "color_correction_method": "none"}},
            "22": {"class_type": "CreateVideo", "inputs": {"images": ["21", 0], "audio": ["12", 0], "fps": 24, "bit_depth": 8}},
            "23": {"class_type": "SaveVideo", "inputs": {"video": ["22", 0], "filename_prefix": "mayhem-h3/anime-fight-seed17-upscale", "format": "mp4", "codec": "auto"}}
        });

        let derivation = derive_comfy_workflow(&graph, &policy).unwrap();

        assert_eq!(derivation.outcome_spec.width, Some(1_792));
        assert_eq!(derivation.outcome_spec.height, Some(1_024));
        assert_eq!(derivation.outcome_spec.frames, Some(124));
        assert_eq!(
            derivation.outcome_spec.output_modalities,
            vec!["audio".to_owned(), "video".to_owned()]
        );
        assert_eq!(derivation.quoted_usage.get(USAGE_MEGAPIXEL_STEP), 248);
    }

    #[test]
    fn refuses_non_whitelisted_node() {
        let mut graph = image_graph();
        graph["666"] = json!({"class_type": "ShellExec", "inputs": {}});
        let err = derive_comfy_workflow(&graph, &policy()).unwrap_err();
        assert_eq!(
            err,
            ComfyWorkflowDerivationError::NonWhitelistedNode("ShellExec".to_owned())
        );
    }

    #[test]
    fn derives_realistic_av_graph_parts_modalities_and_usage() {
        let derivation = derive_comfy_workflow(&av_graph(), &av_policy()).unwrap();
        let names = derivation
            .parts_required
            .iter()
            .map(|part| part.name.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            names,
            BTreeSet::from([
                "clip_l.safetensors",
                "control.safetensors",
                "stable-audio.safetensors",
                "style.safetensors",
                "t5xxl_fp16.safetensors",
                "wan.safetensors",
                "wan.vae.safetensors"
            ])
        );
        assert_eq!(
            derivation.outcome_spec.output_modalities,
            vec!["audio".to_owned(), "video".to_owned()]
        );
        assert_eq!(derivation.outcome_spec.width, Some(768));
        assert_eq!(derivation.outcome_spec.height, Some(512));
        assert_eq!(derivation.outcome_spec.frames, Some(96));
        assert_eq!(derivation.outcome_spec.duration_seconds, Some(12));
        assert_eq!(derivation.outcome_spec.steps, Some(32));
        assert_eq!(derivation.quoted_usage.get(USAGE_AUDIO_SECOND), 12);
        assert_eq!(derivation.quoted_usage.get(USAGE_VIDEO_SECOND), 12);
        assert_eq!(derivation.quoted_usage.get(USAGE_FRAME), 96);
        assert_eq!(derivation.quoted_usage.get(USAGE_STEP), 32);
    }

    #[test]
    fn derives_video_frames_from_seconds_and_fps_when_length_is_absent() {
        let mut policy = av_policy();
        policy.pricing_unit = Some(USAGE_MEGAPIXEL_STEP.to_owned());
        policy
            .whitelisted_nodes
            .extend(["MiniMaxH3Easy", "BasicScheduler", "CreateVideo"].map(str::to_owned));
        policy.max_height = 1_280;
        policy.max_frames = 512;
        policy.max_steps = 8;

        let graph = json!({
            "1": {
                "class_type": "MiniMaxH3Easy",
                "inputs": {
                    "width": 736,
                    "height": 1280,
                    "seconds": 10,
                    "fps": 24
                }
            },
            "2": {
                "class_type": "BasicScheduler",
                "inputs": { "steps": 4 }
            },
            "3": {
                "class_type": "CreateVideo",
                "inputs": {
                    "images": ["1", 0],
                    "fps": 24
                }
            }
        });

        let derivation = derive_comfy_workflow(&graph, &policy).unwrap();
        assert_eq!(derivation.outcome_spec.frames, Some(240));
        assert_eq!(derivation.outcome_spec.duration_seconds, Some(10));
        assert_eq!(derivation.quoted_usage.get(USAGE_MEGAPIXEL_STEP), 240);
    }

    #[test]
    fn refuses_malformed_graph_records_and_empty_policy() {
        let cases = [
            (
                json!([]),
                policy(),
                ComfyWorkflowDerivationError::InvalidGraph(
                    "graph must be a JSON object".to_owned(),
                ),
            ),
            (
                json!({}),
                policy(),
                ComfyWorkflowDerivationError::InvalidGraph(
                    "graph must contain at least one node".to_owned(),
                ),
            ),
            (
                json!({"1": null}),
                policy(),
                ComfyWorkflowDerivationError::InvalidGraph("node 1 must be an object".to_owned()),
            ),
            (
                json!({"1": {"inputs": {}}}),
                policy(),
                ComfyWorkflowDerivationError::InvalidGraph(
                    "node 1 is missing class_type".to_owned(),
                ),
            ),
            (
                json!({"1": {"class_type": " ", "inputs": {}}}),
                policy(),
                ComfyWorkflowDerivationError::InvalidGraph(
                    "node 1 is missing class_type".to_owned(),
                ),
            ),
            (
                image_graph(),
                ComfyWorkflowDerivationPolicy::default(),
                ComfyWorkflowDerivationError::InvalidGraph("node whitelist is empty".to_owned()),
            ),
        ];
        for (graph, policy, expected) in cases {
            assert_eq!(derive_comfy_workflow(&graph, &policy), Err(expected));
        }
    }

    #[test]
    fn refuses_overlong_loader_part_name() {
        let mut graph = image_graph();
        graph["4"]["inputs"]["ckpt_name"] =
            json!("x".repeat(MAX_COMFY_WORKFLOW_PART_NAME_BYTES + 1));
        let err = derive_comfy_workflow(&graph, &policy()).unwrap_err();
        assert_eq!(
            err,
            ComfyWorkflowDerivationError::InvalidGraph(format!(
                "part name for key ckpt_name exceeds {MAX_COMFY_WORKFLOW_PART_NAME_BYTES} bytes"
            ))
        );
    }

    #[test]
    fn refuses_av_outcome_caps() {
        let mut too_many_frames = av_graph();
        too_many_frames["8"]["inputs"]["length"] = json!(129);
        assert_eq!(
            derive_comfy_workflow(&too_many_frames, &av_policy()),
            Err(ComfyWorkflowDerivationError::OutcomeOverflow(
                "frames exceeds 128".to_owned()
            ))
        );

        let mut too_long = av_graph();
        too_long["9"]["inputs"]["duration_seconds"] = json!(31);
        assert_eq!(
            derive_comfy_workflow(&too_long, &av_policy()),
            Err(ComfyWorkflowDerivationError::OutcomeOverflow(
                "duration_seconds exceeds 30".to_owned()
            ))
        );

        let mut too_many_steps = av_graph();
        too_many_steps["7"]["inputs"]["steps"] = json!(65);
        assert_eq!(
            derive_comfy_workflow(&too_many_steps, &av_policy()),
            Err(ComfyWorkflowDerivationError::OutcomeOverflow(
                "steps exceeds 64".to_owned()
            ))
        );

        let mut too_many_outputs = image_graph();
        too_many_outputs["5"]["inputs"]["batch_size"] = json!(5);
        assert_eq!(
            derive_comfy_workflow(&too_many_outputs, &policy()),
            Err(ComfyWorkflowDerivationError::OutcomeOverflow(
                "artifact_count exceeds 4".to_owned()
            ))
        );
    }

    #[test]
    fn catalog_policy_builds_bounded_derivation_policy() {
        let policy = ComfyWorkflowCatalogPolicy {
            whitelisted_nodes: vec![
                "CheckpointLoaderSimple".to_owned(),
                "KSampler".to_owned(),
                "EmptyLatentImage".to_owned(),
                "VAEDecode".to_owned(),
                "SaveImage".to_owned(),
            ],
            parts: vec![ComfyWorkflowPartRef {
                part_id: "11".repeat(32),
                name: "sdxl.safetensors".to_owned(),
                part_type: "checkpoint".to_owned(),
                sha256: "22".repeat(32),
                scale: None,
            }],
            runtime_id: Some("comfyui-v0.31.0".to_owned()),
            outcome_class: Some("image.light.512".to_owned()),
            pricing_unit: Some(USAGE_MEGAPIXEL_STEP.to_owned()),
            inventory_root: Some("33".repeat(32)),
            max_width: Some(512),
            ..ComfyWorkflowCatalogPolicy::default()
        };

        let derived_policy = policy.derivation_policy().unwrap();
        let mut graph = image_graph();
        graph["5"]["inputs"]["width"] = json!(768);
        let err = derive_comfy_workflow(&graph, &derived_policy).unwrap_err();

        assert_eq!(policy.runtime_id(), "comfyui-v0.31.0");
        assert_eq!(policy.outcome_class_for("image"), "image.light.512");
        assert_eq!(
            derived_policy.pricing_unit.as_deref(),
            Some(USAGE_MEGAPIXEL_STEP)
        );
        assert_eq!(
            err,
            ComfyWorkflowDerivationError::OutcomeOverflow("width exceeds 512".to_owned())
        );
    }

    #[test]
    fn catalog_policy_refuses_unknown_pricing_unit() {
        let policy = ComfyWorkflowCatalogPolicy {
            whitelisted_nodes: vec!["SaveImage".to_owned()],
            pricing_unit: Some("seller_second".to_owned()),
            ..ComfyWorkflowCatalogPolicy::default()
        };
        assert_eq!(
            policy.derivation_policy().unwrap_err(),
            ComfyWorkflowDerivationError::InvalidPolicy(
                "unsupported pricing_unit seller_second".to_owned()
            )
        );
    }

    #[test]
    fn refuses_missing_part() {
        let mut graph = image_graph();
        graph["4"]["inputs"]["ckpt_name"] = json!("missing.safetensors");
        let err = derive_comfy_workflow(&graph, &policy()).unwrap_err();
        assert_eq!(
            err,
            ComfyWorkflowDerivationError::MissingPart("missing.safetensors".to_owned())
        );
    }

    #[test]
    fn refuses_outcome_overflow() {
        let mut graph = image_graph();
        graph["5"]["inputs"]["width"] = json!(2048);
        let err = derive_comfy_workflow(&graph, &policy()).unwrap_err();
        assert_eq!(
            err,
            ComfyWorkflowDerivationError::OutcomeOverflow("width exceeds 1024".to_owned())
        );
    }
}
