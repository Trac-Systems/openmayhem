use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    stable_json_bytes, ReceiptUsage, WorkflowOutputBinding, USAGE_AUDIO_SECOND, USAGE_FRAME,
    USAGE_IMAGE, USAGE_STEP, USAGE_VIDEO_SECOND,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_artifacts: Option<u64>,
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
        }
        let defaults = ComfyWorkflowDerivationPolicy::default();
        Ok(ComfyWorkflowDerivationPolicy {
            whitelisted_nodes,
            parts_by_name,
            max_nodes: self.max_nodes.unwrap_or(defaults.max_nodes).max(1),
            max_width: self.max_width.unwrap_or(defaults.max_width).max(1),
            max_height: self.max_height.unwrap_or(defaults.max_height).max(1),
            max_frames: self.max_frames.unwrap_or(defaults.max_frames).max(1),
            max_duration_seconds: self
                .max_duration_seconds
                .unwrap_or(defaults.max_duration_seconds)
                .max(1),
            max_steps: self.max_steps.unwrap_or(defaults.max_steps).max(1),
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComfyWorkflowDerivationPolicy {
    pub whitelisted_nodes: BTreeSet<String>,
    pub parts_by_name: BTreeMap<String, ComfyWorkflowPartRef>,
    pub max_nodes: usize,
    pub max_width: u64,
    pub max_height: u64,
    pub max_frames: u64,
    pub max_duration_seconds: u64,
    pub max_steps: u64,
    pub max_artifacts: u64,
}

impl Default for ComfyWorkflowDerivationPolicy {
    fn default() -> Self {
        Self {
            whitelisted_nodes: BTreeSet::new(),
            parts_by_name: BTreeMap::new(),
            max_nodes: DEFAULT_COMFY_WORKFLOW_MAX_NODES,
            max_width: DEFAULT_COMFY_WORKFLOW_MAX_WIDTH,
            max_height: DEFAULT_COMFY_WORKFLOW_MAX_HEIGHT,
            max_frames: DEFAULT_COMFY_WORKFLOW_MAX_FRAMES,
            max_duration_seconds: DEFAULT_COMFY_WORKFLOW_MAX_DURATION_SECONDS,
            max_steps: DEFAULT_COMFY_WORKFLOW_MAX_STEPS,
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

    let outcome_spec = metrics.into_outcome_spec(policy)?;
    let quoted_usage = workflow_usage_from_outcome(&outcome_spec);
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

fn graph_hash(graph: &Value) -> Result<String, ComfyWorkflowDerivationError> {
    let graph_bytes = stable_json_bytes(graph)
        .map_err(|err| ComfyWorkflowDerivationError::Hash(err.to_string()))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(GRAPH_HASH_DOMAIN);
    hasher.update(&(graph_bytes.len() as u64).to_le_bytes());
    hasher.update(&graph_bytes);
    Ok(hasher.finalize().to_hex().to_string())
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
        let frames = self.frames;
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
                if let Some(part_name) = comfy_part_name_for_key(key, value) {
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

fn comfy_part_name_for_key<'a>(key: &str, value: &'a Value) -> Option<&'a str> {
    let key = key.to_ascii_lowercase();
    let looks_like_part = matches!(
        key.as_str(),
        "ckpt_name"
            | "checkpoint"
            | "checkpoint_name"
            | "model_name"
            | "unet_name"
            | "vae_name"
            | "clip_name"
            | "clip_vision_name"
            | "lora_name"
            | "control_net_name"
            | "controlnet_name"
            | "upscale_model"
            | "upscale_model_name"
            | "audio_model"
            | "audio_model_name"
            | "video_model"
            | "video_model_name"
    );
    if !looks_like_part {
        return None;
    }
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= MAX_COMFY_WORKFLOW_PART_NAME_BYTES)
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

fn workflow_usage_from_outcome(outcome: &ComfyWorkflowOutcomeSpec) -> ReceiptUsage {
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
            }],
            runtime_id: Some("comfyui-v0.31.0".to_owned()),
            outcome_class: Some("image.light.512".to_owned()),
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
            err,
            ComfyWorkflowDerivationError::OutcomeOverflow("width exceeds 512".to_owned())
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
