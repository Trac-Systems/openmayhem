use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::stable_json_bytes;

pub const COMFY_PART_RECORD_SCHEMA_VERSION: u32 = 1;
pub const COMFY_PARTS_INDEX_SCHEMA_VERSION: u32 = 1;
pub const COMFY_PARTS_ANCHOR_SCHEMA_VERSION: u32 = 1;

const PART_ID_DOMAIN: &[u8] = b"mayhem:comfy-part-id:v1";
const PART_LEAF_DOMAIN: &[u8] = b"mayhem:comfy-parts-leaf:v1";
const PART_BRANCH_DOMAIN: &[u8] = b"mayhem:comfy-parts-branch:v1";
const PART_EMPTY_INDEX_DOMAIN: &[u8] = b"mayhem:comfy-parts-empty-index:v1";
const PART_ANCHOR_DOMAIN: &[u8] = b"mayhem:comfy-parts-anchor:v1";

const PART_TYPES: &[&str] = &[
    "audio-model",
    "checkpoint",
    "clip-vision",
    "controlnet",
    "custom-node",
    "lipsync",
    "lora",
    "text-encoder",
    "tts",
    "upscaler",
    "vae",
    "video-model",
];

const PART_STATUSES: &[&str] = &["draft", "linked", "policy-review", "in-catalog", "excluded"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyPartRecord {
    #[serde(rename = "v")]
    pub schema_version: u32,
    pub part_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub part_type: String,
    pub lane: String,
    pub sha256: String,
    pub blake3_root: String,
    pub size_bytes: u64,
    pub file_format: String,
    pub license: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy_flags: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub adapter: BTreeMap<String, Value>,
    pub min_runtime: String,
    pub sources: ComfyPartSources,
    pub license_evidence: ComfyPartLicenseEvidence,
    pub canary: ComfyPartCanary,
    pub status: String,
}

impl ComfyPartRecord {
    pub fn validate(&self) -> Result<(), ComfyPartsCatalogError> {
        if self.schema_version != COMFY_PART_RECORD_SCHEMA_VERSION {
            return Err(ComfyPartsCatalogError::InvalidSchemaVersion {
                kind: "part record",
                expected: COMFY_PART_RECORD_SCHEMA_VERSION,
                got: self.schema_version,
            });
        }
        validate_non_empty("part_id", &self.part_id)?;
        validate_non_empty("name", &self.name)?;
        validate_enum("type", &self.part_type, PART_TYPES)?;
        validate_non_empty("lane", &self.lane)?;
        validate_hex("sha256", &self.sha256)?;
        let expected_part_id = derive_comfy_part_id(&self.part_type, &self.name, &self.sha256);
        if self.part_id != expected_part_id {
            return Err(ComfyPartsCatalogError::InvalidField {
                field: "part_id",
                reason: "does not match type/name/sha256 identity".to_owned(),
            });
        }
        validate_hex("blake3_root", &self.blake3_root)?;
        if self.size_bytes == 0 {
            return Err(ComfyPartsCatalogError::InvalidField {
                field: "size_bytes",
                reason: "must be greater than zero".to_owned(),
            });
        }
        validate_non_empty("file_format", &self.file_format)?;
        validate_non_empty("license", &self.license)?;
        validate_non_empty("min_runtime", &self.min_runtime)?;
        validate_enum("status", &self.status, PART_STATUSES)?;
        self.sources.validate()?;
        self.license_evidence.validate()?;
        self.canary.validate()?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyPartSources {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mirrors: Vec<ComfyPartSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub origins: Vec<ComfyPartSource>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub require_auth: bool,
}

impl ComfyPartSources {
    fn validate(&self) -> Result<(), ComfyPartsCatalogError> {
        if self.mirrors.is_empty() && self.origins.is_empty() {
            return Err(ComfyPartsCatalogError::InvalidField {
                field: "sources",
                reason: "must contain at least one mirror or origin".to_owned(),
            });
        }
        for source in self.mirrors.iter().chain(self.origins.iter()) {
            source.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyPartSource {
    pub kind: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

impl ComfyPartSource {
    fn validate(&self) -> Result<(), ComfyPartsCatalogError> {
        validate_non_empty("source.kind", &self.kind)?;
        validate_https("source.url", &self.url)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyPartLicenseEvidence {
    pub doc_hash: String,
    pub archived_ref: String,
    pub captured_at: String,
}

impl ComfyPartLicenseEvidence {
    fn validate(&self) -> Result<(), ComfyPartsCatalogError> {
        validate_hex("license_evidence.doc_hash", &self.doc_hash)?;
        validate_non_empty("license_evidence.archived_ref", &self.archived_ref)?;
        validate_non_empty("license_evidence.captured_at", &self.captured_at)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyPartCanary {
    pub probe_graph_hash: String,
    pub reference_output_ref: String,
    pub tolerance: ComfyPartCanaryTolerance,
}

impl ComfyPartCanary {
    fn validate(&self) -> Result<(), ComfyPartsCatalogError> {
        validate_hex("canary.probe_graph_hash", &self.probe_graph_hash)?;
        validate_non_empty("canary.reference_output_ref", &self.reference_output_ref)?;
        self.tolerance.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyPartCanaryTolerance {
    pub method: String,
    pub max_distance_bps: u32,
}

impl ComfyPartCanaryTolerance {
    fn validate(&self) -> Result<(), ComfyPartsCatalogError> {
        validate_non_empty("canary.tolerance.method", &self.method)?;
        let method = self.method.trim().to_ascii_lowercase();
        if !matches!(
            method.as_str(),
            "phash"
                | "average_hash"
                | "image_average_hash"
                | "seed_perceptual_hash"
                | "audio_fingerprint"
                | "video_av_fingerprint"
                | "sha256"
                | "exact_sha256"
        ) {
            return Err(ComfyPartsCatalogError::InvalidField {
                field: "canary.tolerance.method",
                reason: format!("unsupported method {}", self.method),
            });
        }
        if self.max_distance_bps > 10_000 {
            return Err(ComfyPartsCatalogError::InvalidField {
                field: "canary.tolerance.max_distance_bps",
                reason: "must be at most 10000".to_owned(),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyPartDraft {
    pub name: String,
    #[serde(rename = "type")]
    pub part_type: String,
    pub lane: String,
    pub sha256: String,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub size_bytes_exact: bool,
    pub file_format: String,
    pub license: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy_flags: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub adapter: BTreeMap<String, Value>,
    pub sources: ComfyPartSources,
    pub status: String,
}

impl ComfyPartDraft {
    pub fn from_yaml_value(value: &Value) -> Result<Self, ComfyPartsCatalogError> {
        let object = value
            .as_object()
            .ok_or_else(|| ComfyPartsCatalogError::Yaml("part row must be an object".to_owned()))?;
        let name = yaml_string(object, "name")?;
        let part_type = yaml_string(object, "type")
            .or_else(|_| infer_part_type(object))
            .map(normalize_slug)?;
        let lane = yaml_string(object, "lane")
            .or_else(|_| yaml_string(object, "lane_fit"))
            .unwrap_or_else(|_| "all".to_owned());
        let sha256 = normalize_hex(&yaml_string(object, "sha256")?)?;
        let (size_bytes, size_bytes_exact) = match yaml_u64(object, "size_bytes") {
            Ok(size_bytes) => (size_bytes, true),
            Err(_) => (yaml_size_gb(object)?, false),
        };
        let file_format =
            yaml_string(object, "file_format").unwrap_or_else(|_| infer_file_format(object));
        let license = yaml_string(object, "license")?;
        let permissions = parse_permissions(object.get("permissions"));
        let policy_flags = parse_string_list(object.get("policy_flags"))?;
        let status = yaml_string(object, "status").unwrap_or_else(|_| "draft".to_owned());
        let sources = if let Some(value) = object.get("sources") {
            let sources: ComfyPartSources =
                serde_json::from_value(value.clone()).map_err(|error| {
                    ComfyPartsCatalogError::InvalidField {
                        field: "sources",
                        reason: error.to_string(),
                    }
                })?;
            sources.validate()?;
            sources
        } else {
            yaml_sources(object)?
        };
        let mut adapter = BTreeMap::new();
        for key in [
            "role",
            "architecture",
            "scale",
            "priority",
            "version_id",
            "version_name",
            "repository",
            "file",
            "file_path",
            "note",
            "notes",
            "admission_note",
            "openmodeldb",
        ] {
            if let Some(value) = object.get(key) {
                adapter.insert(key.to_owned(), value.clone());
            }
        }
        if let Some(value) = object.get("adapter") {
            let explicit =
                value
                    .as_object()
                    .ok_or_else(|| ComfyPartsCatalogError::InvalidField {
                        field: "adapter",
                        reason: "must be an object".to_owned(),
                    })?;
            for (key, value) in explicit {
                adapter.insert(key.to_owned(), value.clone());
            }
        }
        let draft = Self {
            name,
            part_type,
            lane,
            sha256,
            size_bytes,
            size_bytes_exact,
            file_format,
            license,
            permissions,
            policy_flags,
            adapter,
            sources,
            status,
        };
        draft.validate()?;
        Ok(draft)
    }

    pub fn validate(&self) -> Result<(), ComfyPartsCatalogError> {
        validate_non_empty("name", &self.name)?;
        validate_enum("type", &self.part_type, PART_TYPES)?;
        validate_non_empty("lane", &self.lane)?;
        validate_hex("sha256", &self.sha256)?;
        if self.size_bytes == 0 {
            return Err(ComfyPartsCatalogError::InvalidField {
                field: "size_bytes",
                reason: "must be greater than zero".to_owned(),
            });
        }
        validate_non_empty("file_format", &self.file_format)?;
        validate_non_empty("license", &self.license)?;
        validate_enum("status", &self.status, PART_STATUSES)?;
        self.sources.validate()
    }

    pub fn finalize(
        self,
        blake3_root: String,
        min_runtime: String,
        license_evidence: ComfyPartLicenseEvidence,
        canary: ComfyPartCanary,
    ) -> Result<ComfyPartRecord, ComfyPartsCatalogError> {
        let record = ComfyPartRecord {
            schema_version: COMFY_PART_RECORD_SCHEMA_VERSION,
            part_id: derive_comfy_part_id(&self.part_type, &self.name, &self.sha256),
            name: self.name,
            part_type: self.part_type,
            lane: self.lane,
            sha256: self.sha256,
            blake3_root,
            size_bytes: self.size_bytes,
            file_format: self.file_format,
            license: self.license,
            permissions: self.permissions,
            policy_flags: self.policy_flags,
            adapter: self.adapter,
            min_runtime,
            sources: self.sources,
            license_evidence,
            canary,
            status: self.status,
        };
        record.validate()?;
        Ok(record)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyPartsIndexEntry {
    pub part_id: String,
    pub record_hash: String,
    #[serde(rename = "type")]
    pub part_type: String,
    pub lane: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyPartsIndex {
    #[serde(rename = "v")]
    pub schema_version: u32,
    pub index_ver: u32,
    pub root: String,
    pub parts: Vec<ComfyPartsIndexEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyPartMerkleProof {
    pub leaf_index: u64,
    pub leaf_count: u64,
    pub leaf_hash: String,
    pub siblings: Vec<ComfyPartMerkleSibling>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyPartMerkleSibling {
    pub side: ComfyPartMerkleSide,
    pub hash: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComfyPartMerkleSide {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComfyPartsAnchor {
    #[serde(rename = "v")]
    pub schema_version: u32,
    pub parts_index_root: String,
    pub index_ver: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blessed_runtimes: Vec<String>,
    pub whitelist_ver: u32,
    pub outcome_classes_ver: u32,
}

impl ComfyPartsAnchor {
    pub fn validate(&self) -> Result<(), ComfyPartsCatalogError> {
        if self.schema_version != COMFY_PARTS_ANCHOR_SCHEMA_VERSION {
            return Err(ComfyPartsCatalogError::InvalidSchemaVersion {
                kind: "parts anchor",
                expected: COMFY_PARTS_ANCHOR_SCHEMA_VERSION,
                got: self.schema_version,
            });
        }
        validate_hex("parts_index_root", &self.parts_index_root)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComfyPartsCatalogError {
    InvalidSchemaVersion {
        kind: &'static str,
        expected: u32,
        got: u32,
    },
    InvalidField {
        field: &'static str,
        reason: String,
    },
    DuplicatePartId(String),
    PartNotFound(String),
    InvalidProof(String),
    Json(String),
    Yaml(String),
}

impl fmt::Display for ComfyPartsCatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSchemaVersion {
                kind,
                expected,
                got,
            } => write!(
                f,
                "{kind} schema version mismatch: expected {expected}, got {got}"
            ),
            Self::InvalidField { field, reason } => write!(f, "{field} is invalid: {reason}"),
            Self::DuplicatePartId(part_id) => write!(f, "duplicate part_id {part_id}"),
            Self::PartNotFound(part_id) => write!(f, "part_id {part_id} is not in the index"),
            Self::InvalidProof(reason) => write!(f, "invalid parts merkle proof: {reason}"),
            Self::Json(reason) => write!(f, "parts catalog JSON error: {reason}"),
            Self::Yaml(reason) => write!(f, "parts catalog YAML error: {reason}"),
        }
    }
}

impl std::error::Error for ComfyPartsCatalogError {}

pub fn derive_comfy_part_id(part_type: &str, name: &str, sha256: &str) -> String {
    let mut input = Vec::new();
    input.extend_from_slice(PART_ID_DOMAIN);
    input.push(0);
    input.extend_from_slice(part_type.as_bytes());
    input.push(0);
    input.extend_from_slice(name.as_bytes());
    input.push(0);
    input.extend_from_slice(sha256.to_ascii_lowercase().as_bytes());
    blake3::hash(&input).to_hex().to_string()
}

pub fn comfy_part_record_hash(record: &ComfyPartRecord) -> Result<String, ComfyPartsCatalogError> {
    record.validate()?;
    let value = serde_json::to_value(record).map_err(|err| {
        ComfyPartsCatalogError::Json(format!("serializing part record failed: {err}"))
    })?;
    let bytes = stable_json_bytes(&value).map_err(|err| {
        ComfyPartsCatalogError::Json(format!("canonicalizing part record failed: {err}"))
    })?;
    Ok(domain_hash_hex(PART_LEAF_DOMAIN, &[&bytes]))
}

pub fn comfy_parts_anchor_hash(
    anchor: &ComfyPartsAnchor,
) -> Result<String, ComfyPartsCatalogError> {
    anchor.validate()?;
    let value = serde_json::to_value(anchor).map_err(|err| {
        ComfyPartsCatalogError::Json(format!("serializing parts anchor failed: {err}"))
    })?;
    let bytes = stable_json_bytes(&value).map_err(|err| {
        ComfyPartsCatalogError::Json(format!("canonicalizing parts anchor failed: {err}"))
    })?;
    Ok(domain_hash_hex(PART_ANCHOR_DOMAIN, &[&bytes]))
}

pub fn build_comfy_parts_index(
    records: &[ComfyPartRecord],
    index_ver: u32,
) -> Result<ComfyPartsIndex, ComfyPartsCatalogError> {
    let sorted = sorted_records(records)?;
    let mut parts = Vec::with_capacity(sorted.len());
    let mut leaves = Vec::with_capacity(sorted.len());
    for record in sorted {
        let record_hash = comfy_part_record_hash(record)?;
        leaves.push(record_hash.clone());
        parts.push(ComfyPartsIndexEntry {
            part_id: record.part_id.clone(),
            record_hash,
            part_type: record.part_type.clone(),
            lane: record.lane.clone(),
            status: record.status.clone(),
        });
    }
    Ok(ComfyPartsIndex {
        schema_version: COMFY_PARTS_INDEX_SCHEMA_VERSION,
        index_ver,
        root: merkle_root_from_leaves(&leaves)?,
        parts,
    })
}

pub fn prove_comfy_part(
    records: &[ComfyPartRecord],
    part_id: &str,
) -> Result<ComfyPartMerkleProof, ComfyPartsCatalogError> {
    let sorted = sorted_records(records)?;
    let leaves = sorted
        .iter()
        .map(|record| comfy_part_record_hash(record))
        .collect::<Result<Vec<_>, _>>()?;
    let position = sorted
        .iter()
        .position(|record| record.part_id == part_id)
        .ok_or_else(|| ComfyPartsCatalogError::PartNotFound(part_id.to_owned()))?;
    let siblings = merkle_siblings(&leaves, position)?;
    Ok(ComfyPartMerkleProof {
        leaf_index: u64::try_from(position)
            .map_err(|_| ComfyPartsCatalogError::InvalidProof("leaf index overflow".to_owned()))?,
        leaf_count: u64::try_from(leaves.len())
            .map_err(|_| ComfyPartsCatalogError::InvalidProof("leaf count overflow".to_owned()))?,
        leaf_hash: leaves[position].clone(),
        siblings,
    })
}

pub fn verify_comfy_part_proof(
    record: &ComfyPartRecord,
    proof: &ComfyPartMerkleProof,
    root: &str,
) -> Result<(), ComfyPartsCatalogError> {
    validate_hex("parts_index_root", root)?;
    if proof.leaf_count == 0 {
        return Err(ComfyPartsCatalogError::InvalidProof(
            "leaf_count must be greater than zero".to_owned(),
        ));
    }
    if proof.leaf_index >= proof.leaf_count {
        return Err(ComfyPartsCatalogError::InvalidProof(
            "leaf_index must be less than leaf_count".to_owned(),
        ));
    }
    let expected_leaf = comfy_part_record_hash(record)?;
    if proof.leaf_hash != expected_leaf {
        return Err(ComfyPartsCatalogError::InvalidProof(
            "leaf hash does not match the part record".to_owned(),
        ));
    }
    let mut hash = proof.leaf_hash.clone();
    let mut index = proof.leaf_index;
    let mut count = proof.leaf_count;
    let mut siblings = proof.siblings.iter();
    while count > 1 {
        if index % 2 == 0 {
            if index + 1 < count {
                let sibling = siblings.next().ok_or_else(|| {
                    ComfyPartsCatalogError::InvalidProof("missing right sibling".to_owned())
                })?;
                if sibling.side != ComfyPartMerkleSide::Right {
                    return Err(ComfyPartsCatalogError::InvalidProof(
                        "expected right sibling".to_owned(),
                    ));
                }
                validate_hex("proof sibling hash", &sibling.hash)?;
                hash = merkle_branch_hash(&hash, &sibling.hash)?;
            }
        } else {
            let sibling = siblings.next().ok_or_else(|| {
                ComfyPartsCatalogError::InvalidProof("missing left sibling".to_owned())
            })?;
            if sibling.side != ComfyPartMerkleSide::Left {
                return Err(ComfyPartsCatalogError::InvalidProof(
                    "expected left sibling".to_owned(),
                ));
            }
            validate_hex("proof sibling hash", &sibling.hash)?;
            hash = merkle_branch_hash(&sibling.hash, &hash)?;
        }
        index /= 2;
        count = count.div_ceil(2);
    }
    if siblings.next().is_some() {
        return Err(ComfyPartsCatalogError::InvalidProof(
            "proof contains extra siblings".to_owned(),
        ));
    }
    if hash == root {
        Ok(())
    } else {
        Err(ComfyPartsCatalogError::InvalidProof(
            "recomputed root does not match".to_owned(),
        ))
    }
}

fn sorted_records(
    records: &[ComfyPartRecord],
) -> Result<Vec<&ComfyPartRecord>, ComfyPartsCatalogError> {
    let mut seen = BTreeSet::new();
    for record in records {
        record.validate()?;
        if !seen.insert(record.part_id.clone()) {
            return Err(ComfyPartsCatalogError::DuplicatePartId(
                record.part_id.clone(),
            ));
        }
    }
    let mut sorted = records.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.part_id.cmp(&right.part_id));
    Ok(sorted)
}

fn merkle_root_from_leaves(leaves: &[String]) -> Result<String, ComfyPartsCatalogError> {
    if leaves.is_empty() {
        return Ok(blake3::hash(PART_EMPTY_INDEX_DOMAIN).to_hex().to_string());
    }
    for leaf in leaves {
        validate_hex("leaf hash", leaf)?;
    }
    let mut layer = leaves.to_vec();
    while layer.len() > 1 {
        let mut next = Vec::with_capacity(layer.len().div_ceil(2));
        for pair in layer.chunks(2) {
            if let [left, right] = pair {
                next.push(merkle_branch_hash(left, right)?);
            } else {
                next.push(pair[0].clone());
            }
        }
        layer = next;
    }
    Ok(layer[0].clone())
}

fn merkle_siblings(
    leaves: &[String],
    position: usize,
) -> Result<Vec<ComfyPartMerkleSibling>, ComfyPartsCatalogError> {
    if leaves.is_empty() || position >= leaves.len() {
        return Err(ComfyPartsCatalogError::InvalidProof(
            "leaf position is out of bounds".to_owned(),
        ));
    }
    let mut siblings = Vec::new();
    let mut index = position;
    let mut layer = leaves.to_vec();
    while layer.len() > 1 {
        if index % 2 == 0 {
            if index + 1 < layer.len() {
                siblings.push(ComfyPartMerkleSibling {
                    side: ComfyPartMerkleSide::Right,
                    hash: layer[index + 1].clone(),
                });
            }
        } else {
            siblings.push(ComfyPartMerkleSibling {
                side: ComfyPartMerkleSide::Left,
                hash: layer[index - 1].clone(),
            });
        }
        let mut next = Vec::with_capacity(layer.len().div_ceil(2));
        for pair in layer.chunks(2) {
            if let [left, right] = pair {
                next.push(merkle_branch_hash(left, right)?);
            } else {
                next.push(pair[0].clone());
            }
        }
        index /= 2;
        layer = next;
    }
    Ok(siblings)
}

fn merkle_branch_hash(left: &str, right: &str) -> Result<String, ComfyPartsCatalogError> {
    validate_hex("left branch hash", left)?;
    validate_hex("right branch hash", right)?;
    Ok(domain_hash_hex(
        PART_BRANCH_DOMAIN,
        &[left.as_bytes(), right.as_bytes()],
    ))
}

fn domain_hash_hex(domain: &[u8], chunks: &[&[u8]]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    for chunk in chunks {
        hasher.update(&[0]);
        hasher.update(chunk);
    }
    hasher.finalize().to_hex().to_string()
}

fn yaml_sources(
    object: &serde_json::Map<String, Value>,
) -> Result<ComfyPartSources, ComfyPartsCatalogError> {
    let mut origins = Vec::new();
    let source_revision = object
        .get("revision")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|revision| !revision.is_empty())
        .map(str::to_owned);
    let source_path = object
        .get("file")
        .or_else(|| object.get("file_path"))
        .and_then(Value::as_str)
        .map(safe_comfy_source_path)
        .transpose()?;
    if let Some(url) = object.get("download_url").and_then(Value::as_str) {
        origins.push(ComfyPartSource {
            kind: infer_source_kind(url).to_owned(),
            url: url.to_owned(),
            repository: object
                .get("repository")
                .and_then(Value::as_str)
                .map(str::to_owned),
            path: source_path.clone(),
            revision: None,
        });
    }
    if let Some(source) = object.get("source").and_then(Value::as_str) {
        let source_kind = infer_source_kind(source);
        if origins.is_empty() && source_kind == "huggingface" {
            if let Some(path) = source_path.as_deref() {
                let repository = source.trim().trim_end_matches('/').to_owned();
                let revision = source_revision.clone().unwrap_or_else(|| "main".to_owned());
                origins.push(ComfyPartSource {
                    kind: "huggingface".to_owned(),
                    url: format!("{repository}/resolve/{revision}/{path}"),
                    repository: Some(repository),
                    path: Some(path.to_owned()),
                    revision: source_revision,
                });
                return Ok(ComfyPartSources {
                    mirrors: Vec::new(),
                    origins,
                    require_auth: false,
                });
            }
        }
        origins.push(ComfyPartSource {
            kind: source_kind.to_owned(),
            url: source.to_owned(),
            repository: None,
            path: None,
            revision: object
                .get("version_id")
                .and_then(value_to_u64)
                .map(|version| version.to_string()),
        });
    }
    if origins.is_empty() {
        return Err(ComfyPartsCatalogError::InvalidField {
            field: "sources",
            reason: "YAML row has no download_url or source".to_owned(),
        });
    }
    origins.sort_by(|left, right| {
        comfy_part_source_payload_rank(left)
            .cmp(&comfy_part_source_payload_rank(right))
            .then_with(|| left.url.cmp(&right.url))
    });
    origins.dedup_by(|left, right| left.url == right.url);
    let require_auth = origins.iter().any(|origin| origin.kind == "civitai");
    Ok(ComfyPartSources {
        mirrors: Vec::new(),
        origins,
        require_auth,
    })
}

fn comfy_part_source_payload_rank(source: &ComfyPartSource) -> u8 {
    if source.path.is_some() || looks_like_direct_comfy_payload_url(&source.url) {
        0
    } else {
        1
    }
}

fn looks_like_direct_comfy_payload_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains("/resolve/")
        || lower.contains("/api/download/")
        || lower.contains("/releases/download/")
        || lower.ends_with(".safetensors")
        || lower.ends_with(".gguf")
        || lower.ends_with(".pt")
        || lower.ends_with(".pth")
        || lower.ends_with(".bin")
}

fn safe_comfy_source_path(value: &str) -> Result<String, ComfyPartsCatalogError> {
    let path = value.trim();
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(ComfyPartsCatalogError::InvalidField {
            field: "file_path",
            reason: "must be a safe relative repository path".to_owned(),
        });
    }
    Ok(path.to_owned())
}

fn infer_source_kind(url: &str) -> &'static str {
    if url.contains("huggingface.co") {
        "huggingface"
    } else if url.contains("civitai.com") {
        "civitai"
    } else if url.contains("github.com") {
        "github"
    } else {
        "https"
    }
}

fn infer_part_type(
    object: &serde_json::Map<String, Value>,
) -> Result<String, ComfyPartsCatalogError> {
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let role = object
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.contains("vae") || role.contains("vae") {
        Ok("vae".to_owned())
    } else if name.contains("clip-vision") || role.contains("vision encoder") {
        Ok("clip-vision".to_owned())
    } else if role.contains("lipsync") || role.contains("lip sync") {
        Ok("lipsync".to_owned())
    } else if role.contains("tts") || role.contains("voice") {
        Ok("tts".to_owned())
    } else if object.get("scale").is_some()
        || role.contains("upscale")
        || role.contains("rescale")
        || name.contains("upscale")
        || name.starts_with("1x-")
        || name.starts_with("2x-")
        || name.starts_with("4x-")
        || name.starts_with("8x-")
    {
        Ok("upscaler".to_owned())
    } else {
        Ok("controlnet".to_owned())
    }
}

fn infer_file_format(object: &serde_json::Map<String, Value>) -> String {
    for key in ["file", "file_path", "download_url"] {
        if let Some(value) = object.get(key).and_then(Value::as_str) {
            let lower = value.to_ascii_lowercase();
            if lower.ends_with(".safetensors") {
                return "safetensors".to_owned();
            }
            if lower.ends_with(".pth") {
                return "pickle(.pth)".to_owned();
            }
            if lower.ends_with(".pt") {
                return "pickle(.pt)".to_owned();
            }
            if lower.ends_with(".bin") {
                return "pickle(.bin)".to_owned();
            }
        }
    }
    "unknown".to_owned()
}

fn parse_permissions(value: Option<&Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let mut permissions = match value {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        Value::String(text) => text
            .trim_matches(|ch| ch == '{' || ch == '}')
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    permissions.sort();
    permissions.dedup();
    permissions
}

fn parse_string_list(value: Option<&Value>) -> Result<Vec<String>, ComfyPartsCatalogError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let mut items = match value {
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    ComfyPartsCatalogError::Yaml("list item must be a string".to_owned())
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        Value::String(value) => vec![value.clone()],
        _ => {
            return Err(ComfyPartsCatalogError::Yaml(
                "expected string or string list".to_owned(),
            ))
        }
    };
    items.sort();
    items.dedup();
    Ok(items)
}

fn yaml_string(
    object: &serde_json::Map<String, Value>,
    key: &'static str,
) -> Result<String, ComfyPartsCatalogError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ComfyPartsCatalogError::Yaml(format!("{key} must be a non-empty string")))
}

fn yaml_u64(
    object: &serde_json::Map<String, Value>,
    key: &'static str,
) -> Result<u64, ComfyPartsCatalogError> {
    object
        .get(key)
        .and_then(value_to_u64)
        .ok_or_else(|| ComfyPartsCatalogError::Yaml(format!("{key} must be a positive integer")))
}

fn yaml_size_gb(object: &serde_json::Map<String, Value>) -> Result<u64, ComfyPartsCatalogError> {
    let gb = object
        .get("size_gb")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| {
            ComfyPartsCatalogError::Yaml("size_bytes or size_gb must be present".to_owned())
        })?;
    Ok((gb * 1024.0 * 1024.0 * 1024.0).round() as u64)
}

fn value_to_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse::<u64>().ok(),
        _ => None,
    }
}

fn normalize_slug(value: String) -> String {
    value.trim().to_ascii_lowercase().replace('_', "-")
}

fn normalize_hex(value: &str) -> Result<String, ComfyPartsCatalogError> {
    let normalized = value.to_ascii_lowercase();
    validate_hex("hex", &normalized)?;
    Ok(normalized)
}

fn validate_non_empty(field: &'static str, value: &str) -> Result<(), ComfyPartsCatalogError> {
    if value.trim().is_empty() {
        return Err(ComfyPartsCatalogError::InvalidField {
            field,
            reason: "must be non-empty".to_owned(),
        });
    }
    Ok(())
}

fn validate_enum(
    field: &'static str,
    value: &str,
    allowed: &[&str],
) -> Result<(), ComfyPartsCatalogError> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(ComfyPartsCatalogError::InvalidField {
            field,
            reason: format!("must be one of {}", allowed.join(", ")),
        })
    }
}

fn validate_hex(field: &'static str, value: &str) -> Result<(), ComfyPartsCatalogError> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(ComfyPartsCatalogError::InvalidField {
            field,
            reason: "must be a 32-byte hex digest".to_owned(),
        })
    }
}

fn validate_https(field: &'static str, value: &str) -> Result<(), ComfyPartsCatalogError> {
    if value.starts_with("https://") {
        Ok(())
    } else {
        Err(ComfyPartsCatalogError::InvalidField {
            field,
            reason: "must be an HTTPS URL".to_owned(),
        })
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HEX_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const HEX_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    fn evidence() -> ComfyPartLicenseEvidence {
        ComfyPartLicenseEvidence {
            doc_hash: HEX_A.to_owned(),
            archived_ref: "https://example.test/license".to_owned(),
            captured_at: "2026-08-04T00:00:00Z".to_owned(),
        }
    }

    fn canary() -> ComfyPartCanary {
        ComfyPartCanary {
            probe_graph_hash: HEX_B.to_owned(),
            reference_output_ref: "hf://TracNetwork/openmayhem-parts-canaries/example.png"
                .to_owned(),
            tolerance: ComfyPartCanaryTolerance {
                method: "seed_perceptual_hash".to_owned(),
                max_distance_bps: 100,
            },
        }
    }

    #[test]
    fn canary_tolerance_rejects_unknown_method_and_overflow_distance() {
        let mut unknown_method = canary();
        unknown_method.tolerance.method = "shell_exec".to_owned();
        assert!(unknown_method.validate().is_err());

        let mut overflow_distance = canary();
        overflow_distance.tolerance.max_distance_bps = 10_001;
        assert!(overflow_distance.validate().is_err());

        let valid = canary();
        assert!(valid.validate().is_ok());
    }

    fn record(name: &str, sha256: &str) -> ComfyPartRecord {
        ComfyPartDraft {
            name: name.to_owned(),
            part_type: "upscaler".to_owned(),
            lane: "all lanes".to_owned(),
            sha256: sha256.to_owned(),
            size_bytes: 64,
            size_bytes_exact: true,
            file_format: "safetensors".to_owned(),
            license: "MIT".to_owned(),
            permissions: vec!["Rent".to_owned()],
            policy_flags: Vec::new(),
            adapter: BTreeMap::new(),
            sources: ComfyPartSources {
                mirrors: Vec::new(),
                origins: vec![ComfyPartSource {
                    kind: "huggingface".to_owned(),
                    url: "https://huggingface.co/example/model/resolve/main/model.safetensors"
                        .to_owned(),
                    repository: Some("example/model".to_owned()),
                    path: Some("model.safetensors".to_owned()),
                    revision: Some("main".to_owned()),
                }],
                require_auth: false,
            },
            status: "linked".to_owned(),
        }
        .finalize(
            HEX_C.to_owned(),
            "comfyui-v0.30.1".to_owned(),
            evidence(),
            canary(),
        )
        .unwrap()
    }

    #[test]
    fn part_record_hash_uses_stable_json_order() {
        let mut first = record("alpha", HEX_A);
        first.adapter.insert("z".to_owned(), Value::from(1));
        first.adapter.insert("a".to_owned(), Value::from(2));
        let mut second = record("alpha", HEX_A);
        second.adapter.insert("a".to_owned(), Value::from(2));
        second.adapter.insert("z".to_owned(), Value::from(1));
        assert_eq!(
            comfy_part_record_hash(&first).unwrap(),
            comfy_part_record_hash(&second).unwrap()
        );
    }

    #[test]
    fn custom_node_parts_are_valid_catalog_records() {
        let mut record = record("ComfyUI-Spectrum-MiniMax-H3", HEX_A);
        record.part_type = "custom-node".to_owned();
        record.file_format = "tar.gz".to_owned();
        record.part_id = derive_comfy_part_id(&record.part_type, &record.name, &record.sha256);
        record.adapter.insert(
            "comfy_custom_node_dir".to_owned(),
            Value::from("ComfyUI-Spectrum-MiniMax-H3"),
        );

        record.validate().unwrap();
    }

    #[test]
    fn parts_index_root_and_proof_verify_sorted_records() {
        let records = vec![
            record("gamma", HEX_C),
            record("alpha", HEX_A),
            record("beta", HEX_B),
        ];
        let index = build_comfy_parts_index(&records, 7).unwrap();
        assert_eq!(index.schema_version, COMFY_PARTS_INDEX_SCHEMA_VERSION);
        assert_eq!(index.index_ver, 7);
        assert_eq!(index.parts.len(), 3);

        let target = &records[1];
        let proof = prove_comfy_part(&records, &target.part_id).unwrap();
        verify_comfy_part_proof(target, &proof, &index.root).unwrap();

        let mut tampered = target.clone();
        tampered.size_bytes += 1;
        assert!(verify_comfy_part_proof(&tampered, &proof, &index.root).is_err());

        let mut wrong_identity = target.clone();
        wrong_identity.part_id =
            derive_comfy_part_id("upscaler", "different", &wrong_identity.sha256);
        assert!(wrong_identity.validate().is_err());
    }

    #[test]
    fn yaml_import_accepts_checkpoint_controlnet_and_upscaler_shapes() {
        let rows = vec![
            serde_json::json!({
                "name": "Illustrious-XL v0.1",
                "type": "checkpoint",
                "lane": "sdxl",
                "source": "https://civitai.com/models/795765",
                "version_id": 889818_u64,
                "sha256": "3E15BA00387DB678AB4A099F75771C4F5AC67FDA9E7100A01D263EAF30145AA9",
                "size_gb": 6.5,
                "license": "TBD",
                "permissions": "{RentCivit,Rent}",
                "policy_flags": ["nsfw"],
                "status": "linked",
            }),
            serde_json::json!({
                "name": "ControlNet Union SDXL promax",
                "role": "all-in-one conditioning",
                "lane_fit": "SDXL / Illustrious / Pony",
                "repository": "https://huggingface.co/xinsir/controlnet-union-sdxl-1.0",
                "file": "diffusion_pytorch_model_promax.safetensors",
                "license": "apache-2.0",
                "file_format": "safetensors",
                "sha256": "9fae2e50cb431bfcbe05822b59ec2228df545ef27f711dea8949e9f4ed9f7cdc",
                "size_bytes": 2513342408_u64,
                "download_url": "https://huggingface.co/xinsir/controlnet-union-sdxl-1.0/resolve/main/diffusion_pytorch_model_promax.safetensors",
                "status": "linked",
            }),
            serde_json::json!({
                "name": "4x_NMKD-Superscale-SP_178000_G",
                "role": "general 4x workhorse",
                "lane_fit": "all lanes",
                "architecture": "esrgan",
                "scale": 4,
                "license": "WTFPL",
                "status": "linked",
                "file_format": "pickle(.pth)",
                "sha256": "1d1b0078fe71446e0469d8d4df59e96baa80d83cda600d68237d655830821bcc",
                "size_bytes": 66958607_u64,
                "download_url": "https://huggingface.co/uwg/upscaler/resolve/main/ESRGAN/4x_NMKD-Superscale-SP_178000_G.pth",
            }),
        ];
        let drafts = rows
            .iter()
            .map(ComfyPartDraft::from_yaml_value)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(drafts.len(), 3);
        assert_eq!(drafts[0].part_type, "checkpoint");
        assert_eq!(
            drafts[0].sha256,
            "3e15ba00387db678ab4a099f75771c4f5ac67fda9e7100a01d263eaf30145aa9"
        );
        assert!(!drafts[0].size_bytes_exact);
        assert_eq!(drafts[0].size_bytes, 6_979_321_856);
        assert_eq!(
            drafts[0].permissions,
            vec!["Rent".to_owned(), "RentCivit".to_owned()]
        );
        assert!(drafts[0].sources.require_auth);
        assert_eq!(drafts[1].part_type, "controlnet");
        assert!(drafts[1].size_bytes_exact);
        assert_eq!(drafts[2].part_type, "upscaler");

        let finalized = drafts[2]
            .clone()
            .finalize(
                HEX_C.to_owned(),
                "comfyui-v0.30.1".to_owned(),
                evidence(),
                canary(),
            )
            .unwrap();
        assert_eq!(finalized.schema_version, COMFY_PART_RECORD_SCHEMA_VERSION);
        assert_eq!(
            finalized.part_id,
            derive_comfy_part_id(&finalized.part_type, &finalized.name, &finalized.sha256)
        );
    }

    #[test]
    fn yaml_import_resolves_huggingface_file_path_sources() {
        let row = serde_json::json!({
            "name": "UMT5-XXL fp8 scaled",
            "type": "text-encoder",
            "lane": "wan",
            "source": "https://huggingface.co/Comfy-Org/Wan_2.2_ComfyUI_Repackaged",
            "file_path": "split_files/text_encoders/umt5_xxl_fp8_e4m3fn_scaled.safetensors",
            "sha256": "c3355d30191f1f066b26d93fba017ae9809dce6c627dda5f6a66eaa651204f68",
            "size_bytes": 7_237_545_820_u64,
            "license": "apache-2.0",
            "status": "linked",
        });
        let draft = ComfyPartDraft::from_yaml_value(&row).unwrap();
        assert_eq!(draft.file_format, "safetensors");
        assert_eq!(
            draft.sources.origins[0].url,
            "https://huggingface.co/Comfy-Org/Wan_2.2_ComfyUI_Repackaged/resolve/main/split_files/text_encoders/umt5_xxl_fp8_e4m3fn_scaled.safetensors"
        );
        assert_eq!(
            draft.sources.origins[0].path.as_deref(),
            Some("split_files/text_encoders/umt5_xxl_fp8_e4m3fn_scaled.safetensors")
        );
    }

    #[test]
    fn yaml_import_resolves_huggingface_file_path_sources_with_revision() {
        let revision = "014cd40f7e177756c6b2473c0d93b1c89a790dd2";
        let row = serde_json::json!({
            "name": "MiniMax H3 audio VAE fp32",
            "type": "vae",
            "lane": "minimax-h3",
            "source": "https://huggingface.co/Comfy-Org/MiniMax-H3",
            "file_path": "vae/minimax_h3_audio_vae_fp32.safetensors",
            "revision": revision,
            "sha256": "d132ce0297fda95139762b689c22de3507581b897c03f766964a9edfee8c8d3c",
            "size_bytes": 605_254_808_u64,
            "license": "other",
            "status": "linked",
        });
        let draft = ComfyPartDraft::from_yaml_value(&row).unwrap();
        assert_eq!(
            draft.sources.origins[0].url,
            format!(
                "https://huggingface.co/Comfy-Org/MiniMax-H3/resolve/{revision}/vae/minimax_h3_audio_vae_fp32.safetensors"
            )
        );
        assert_eq!(draft.sources.origins[0].revision.as_deref(), Some(revision));
    }

    #[test]
    fn yaml_import_prefers_payload_urls_over_repository_pages() {
        let row = serde_json::json!({
            "name": "LatentSync 1.6 UNet",
            "type": "lipsync",
            "lane": "shared",
            "source": "https://huggingface.co/ByteDance/LatentSync-1.6",
            "file": "latentsync_unet.pt",
            "download_url": "https://huggingface.co/ByteDance/LatentSync-1.6/resolve/main/latentsync_unet.pt",
            "sha256": "0a478e89eb660f82da4c35dbdde8a5adfb27f99d1b4e50edd03729e1e98316d3",
            "size_bytes": 5_443_871_048_u64,
            "file_format": "pickle",
            "license": "openrail++",
            "status": "linked",
        });
        let draft = ComfyPartDraft::from_yaml_value(&row).unwrap();
        assert_eq!(draft.sources.origins.len(), 2);
        assert_eq!(
            draft.sources.origins[0].url,
            "https://huggingface.co/ByteDance/LatentSync-1.6/resolve/main/latentsync_unet.pt"
        );
        assert_eq!(
            draft.sources.origins[0].path.as_deref(),
            Some("latentsync_unet.pt")
        );
        assert_eq!(
            draft.sources.origins[1].url,
            "https://huggingface.co/ByteDance/LatentSync-1.6"
        );
    }

    #[test]
    fn yaml_import_marks_civitai_download_urls_auth_required() {
        let row = serde_json::json!({
            "name": "Illustrious-XL v0.1",
            "type": "checkpoint",
            "lane": "sdxl",
            "source": "https://civitai.com/models/795765",
            "version_id": 889818_u64,
            "sha256": "3E15BA00387DB678AB4A099F75771C4F5AC67FDA9E7100A01D263EAF30145AA9",
            "size_bytes": 6_938_040_760_u64,
            "file_format": "safetensors",
            "download_url": "https://civitai.com/api/download/models/889818",
            "license": "TBD",
            "status": "linked",
        });
        let draft = ComfyPartDraft::from_yaml_value(&row).unwrap();
        assert!(draft.sources.require_auth);
        assert!(draft
            .sources
            .origins
            .iter()
            .any(|origin| origin.kind == "civitai"
                && origin.url == "https://civitai.com/api/download/models/889818"));
    }

    #[test]
    fn yaml_import_accepts_explicit_sources_for_final_records() {
        let row = serde_json::json!({
            "name": "Mirrored checkpoint",
            "type": "checkpoint",
            "lane": "sdxl",
            "sha256": HEX_A,
            "size_bytes": 1024_u64,
            "file_format": "safetensors",
            "license": "apache-2.0",
            "status": "linked",
            "sources": {
                "mirrors": [{
                    "kind": "huggingface",
                    "url": "https://huggingface.co/openmayhem/comfy-parts/resolve/abc123/checkpoints/mirrored.safetensors",
                    "repository": "openmayhem/comfy-parts",
                    "path": "checkpoints/mirrored.safetensors",
                    "revision": "abc123"
                }],
                "origins": [{
                    "kind": "civitai",
                    "url": "https://civitai.com/api/download/models/889818",
                    "revision": "889818"
                }],
                "require_auth": true
            }
        });
        let draft = ComfyPartDraft::from_yaml_value(&row).unwrap();
        assert!(draft.sources.require_auth);
        assert_eq!(draft.sources.mirrors.len(), 1);
        assert_eq!(draft.sources.origins.len(), 1);
        assert_eq!(
            draft.sources.mirrors[0].path.as_deref(),
            Some("checkpoints/mirrored.safetensors")
        );
        assert_eq!(draft.sources.origins[0].kind, "civitai");
    }

    #[test]
    fn yaml_import_merges_explicit_adapter_metadata() {
        let row = serde_json::json!({
            "name": "Adapter-rich controlnet",
            "type": "controlnet",
            "lane": "sdxl",
            "role": "legacy role",
            "sha256": HEX_B,
            "size_bytes": 2048_u64,
            "file_format": "safetensors",
            "license": "apache-2.0",
            "status": "linked",
            "download_url": "https://huggingface.co/openmayhem/comfy/resolve/main/controlnet.safetensors",
            "adapter": {
                "role": "conditioning",
                "placement": "phase:condition",
                "resident": false
            }
        });
        let draft = ComfyPartDraft::from_yaml_value(&row).unwrap();
        assert_eq!(
            draft.adapter.get("role"),
            Some(&Value::from("conditioning"))
        );
        assert_eq!(
            draft.adapter.get("placement"),
            Some(&Value::from("phase:condition"))
        );
        assert_eq!(draft.adapter.get("resident"), Some(&Value::from(false)));
    }

    #[test]
    fn parts_anchor_hash_is_validated_and_domain_separated() {
        let anchor = ComfyPartsAnchor {
            schema_version: COMFY_PARTS_ANCHOR_SCHEMA_VERSION,
            parts_index_root: HEX_A.to_owned(),
            index_ver: 11,
            blessed_runtimes: vec!["comfyui-v0.30.1".to_owned()],
            whitelist_ver: 3,
            outcome_classes_ver: 5,
        };
        let hash = comfy_parts_anchor_hash(&anchor).unwrap();
        assert_eq!(hash.len(), 64);

        let mut changed = anchor;
        changed.parts_index_root = HEX_B.to_owned();
        assert_ne!(hash, comfy_parts_anchor_hash(&changed).unwrap());
    }
}
