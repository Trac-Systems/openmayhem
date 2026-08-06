#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

mod comfy_workflow;
mod endpoint_contract;
mod parts_catalog;
mod validated_audio;

pub use comfy_workflow::{
    derive_comfy_workflow, valid_comfy_pricing_unit, ComfyWorkflowCatalogPolicy,
    ComfyWorkflowDerivation, ComfyWorkflowDerivationError, ComfyWorkflowDerivationPolicy,
    ComfyWorkflowOutcomeSpec, ComfyWorkflowPartRef, COMFY_WORKFLOW_DERIVATION_SCHEMA_VERSION,
    DEFAULT_COMFY_WORKFLOW_RUNTIME_ID,
};
pub use endpoint_contract::{
    artifact_generation_inline_audio_load, artifact_generation_input_characters,
    canonicalize_endpoint_request_aliases, endpoint_attribute_value_matches,
    endpoint_contract_fingerprint, endpoint_family_contract_template, endpoint_request_fingerprint,
    generate_endpoint_calibration_cases, materialize_endpoint_calibration_request,
    materialize_endpoint_request_defaults, validate_endpoint_attribute_value,
    validate_endpoint_request, validate_endpoint_response, ArtifactGenerationInlineAudioLoad,
    EndpointCalibrationCase, EndpointCalibrationMutation, EndpointCalibrationValue,
    EndpointContractViolation,
};
pub use parts_catalog::{
    build_comfy_parts_index, comfy_part_record_hash, comfy_parts_anchor_hash, derive_comfy_part_id,
    prove_comfy_part, verify_comfy_part_proof, ComfyPartCanary, ComfyPartCanaryTolerance,
    ComfyPartDraft, ComfyPartLicenseEvidence, ComfyPartMerkleProof, ComfyPartMerkleSibling,
    ComfyPartMerkleSide, ComfyPartRecord, ComfyPartSource, ComfyPartSources, ComfyPartsAnchor,
    ComfyPartsCatalogError, ComfyPartsIndex, ComfyPartsIndexEntry,
    COMFY_PARTS_ANCHOR_SCHEMA_VERSION, COMFY_PARTS_INDEX_SCHEMA_VERSION,
    COMFY_PART_RECORD_SCHEMA_VERSION,
};
pub use validated_audio::{
    validated_audio_metadata, validated_flac_audio_metadata, validated_wav_audio_metadata,
    ValidatedAudioFormat, ValidatedAudioMetadata, MAX_TRANSCRIPTION_AUDIO_SECONDS,
};

pub const CRATE_NAME: &str = "mayhem-proto";
pub const CONTRACT_VERSION: u32 = 20;
pub const ATTESTATION_SCHEMA_VERSION: u32 = 2;
pub const ATTESTATION_ALG: &str = "ed25519";
pub const ATTESTATION_POLICY_SCHEMA_VERSION: u32 = 1;
pub const ATTESTATION_EVIDENCE_BINDING_VERSION: u32 = 1;
pub const TPM_ACTIVATE_CREDENTIAL_SCHEMA_VERSION: u32 = 1;
pub const TPM_ACTIVATE_CREDENTIAL_FRAME_VERSION: u32 = 1;
pub const TPM_ACTIVATE_CREDENTIAL_CHALLENGE_FRAME_TYPE: &str = "tpm.activate.challenge";
pub const TPM_ACTIVATE_CREDENTIAL_RESPONSE_FRAME_TYPE: &str = "tpm.activate.response";
pub const TPM_PCR_POLICY_SCHEMA_VERSION: u32 = 2;
pub const TPM_QUOTE_EVIDENCE_SCHEMA_VERSION: u32 = 1;
pub const SESSION_RECEIPT_SCHEMA_VERSION: u32 = 11;
pub const SIGNING_MESSAGE_VERSION: u32 = 2;
pub const CTX_BRACKET_TABLE_VERSION: u32 = 1;
pub const CTX_BRACKETS: &[(u32, &str)] = &[
    (8_192, "le8k"),
    (32_768, "le32k"),
    (131_072, "le128k"),
    (262_144, "le256k"),
];
pub type MoneyAu = u128;
pub const VISIBLE_OUTPUT_BYTES_PER_UNIT: u64 = 4;
pub const MAX_VISIBLE_OUTPUT_BYTES_PER_REQUEST_TOKEN: u64 = 256;
pub const MAX_VISIBLE_OUTPUT_UNITS_PER_REQUEST_TOKEN: u64 =
    MAX_VISIBLE_OUTPUT_BYTES_PER_REQUEST_TOKEN / VISIBLE_OUTPUT_BYTES_PER_UNIT;

pub mod decimal_u128 {
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u128, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        parse_decimal_u128(&value).map_err(D::Error::custom)
    }

    fn parse_decimal_u128(value: &str) -> Result<u128, String> {
        if value.is_empty() {
            return Err("money amount must be a decimal string".to_owned());
        }
        if value.len() > 1 && value.starts_with('0') {
            return Err("money amount must be canonical without leading zeros".to_owned());
        }
        if !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("money amount must contain only decimal digits".to_owned());
        }
        value
            .parse::<u128>()
            .map_err(|_| "money amount exceeds u128".to_owned())
    }
}

pub mod optional_decimal_u128 {
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &Option<u128>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(value) => serializer.serialize_some(&value.to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<u128>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Option::<String>::deserialize(deserializer)?;
        value
            .map(parse_decimal_u128)
            .transpose()
            .map_err(D::Error::custom)
    }

    fn parse_decimal_u128(value: String) -> Result<u128, String> {
        if value.is_empty() {
            return Err("money amount must be a decimal string".to_owned());
        }
        if value.len() > 1 && value.starts_with('0') {
            return Err("money amount must be canonical without leading zeros".to_owned());
        }
        if !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("money amount must contain only decimal digits".to_owned());
        }
        value
            .parse::<u128>()
            .map_err(|_| "money amount exceeds u128".to_owned())
    }
}

pub const CTX_BRACKET_UNBOUNDED_ID: &str = "gt256k";
pub fn ctx_bracket_for_tokens(tokens: u32) -> &'static str {
    CTX_BRACKETS
        .iter()
        .find_map(|(max_ctx, bracket)| (tokens <= *max_ctx).then_some(*bracket))
        .unwrap_or(CTX_BRACKET_UNBOUNDED_ID)
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CtxBracketEntry {
    pub id: String,
    pub max_ctx: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CtxBracketTableRecord {
    pub ver: u32,
    pub brackets: Vec<CtxBracketEntry>,
    #[serde(default)]
    pub submitted_at: u64,
    #[serde(default)]
    pub effective_at: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CtxBracketSchedule {
    pub current: CtxBracketTableRecord,
    #[serde(default)]
    pub pending: Option<CtxBracketTableRecord>,
}

pub fn default_ctx_bracket_table_record() -> CtxBracketTableRecord {
    let mut brackets = CTX_BRACKETS
        .iter()
        .map(|(max_ctx, id)| CtxBracketEntry {
            id: (*id).to_owned(),
            max_ctx: Some(u64::from(*max_ctx)),
        })
        .collect::<Vec<_>>();
    brackets.push(CtxBracketEntry {
        id: CTX_BRACKET_UNBOUNDED_ID.to_owned(),
        max_ctx: None,
    });
    CtxBracketTableRecord {
        ver: CTX_BRACKET_TABLE_VERSION,
        brackets,
        submitted_at: 0,
        effective_at: 0,
    }
}

pub fn default_ctx_bracket_schedule() -> CtxBracketSchedule {
    CtxBracketSchedule {
        current: default_ctx_bracket_table_record(),
        pending: None,
    }
}

pub fn validate_ctx_bracket_table(record: &CtxBracketTableRecord) -> Result<(), String> {
    if record.ver == 0 {
        return Err("context bracket table version must be positive".to_owned());
    }
    if record.brackets.is_empty() || record.brackets.len() > 32 {
        return Err("context bracket table must contain 1..=32 entries".to_owned());
    }
    let mut previous_max = 0_u64;
    let mut ids = BTreeMap::new();
    for (idx, entry) in record.brackets.iter().enumerate() {
        if entry.id.is_empty()
            || entry.id.len() > 128
            || entry
                .id
                .bytes()
                .any(|byte| !matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-'))
        {
            return Err(format!("invalid context bracket id {}", entry.id));
        }
        if ids.insert(entry.id.as_str(), ()).is_some() {
            return Err(format!("duplicate context bracket id {}", entry.id));
        }
        let is_last = idx + 1 == record.brackets.len();
        match (is_last, entry.max_ctx) {
            (true, None) => {}
            (true, Some(_)) => {
                return Err("last context bracket max_ctx must be null".to_owned());
            }
            (false, None) => {
                return Err("only the last context bracket may have null max_ctx".to_owned());
            }
            (false, Some(max_ctx)) if max_ctx > previous_max => {
                previous_max = max_ctx;
            }
            (false, Some(_)) => {
                return Err("context bracket max_ctx values must increase".to_owned());
            }
        }
    }
    Ok(())
}

pub fn validate_ctx_bracket_schedule(schedule: &CtxBracketSchedule) -> Result<(), String> {
    validate_ctx_bracket_table(&schedule.current)?;
    if let Some(pending) = &schedule.pending {
        validate_ctx_bracket_table(pending)?;
        if pending.ver <= schedule.current.ver {
            return Err("pending context bracket version must advance".to_owned());
        }
    }
    Ok(())
}

pub fn ctx_bracket_table_at(schedule: &CtxBracketSchedule, at: u64) -> &CtxBracketTableRecord {
    schedule
        .pending
        .as_ref()
        .filter(|pending| pending.effective_at <= at)
        .unwrap_or(&schedule.current)
}

pub fn ctx_bracket_for_tokens_in_table(
    tokens: u32,
    table: &CtxBracketTableRecord,
) -> Option<String> {
    let tokens = u64::from(tokens);
    table
        .brackets
        .iter()
        .find(|entry| match entry.max_ctx {
            Some(max_ctx) => tokens <= max_ctx,
            None => true,
        })
        .map(|entry| entry.id.clone())
}

pub fn ctx_bracket_for_tokens_in_schedule(
    tokens: u32,
    schedule: &CtxBracketSchedule,
    at: u64,
) -> Option<(String, u32)> {
    let table = ctx_bracket_table_at(schedule, at);
    ctx_bracket_for_tokens_in_table(tokens, table).map(|bracket| (bracket, table.ver))
}
pub const CATALOG_ENCLAVE_ID_DOMAIN: &str = "mayhem-catalog-enclave-id-v2";
pub const HARDWARE_QUOTE_BINDING_DOMAIN: &str = "mayhem-hardware-quote-binding-v2";
pub const SESSION_ACCEPT_SIGNING_DOMAIN: &str = "mayhem/session-accept/v1";
pub const DEFAULT_MODEL_CLASS: &str = "text-generation";
pub const USAGE_INPUT_TOKEN: &str = "input_token";
pub const USAGE_CACHED_INPUT_TOKEN: &str = "cached_input_token";
pub const USAGE_OUTPUT_TOKEN: &str = "output_token";
pub const USAGE_IMAGE: &str = "image";
pub const USAGE_STEP: &str = "step";
pub const USAGE_INPUT_CHARACTER: &str = "input_character";
pub const USAGE_AUDIO_SECOND: &str = "audio_second";
pub const USAGE_VIDEO_SECOND: &str = "video_second";
pub const USAGE_FRAME: &str = "frame";
pub const USAGE_MEGAPIXEL_STEP: &str = "megapixel_step";
pub const USAGE_MEGAPIXEL: &str = "megapixel";
pub const USAGE_COMPUTE_SECOND: &str = "compute_second";
pub const DEFAULT_VIDEO_GENERATION_FPS: u64 = 24;
pub const ENDPOINT_OPENAI_CHAT_COMPLETIONS: &str = "openai_chat_completions";
pub const ENDPOINT_OPENAI_COMPLETIONS: &str = "openai_completions";
pub const ENDPOINT_OPENAI_RESPONSES: &str = "openai_responses";
pub const ENDPOINT_HF_MULTIMODAL_CHAT: &str = "hf_multimodal_chat";
pub const ENDPOINT_OPENAI_EMBEDDINGS: &str = "openai_embeddings";
pub const ENDPOINT_HF_FEATURE_EXTRACTION: &str = "hf_feature_extraction";
pub const ENDPOINT_OPENAI_IMAGE_GENERATIONS: &str = "openai_image_generations";
pub const ENDPOINT_HF_TEXT_TO_IMAGE: &str = "hf_text_to_image";
pub const ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS: &str = "openai_audio_transcriptions";
pub const ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION: &str = "hf_automatic_speech_recognition";
pub const ENDPOINT_OPENAI_AUDIO_SPEECH: &str = "openai_audio_speech";
pub const ENDPOINT_HF_TEXT_TO_SPEECH: &str = "hf_text_to_speech";
pub const ENDPOINT_OPENAI_VIDEOS: &str = "openai_videos";
pub const ENDPOINT_HF_TEXT_TO_VIDEO: &str = "hf_text_to_video";
pub const ENDPOINT_MAYHEM_AUDIO_GENERATIONS: &str = "mayhem_audio_generations";
pub const ENDPOINT_MAYHEM_MUSIC_GENERATIONS: &str = "mayhem_music_generations";
pub const ENDPOINT_MAYHEM_COMFY_WORKFLOWS: &str = "mayhem_comfy_workflows";
pub const ENDPOINT_HF_TEXT_TO_AUDIO: &str = "hf_text_to_audio";
pub const DEFAULT_SESSION_MAX_FRAME_BYTES: usize = 256 * 1024;
pub const DEFAULT_SESSION_PAYLOAD_CHUNK_BYTES: usize = 16 * 1024;
pub const SESSION_PAYLOAD_CHUNK_SCHEMA_VERSION: u32 = 1;
pub const SESSION_PAYLOAD_CHUNK_ENCODING: &str = "hex";
pub const TIER1_SOFTWARE_ATTESTATION_TIER: u8 = 1;
pub const TIER2_DEVICE_IDENTITY_TIER: u8 = 2;
pub const TIER3_CONFIDENTIAL_COMPUTE_TIER: u8 = 3;
pub const TIER4_PROVIDER_KYB_TIER: u8 = 4;

pub fn video_generation_required_modalities(
    output_modalities: &[String],
    has_conditioning_image: bool,
) -> Vec<String> {
    let mut required = vec!["video".to_owned()];
    if output_modalities.iter().any(|modality| modality == "audio") {
        required.push("audio".to_owned());
    }
    if has_conditioning_image {
        required.push("image".to_owned());
    }
    required
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointValueType {
    String,
    Boolean,
    Integer,
    Number,
    Object,
    Array,
    Null,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EndpointAttributeSpec {
    pub value_types: Vec<EndpointValueType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiple_of: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_items: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calibration_values: Vec<Value>,
}

impl EndpointAttributeSpec {
    #[must_use]
    pub fn new(value_type: EndpointValueType) -> Self {
        Self {
            value_types: vec![value_type],
            default: None,
            enum_values: Vec::new(),
            minimum: None,
            maximum: None,
            multiple_of: None,
            min_length: None,
            max_length: None,
            min_items: None,
            max_items: None,
            calibration_values: Vec::new(),
        }
    }
}

pub fn canonicalize_positive_integer_for_spec(
    requested: u64,
    spec: &EndpointAttributeSpec,
) -> Result<u64, String> {
    let mut allowed = spec
        .enum_values
        .iter()
        .filter_map(|value| {
            value.as_u64().filter(|value| *value > 0).or_else(|| {
                value
                    .as_str()?
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value > 0)
            })
        })
        .collect::<Vec<_>>();
    allowed.sort_unstable();
    allowed.dedup();
    if !allowed.is_empty() {
        return allowed
            .into_iter()
            .min_by_key(|candidate| (candidate.abs_diff(requested), *candidate))
            .ok_or_else(|| "signed integer set is empty".to_owned());
    }

    let minimum = spec.minimum.unwrap_or(1.0).ceil().max(1.0) as u64;
    let maximum = spec.maximum.unwrap_or(u64::MAX as f64).floor().max(1.0) as u64;
    if minimum > maximum {
        return Err("signed integer range has minimum greater than maximum".to_owned());
    }
    let mut actual = requested.clamp(minimum, maximum);
    if let Some(multiple) = spec
        .multiple_of
        .filter(|value| value.is_finite() && *value >= 1.0)
        .map(|value| value.round() as u64)
        .filter(|value| *value > 1)
    {
        let lower = actual / multiple * multiple;
        let upper = lower.saturating_add(multiple);
        actual = [lower, upper]
            .into_iter()
            .filter(|candidate| *candidate >= minimum && *candidate <= maximum)
            .min_by_key(|candidate| (candidate.abs_diff(actual), *candidate))
            .ok_or_else(|| "signed integer range contains no valid multiple".to_owned())?;
    }
    Ok(actual)
}

pub fn canonicalize_positive_integer_at_most_for_spec(
    requested: u64,
    spec: &EndpointAttributeSpec,
) -> Result<u64, String> {
    let mut allowed = spec
        .enum_values
        .iter()
        .filter_map(|value| {
            value.as_u64().filter(|value| *value > 0).or_else(|| {
                value
                    .as_str()?
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value > 0)
            })
        })
        .collect::<Vec<_>>();
    allowed.sort_unstable();
    allowed.dedup();
    if !allowed.is_empty() {
        return allowed
            .into_iter()
            .filter(|candidate| *candidate <= requested)
            .next_back()
            .ok_or_else(|| {
                format!(
                    "signed integer set contains no value at or below requested value {requested}"
                )
            });
    }

    let minimum = spec.minimum.unwrap_or(1.0).ceil().max(1.0) as u64;
    let maximum = spec.maximum.unwrap_or(u64::MAX as f64).floor().max(1.0) as u64;
    if minimum > maximum {
        return Err("signed integer range has minimum greater than maximum".to_owned());
    }
    let upper = requested.min(maximum);
    if upper < minimum {
        return Err(format!(
            "signed integer range contains no value at or below requested value {requested}"
        ));
    }
    let actual = if let Some(multiple) = spec
        .multiple_of
        .filter(|value| value.is_finite() && *value >= 1.0)
        .map(|value| value.round() as u64)
        .filter(|value| *value > 1)
    {
        let candidate = upper / multiple * multiple;
        if candidate < minimum {
            return Err(format!(
                "signed integer range contains no valid multiple at or below requested value {requested}"
            ));
        }
        candidate
    } else {
        upper
    };
    Ok(actual)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EndpointFamilyContract {
    pub family: String,
    pub request_attributes: Vec<String>,
    pub required_request_attributes: Vec<String>,
    pub response_attributes: Vec<String>,
    pub required_response_attributes: Vec<String>,
    pub request_attribute_specs: BTreeMap<String, EndpointAttributeSpec>,
    pub response_attribute_specs: BTreeMap<String, EndpointAttributeSpec>,
    pub interaction_groups: Vec<Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub speciality_mappings: BTreeMap<String, EndpointSpecialityMapping>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointSpecialityTarget {
    ChatTemplateKwarg,
    SamplingParameter,
    PromptSuffix,
    BackendParameter,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointSpecialitySelector {
    #[default]
    Exact,
    NonEmpty,
}

impl EndpointSpecialitySelector {
    #[must_use]
    pub fn is_exact(&self) -> bool {
        *self == Self::Exact
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EndpointSpecialityMapping {
    pub request_path: String,
    pub target: EndpointSpecialityTarget,
    pub native_path: String,
    #[serde(default, skip_serializing_if = "EndpointSpecialitySelector::is_exact")]
    pub selector: EndpointSpecialitySelector,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelSpecialityDescriptor {
    pub name: String,
    pub mechanism: String,
    pub default_level: String,
    pub levels: Vec<ModelSpecialityLevel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calibration_modalities: Vec<String>,
    pub research_evidence: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelSpecialityLevel {
    pub name: String,
    pub rank: u32,
    pub native_value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_reasoning_tokens: Option<u32>,
}

#[must_use]
pub fn endpoint_speciality_effective_native_value(
    mapping: &EndpointSpecialityMapping,
    submitted: Option<&Value>,
) -> Option<Value> {
    match mapping.selector {
        EndpointSpecialitySelector::Exact => submitted.cloned(),
        EndpointSpecialitySelector::NonEmpty => Some(Value::Bool(match submitted {
            None | Some(Value::Null) => false,
            Some(Value::String(value)) => !value.is_empty(),
            Some(Value::Array(value)) => !value.is_empty(),
            Some(Value::Object(value)) => !value.is_empty(),
            Some(_) => true,
        })),
    }
}

#[must_use]
pub fn endpoint_speciality_level_for_request<'a>(
    descriptor: &'a ModelSpecialityDescriptor,
    mapping: &EndpointSpecialityMapping,
    submitted: Option<&Value>,
) -> Option<&'a ModelSpecialityLevel> {
    match endpoint_speciality_effective_native_value(mapping, submitted) {
        Some(submitted) => descriptor.levels.iter().find(|level| {
            submitted.as_str() == Some(level.name.as_str()) || submitted == level.native_value
        }),
        None => descriptor
            .levels
            .iter()
            .find(|level| level.name == descriptor.default_level),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttestationSigner {
    Enclave,
    Provider,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CatalogEnclaveIdentity {
    pub admin_pubkey: String,
    pub model_id: String,
    pub artifact_root: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub artifact_sidecar_roots: BTreeMap<String, String>,
    pub manifest_hash: String,
    pub binary_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttestationBody {
    pub schema_version: u32,
    pub alg: String,
    pub enclave_id: String,
    pub enclave_pubkey: String,
    pub provider_pubkey: String,
    pub manifest_hash: String,
    pub binary_hash: String,
    pub att_tier: u8,
    pub hw_quote: Option<HardwareQuote>,
    pub boot_epoch: u64,
    pub report_ts: u64,
    pub nonce_u: String,
    pub runtime_config: AttestationRuntimeConfig,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttestationRuntimeConfig {
    pub model_class: String,
    pub backend: String,
    pub ctx: u32,
    pub tp_degree: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_batch_size: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_num_tokens: Option<u32>,
}

impl Default for AttestationRuntimeConfig {
    fn default() -> Self {
        Self {
            model_class: DEFAULT_MODEL_CLASS.to_owned(),
            backend: "unknown".to_owned(),
            ctx: 0,
            tp_degree: 1,
            max_batch_size: None,
            max_num_tokens: None,
        }
    }
}

pub fn default_model_class() -> String {
    DEFAULT_MODEL_CLASS.to_owned()
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareQuoteKind {
    AppleAppAttestJwt,
    AmdSevSnpVcek,
    IntelTdxDcap,
    NvidiaGb10DeviceJwt,
    NvidiaNrasJwt,
    NvidiaNvtrustOfflineJwt,
    Tpm2QuoteEk,
}

impl HardwareQuoteKind {
    pub const ALL: [Self; 7] = [
        Self::AppleAppAttestJwt,
        Self::AmdSevSnpVcek,
        Self::IntelTdxDcap,
        Self::NvidiaGb10DeviceJwt,
        Self::NvidiaNrasJwt,
        Self::NvidiaNvtrustOfflineJwt,
        Self::Tpm2QuoteEk,
    ];

    pub fn attestation_tier(&self) -> u8 {
        match self {
            Self::AppleAppAttestJwt | Self::NvidiaGb10DeviceJwt | Self::Tpm2QuoteEk => {
                TIER2_DEVICE_IDENTITY_TIER
            }
            Self::AmdSevSnpVcek
            | Self::IntelTdxDcap
            | Self::NvidiaNrasJwt
            | Self::NvidiaNvtrustOfflineJwt => TIER3_CONFIDENTIAL_COMPUTE_TIER,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AppleAppAttestJwt => "apple_app_attest_jwt",
            Self::AmdSevSnpVcek => "amd_sev_snp_vcek",
            Self::IntelTdxDcap => "intel_tdx_dcap",
            Self::NvidiaGb10DeviceJwt => "nvidia_gb10_device_jwt",
            Self::NvidiaNrasJwt => "nvidia_nras_jwt",
            Self::NvidiaNvtrustOfflineJwt => "nvidia_nvtrust_offline_jwt",
            Self::Tpm2QuoteEk => "tpm2_quote_ek",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestationVerifierProfile {
    AppleAppAttestNativeV1,
    AmdSevSnpVcekV1,
    IntelTdxDcapV1,
    NvidiaGb10DeviceV1,
    NvidiaNrasCompositeV1,
    NvidiaNvtrustOfflineCompositeV1,
    Tpm2EkActivateCredentialV1,
}

impl AttestationVerifierProfile {
    pub const fn quote_kind(self) -> HardwareQuoteKind {
        match self {
            Self::AppleAppAttestNativeV1 => HardwareQuoteKind::AppleAppAttestJwt,
            Self::AmdSevSnpVcekV1 => HardwareQuoteKind::AmdSevSnpVcek,
            Self::IntelTdxDcapV1 => HardwareQuoteKind::IntelTdxDcap,
            Self::NvidiaGb10DeviceV1 => HardwareQuoteKind::NvidiaGb10DeviceJwt,
            Self::NvidiaNrasCompositeV1 => HardwareQuoteKind::NvidiaNrasJwt,
            Self::NvidiaNvtrustOfflineCompositeV1 => HardwareQuoteKind::NvidiaNvtrustOfflineJwt,
            Self::Tpm2EkActivateCredentialV1 => HardwareQuoteKind::Tpm2QuoteEk,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestationMeasurementLayer {
    Cpu,
    Gpu,
    Workload,
}

/// An exact admin approval for one canonical enclave, quote kind, and platform.
///
/// The runtime binary hash is intentionally absent: it is quote evidence, not
/// part of catalog enclave identity or admin approval.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminEnclaveAttestationBinding {
    pub enclave_id: String,
    pub kind: HardwareQuoteKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub measurement_trust_data: BTreeMap<AttestationMeasurementLayer, String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAttestationPolicy {
    pub schema_version: u32,
    pub sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_policy_digest: Option<String>,
    pub issued_epoch: u64,
    pub effective_epoch: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_epoch: Option<u64>,
    pub min_verifier_version: u32,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub emergency_disabled_quote_kinds: BTreeSet<HardwareQuoteKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub origin_pins: Vec<AttestationOriginPin>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trust_data: Vec<AttestationTrustDataRef>,
    pub quote_kinds: Vec<AttestationQuoteKindPolicy>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttestationOriginPin {
    pub id: String,
    pub https_origin: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttestationTrustDataRef {
    pub id: String,
    pub kind: AttestationTrustDataKind,
    pub sha256: String,
    pub media_type: String,
    pub max_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<AttestationTrustDataSource>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestationTrustDataKind {
    TrustAnchor,
    VerificationKey,
    Revocation,
    Measurement,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttestationTrustDataSource {
    pub origin_pin: String,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttestationQuoteKindPolicy {
    pub kind: HardwareQuoteKind,
    pub enabled: bool,
    pub verifier_profile: AttestationVerifierProfile,
    pub evidence_schema_version: u32,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub required_trust_data: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub measurement_trust_data: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub platforms: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub required_measurement_layers: BTreeSet<AttestationMeasurementLayer>,
}

/// Provider-advertised routing data. `declared_platform` is only a hint until
/// it is matched against an active admin policy and verified evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardwareQuoteRouteAdvertisement {
    pub kind: HardwareQuoteKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_platform: Option<String>,
}

/// Buyer-side commitment captured when a route is selected.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardwareQuoteRoutePolicyBinding {
    pub enclave_id: String,
    pub device_id: String,
    pub kind: HardwareQuoteKind,
    pub evidence_schema_version: u32,
    pub policy_sequence: u64,
    pub policy_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TpmEkProfile {
    RsaSha256Aes128Cfb,
    EccP256Sha256Aes128Cfb,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TpmActivateCredentialHello {
    pub schema_version: u32,
    pub ek_profile: TpmEkProfile,
    pub ek_public_spki_der_b64: String,
    pub ak_name_b64: String,
    pub quote_binding: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TpmActivateCredentialChallenge {
    pub schema_version: u32,
    pub challenge_id: String,
    pub ek_public_sha256: String,
    pub ak_name_b64: String,
    pub quote_binding: String,
    pub credential_blob_b64: String,
    pub encrypted_secret_b64: String,
    pub issued_at_unix: u64,
    pub expires_at_unix: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TpmActivateCredentialResponse {
    pub schema_version: u32,
    pub challenge_id: String,
    pub ak_name_b64: String,
    pub quote_binding: String,
    pub activated_secret_b64: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TpmActivateCredentialChallengeFrame {
    #[serde(rename = "t")]
    pub frame_type: String,
    #[serde(rename = "v")]
    pub version: u32,
    pub session_id: String,
    pub provider: String,
    pub enclave_id: String,
    pub room_id: String,
    pub challenge: TpmActivateCredentialChallenge,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TpmActivateCredentialResponseFrame {
    #[serde(rename = "t")]
    pub frame_type: String,
    #[serde(rename = "v")]
    pub version: u32,
    pub session_id: String,
    pub provider: String,
    pub enclave_id: String,
    pub room_id: String,
    pub response: TpmActivateCredentialResponse,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TpmHashAlgorithm {
    Sha256,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TpmPcrPolicy {
    pub schema_version: u32,
    pub hash_algorithm: TpmHashAlgorithm,
    pub pcrs: BTreeSet<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TpmPcrValue {
    pub hash_algorithm: TpmHashAlgorithm,
    pub index: u8,
    pub digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TpmQuoteEvidence {
    pub schema_version: u32,
    pub ak_public_b64: String,
    pub ak_name_b64: String,
    pub quote_attest_b64: String,
    pub quote_signature_b64: String,
    pub pcr_values: Vec<TpmPcrValue>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardwareQuote {
    pub kind: HardwareQuoteKind,
    pub evidence: String,
    pub binding: String,
    #[serde(default)]
    pub endorsements: Vec<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttestationReport {
    pub schema_version: u32,
    pub alg: String,
    pub enclave_id: String,
    pub enclave_pubkey: String,
    pub provider_pubkey: String,
    pub manifest_hash: String,
    pub binary_hash: String,
    pub att_tier: u8,
    pub hw_quote: Option<HardwareQuote>,
    pub boot_epoch: u64,
    pub report_ts: u64,
    pub nonce_u: String,
    pub runtime_config: AttestationRuntimeConfig,
    pub sig_enclave: String,
    pub sig_provider: String,
}

#[derive(Serialize)]
struct AttestationSigningEnvelope<'a> {
    domain: &'static str,
    signer: &'static str,
    body: &'a AttestationBody,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CheckpointPolicy {
    pub tokens: u64,
    pub ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RateMapEntry {
    pub unit: String,
    #[serde(with = "decimal_u128")]
    pub per_unit_au: MoneyAu,
    pub granularity: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkflowBinding {
    pub endpoint_family: String,
    pub graph_hash: String,
    pub runtime_id: String,
    pub outcome_class: String,
    pub quoted_usage: ReceiptUsage,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkflowOutputBinding {
    pub output_modalities: Vec<String>,
    pub metrics: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SpendVoucherBody {
    pub schema_version: u32,
    pub session_id: String,
    pub billing_id: String,
    pub billing_attempt: u32,
    pub billing_prior_usage: ReceiptUsage,
    #[serde(with = "decimal_u128")]
    pub billing_prior_au_owed_cum: MoneyAu,
    pub billing_epoch: u64,
    pub reservation_id: String,
    pub reservation_expires_after_epoch: u64,
    pub reservation_receipt_grace_epochs: u64,
    pub user: String,
    pub provider: String,
    pub payout_revision: String,
    pub rail: String,
    pub enclave_id: String,
    pub model_id: String,
    pub price_ver: u64,
    pub locked_rate_map: Vec<RateMapEntry>,
    #[serde(with = "decimal_u128")]
    pub locked_per_req_au: MoneyAu,
    #[serde(with = "decimal_u128")]
    pub locked_min_session_au: MoneyAu,
    pub served_ctx: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_modalities: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub required_specialities: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<WorkflowBinding>,
    #[serde(default)]
    pub ctx_bracket: Option<String>,
    #[serde(default)]
    pub ctx_bracket_table_ver: Option<u32>,
    pub rules_ver: u64,
    #[serde(with = "decimal_u128")]
    pub max_spend_au: MoneyAu,
    pub checkpoint_every: CheckpointPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SpendVoucher {
    #[serde(flatten)]
    pub body: SpendVoucherBody,
    pub user_sig: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReceiptUsage {
    units: BTreeMap<String, u64>,
}

impl ReceiptUsage {
    pub fn new(units: BTreeMap<String, u64>) -> Self {
        Self::from_units(units)
    }

    pub fn from_units<I, K>(units: I) -> Self
    where
        I: IntoIterator<Item = (K, u64)>,
        K: Into<String>,
    {
        let mut normalized = BTreeMap::new();
        for (unit, count) in units {
            if count == 0 {
                continue;
            }
            let unit = unit.into();
            let unit = canonical_usage_unit(&unit)
                .unwrap_or(unit.as_str())
                .to_owned();
            let next = normalized
                .get(&unit)
                .copied()
                .unwrap_or(0u64)
                .saturating_add(count);
            normalized.insert(unit, next);
        }
        Self { units: normalized }
    }

    pub fn text(in_tokens: u64, out_tokens: u64) -> Self {
        Self::from_units([
            (USAGE_INPUT_TOKEN, in_tokens),
            (USAGE_OUTPUT_TOKEN, out_tokens),
        ])
    }

    pub fn text_with_cached(in_tokens: u64, cached_in_tokens: u64, out_tokens: u64) -> Self {
        Self::from_units([
            (USAGE_INPUT_TOKEN, in_tokens),
            (USAGE_CACHED_INPUT_TOKEN, cached_in_tokens),
            (USAGE_OUTPUT_TOKEN, out_tokens),
        ])
    }

    pub fn units(&self) -> &BTreeMap<String, u64> {
        &self.units
    }

    pub fn get(&self, unit: &str) -> u64 {
        canonical_usage_unit(unit)
            .and_then(|unit| self.units.get(unit).copied())
            .unwrap_or_else(|| self.units.get(unit).copied().unwrap_or(0))
    }

    pub fn input_tokens(&self) -> u64 {
        self.get(USAGE_INPUT_TOKEN)
    }

    pub fn cached_input_tokens(&self) -> u64 {
        self.get(USAGE_CACHED_INPUT_TOKEN)
    }

    pub fn prompt_tokens(&self) -> u64 {
        self.input_tokens()
            .saturating_add(self.cached_input_tokens())
    }

    pub fn output_tokens(&self) -> u64 {
        self.get(USAGE_OUTPUT_TOKEN)
    }

    pub fn saturating_delta(previous: &Self, current: &Self) -> Self {
        let mut units = BTreeMap::new();
        for (unit, count) in current.units() {
            let previous_count = previous.get(unit);
            let delta = count.saturating_sub(previous_count);
            if delta > 0 {
                units.insert(unit.clone(), delta);
            }
        }
        Self { units }
    }

    pub fn saturating_add(&self, increment: &Self) -> Self {
        let mut units = self.units.clone();
        for (unit, count) in increment.units() {
            let next = units.get(unit).copied().unwrap_or(0).saturating_add(*count);
            if next > 0 {
                units.insert(unit.clone(), next);
            }
        }
        Self { units }
    }

    pub fn is_monotonic_from(&self, previous: &Self) -> bool {
        previous
            .units()
            .iter()
            .all(|(unit, previous_count)| self.get(unit) >= *previous_count)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VisibleToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

pub fn metered_output_units(
    content: &str,
    hidden_reasoning: &str,
    tool_calls: &[VisibleToolCall],
) -> u64 {
    let text_bytes = u64::try_from(content.len()).unwrap_or(u64::MAX);
    let hidden_reasoning_bytes = u64::try_from(hidden_reasoning.len()).unwrap_or(u64::MAX);
    let tool_bytes = if tool_calls.is_empty() {
        0
    } else {
        serde_json::to_value(tool_calls)
            .ok()
            .and_then(|value| stable_json_bytes(&value).ok())
            .and_then(|bytes| u64::try_from(bytes.len()).ok())
            .unwrap_or(u64::MAX)
    };
    let bytes = text_bytes
        .saturating_add(hidden_reasoning_bytes)
        .saturating_add(tool_bytes);
    if bytes == 0 {
        0
    } else {
        bytes.div_ceil(VISIBLE_OUTPUT_BYTES_PER_UNIT)
    }
}

pub fn visible_output_units(content: &str, tool_calls: &[VisibleToolCall]) -> u64 {
    metered_output_units(content, "", tool_calls)
}

impl Serialize for ReceiptUsage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.units.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ReceiptUsage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let units = BTreeMap::<String, u64>::deserialize(deserializer)?;
        Ok(Self::new(units))
    }
}

pub fn canonical_usage_unit(unit: &str) -> Option<&'static str> {
    match unit {
        "in" | "in_tokens" | "input" | "input_tokens" | "prompt_tokens" | USAGE_INPUT_TOKEN => {
            Some(USAGE_INPUT_TOKEN)
        }
        "cached_input"
        | "cached_inputs"
        | "cached_input_tokens"
        | "cached_prompt_tokens"
        | "cached_tokens"
        | USAGE_CACHED_INPUT_TOKEN => Some(USAGE_CACHED_INPUT_TOKEN),
        "out" | "out_tokens" | "output" | "output_tokens" | "completion_tokens"
        | USAGE_OUTPUT_TOKEN => Some(USAGE_OUTPUT_TOKEN),
        "images" | USAGE_IMAGE => Some(USAGE_IMAGE),
        "steps" | USAGE_STEP => Some(USAGE_STEP),
        "input_char" | "input_chars" | "input_character" | "input_characters" => {
            Some(USAGE_INPUT_CHARACTER)
        }
        "audio_seconds" | USAGE_AUDIO_SECOND => Some(USAGE_AUDIO_SECOND),
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReceiptBody {
    pub schema_version: u32,
    pub session_id: String,
    pub billing_id: String,
    pub billing_attempt: u32,
    pub billing_prior_usage: ReceiptUsage,
    #[serde(with = "decimal_u128")]
    pub billing_prior_au_owed_cum: MoneyAu,
    pub billing_epoch: u64,
    pub reservation_id: String,
    pub reservation_expires_after_epoch: u64,
    pub reservation_receipt_grace_epochs: u64,
    pub payout_revision: String,
    pub seq: u64,
    #[serde(rename = "final")]
    pub final_receipt: bool,
    pub rail: String,
    pub user: String,
    pub provider: String,
    pub enclave_id: String,
    pub model_id: String,
    pub price_ver: u64,
    pub locked_rate_map: Vec<RateMapEntry>,
    #[serde(with = "decimal_u128")]
    pub locked_per_req_au: MoneyAu,
    #[serde(with = "decimal_u128")]
    pub locked_min_session_au: MoneyAu,
    pub served_ctx: u32,
    #[serde(default)]
    pub ctx_bracket: Option<String>,
    #[serde(default)]
    pub ctx_bracket_table_ver: Option<u32>,
    pub rules_ver: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<WorkflowBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_output: Option<WorkflowOutputBinding>,
    pub usage: ReceiptUsage,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub usage_attribution: BTreeMap<String, u64>,
    #[serde(with = "decimal_u128")]
    pub au_owed_cum: MoneyAu,
    pub prompt_hash: String,
    pub ts: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionReceipt {
    #[serde(flatten)]
    pub body: ReceiptBody,
    pub enclave_sig: String,
    pub enclave_pubkey: String,
    pub user_sig: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordUsageReceiptEnvelope {
    body: ReceiptBody,
    enclave_sig: String,
    enclave_pubkey: String,
    user_sig: String,
}

pub fn record_usage_receipt_envelope(receipt: &SessionReceipt) -> serde_json::Value {
    serde_json::json!({
        "body": &receipt.body,
        "enclave_sig": &receipt.enclave_sig,
        "enclave_pubkey": &receipt.enclave_pubkey,
        "user_sig": &receipt.user_sig,
    })
}

pub fn parse_record_usage_receipt_envelope(
    value: &serde_json::Value,
) -> Result<SessionReceipt, String> {
    let envelope: RecordUsageReceiptEnvelope = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid record usage receipt envelope: {error}"))?;
    let receipt = SessionReceipt {
        body: envelope.body,
        enclave_sig: envelope.enclave_sig,
        enclave_pubkey: envelope.enclave_pubkey,
        user_sig: envelope.user_sig,
    };
    if record_usage_receipt_envelope(&receipt) != *value {
        return Err("record usage receipt envelope is not canonical".to_owned());
    }
    Ok(receipt)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReceiptAck {
    pub session_id: String,
    pub seq: u64,
    pub user_sig: String,
}

pub fn record_usage_receipt_feature_key(receipt: &SessionReceipt) -> String {
    let evidence = serde_json::json!({
        "contract_version": CONTRACT_VERSION,
        "epoch": receipt.body.billing_epoch,
        "payout_revision": receipt.body.payout_revision,
        "receipt": record_usage_receipt_envelope(receipt),
    });
    let key_material = serde_json::json!({
        "domain": "mayhem-record-usage-receipt-feature-v1",
        "evidence": evidence,
    });
    let digest = stable_json_bytes(&key_material)
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
        .expect("serializing a receipt feature key cannot fail");
    format!(
        "receipt/submit/{}/{}/{}/{}/{}",
        receipt.body.billing_epoch,
        receipt.body.billing_id,
        receipt.body.billing_attempt,
        receipt.body.seq,
        digest
    )
}

pub fn record_usage_receipt_signing_bytes(
    _key: &str,
    value: &serde_json::Value,
) -> Result<Vec<u8>, serde_json::Error> {
    let evidence = serde_json::json!({
        "contract_version": value.get("contract_version"),
        "epoch": value.get("epoch"),
        "payout_revision": value.get("payout_revision"),
        "receipt": value.get("receipt"),
    });
    let mut bytes = b"mayhem-record-usage-receipt-v1".to_vec();
    bytes.extend(stable_json_bytes(&evidence)?);
    Ok(bytes)
}

#[derive(Serialize)]
struct SpendVoucherSigningEnvelopeV2<'a> {
    domain: &'static str,
    signing_version: u32,
    body: &'a SpendVoucherBody,
}

#[derive(Serialize)]
struct ReceiptSigningEnvelopeV2<'a> {
    domain: &'static str,
    signing_version: u32,
    body: &'a ReceiptBody,
}

#[derive(Serialize)]
struct SessionAcceptSigningEnvelope<'a> {
    domain: &'static str,
    body: &'a serde_json::Value,
}

impl AttestationReport {
    pub fn body(&self) -> AttestationBody {
        AttestationBody {
            schema_version: self.schema_version,
            alg: self.alg.clone(),
            enclave_id: self.enclave_id.clone(),
            enclave_pubkey: self.enclave_pubkey.clone(),
            provider_pubkey: self.provider_pubkey.clone(),
            manifest_hash: self.manifest_hash.clone(),
            binary_hash: self.binary_hash.clone(),
            att_tier: self.att_tier,
            hw_quote: self.hw_quote.clone(),
            boot_epoch: self.boot_epoch,
            report_ts: self.report_ts,
            nonce_u: self.nonce_u.clone(),
            runtime_config: self.runtime_config.clone(),
        }
    }
}

pub fn catalog_enclave_id(identity: &CatalogEnclaveIdentity) -> String {
    let mut hasher = blake3::Hasher::new_derive_key(CATALOG_ENCLAVE_ID_DOMAIN);
    update_len_prefixed(&mut hasher, identity.admin_pubkey.as_bytes());
    update_len_prefixed(&mut hasher, identity.model_id.as_bytes());
    update_len_prefixed(&mut hasher, identity.artifact_root.as_bytes());
    hasher.update(&(identity.artifact_sidecar_roots.len() as u64).to_be_bytes());
    for (name, root) in &identity.artifact_sidecar_roots {
        update_len_prefixed(&mut hasher, name.as_bytes());
        update_len_prefixed(&mut hasher, root.as_bytes());
    }
    update_len_prefixed(&mut hasher, identity.manifest_hash.as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn update_len_prefixed(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_be_bytes());
    hasher.update(value);
}

pub fn attestation_signing_bytes(
    body: &AttestationBody,
    signer: AttestationSigner,
) -> Result<Vec<u8>, serde_json::Error> {
    let (domain, signer) = match signer {
        AttestationSigner::Enclave => ("mayhem-attestation-report-v1:enclave", "enclave"),
        AttestationSigner::Provider => ("mayhem-attestation-report-v1:provider", "provider"),
    };
    serde_json::to_vec(&AttestationSigningEnvelope {
        domain,
        signer,
        body,
    })
}

pub fn attestation_report_head(report: &AttestationReport) -> Result<String, serde_json::Error> {
    Ok(blake3::hash(&serde_json::to_vec(report)?)
        .to_hex()
        .to_string())
}

pub fn hardware_quote_binding(body: &AttestationBody) -> Result<String, serde_json::Error> {
    let mut bound_body = body.clone();
    bound_body.hw_quote = None;
    Ok(
        blake3::hash(&serde_json::to_vec(&AttestationHardwareQuoteBinding {
            domain: HARDWARE_QUOTE_BINDING_DOMAIN,
            body: &bound_body,
        })?)
        .to_hex()
        .to_string(),
    )
}

#[derive(Serialize)]
struct AttestationHardwareQuoteBinding<'a> {
    domain: &'static str,
    body: &'a AttestationBody,
}

pub fn spend_voucher_signing_bytes(body: &SpendVoucherBody) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&SpendVoucherSigningEnvelopeV2 {
        domain: "mayhem-spend-voucher",
        signing_version: SIGNING_MESSAGE_VERSION,
        body,
    })
}

pub fn receipt_signing_bytes(body: &ReceiptBody) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&ReceiptSigningEnvelopeV2 {
        domain: "mayhem-session-receipt",
        signing_version: SIGNING_MESSAGE_VERSION,
        body,
    })
}

pub fn session_accept_signing_bytes(
    frame: &serde_json::Value,
) -> Result<Vec<u8>, serde_json::Error> {
    let mut unsigned = frame.clone();
    if let Some(object) = unsigned.as_object_mut() {
        object.remove("sig");
    }
    let stable_body = stable_json_value(&unsigned);
    serde_json::to_vec(&SessionAcceptSigningEnvelope {
        domain: SESSION_ACCEPT_SIGNING_DOMAIN,
        body: &stable_body,
    })
}

pub fn session_frame_head(frame: &serde_json::Value) -> Result<String, serde_json::Error> {
    Ok(
        blake3::hash(&serde_json::to_vec(&stable_json_value(frame))?)
            .to_hex()
            .to_string(),
    )
}

pub fn stable_json_bytes(value: &serde_json::Value) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&stable_json_value(value))
}

pub const NORMALIZED_REQUEST_BYTES_PER_PROMPT_UNIT: u64 = 4;

pub fn normalized_request_prompt_units(
    value: &serde_json::Value,
) -> Result<u64, serde_json::Error> {
    let byte_count = u64::try_from(stable_json_bytes(value)?.len()).unwrap_or(u64::MAX);
    Ok(
        byte_count.saturating_add(NORMALIZED_REQUEST_BYTES_PER_PROMPT_UNIT - 1)
            / NORMALIZED_REQUEST_BYTES_PER_PROMPT_UNIT,
    )
}

pub fn tools_only_model_input_prompt_units(
    endpoint_family: &str,
    request: &serde_json::Value,
) -> Result<u64, String> {
    let projection = tools_only_model_input_projection(endpoint_family, request)?;
    normalized_request_prompt_units(&projection)
        .map_err(|error| format!("serializing tools-only model input failed: {error}"))
}

fn tools_only_model_input_projection(
    endpoint_family: &str,
    request: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let query = match endpoint_family {
        ENDPOINT_OPENAI_CHAT_COMPLETIONS => {
            let messages = request
                .get("messages")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| "tools-only chat request is missing messages".to_owned())?;
            tools_only_single_user_query(messages)?
        }
        ENDPOINT_OPENAI_RESPONSES => match request.get("input") {
            Some(serde_json::Value::String(text)) if !text.trim().is_empty() => text.clone(),
            Some(serde_json::Value::Array(messages)) => tools_only_single_user_query(messages)?,
            _ => return Err("tools-only Responses request has no usable input".to_owned()),
        },
        other => {
            return Err(format!(
                "endpoint family {other} has no tools-only model-input projection"
            ))
        }
    };

    let named_choice = request
        .get("tool_choice")
        .and_then(serde_json::Value::as_object)
        .and_then(|choice| {
            choice
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(serde_json::Value::as_str)
                .or_else(|| choice.get("name").and_then(serde_json::Value::as_str))
        });
    if request
        .get("tool_choice")
        .and_then(serde_json::Value::as_str)
        == Some("none")
    {
        return Err("tools-only request cannot disable tools".to_owned());
    }

    let tools = request
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "tools-only request is missing tools".to_owned())?;
    let mut projected_tools = Vec::with_capacity(tools.len());
    for (index, tool) in tools.iter().enumerate() {
        if tool.get("type").and_then(serde_json::Value::as_str) != Some("function") {
            return Err(format!("tools-only request tool {index} is not a function"));
        }
        let function = tool
            .get("function")
            .and_then(serde_json::Value::as_object)
            .or_else(|| tool.as_object().filter(|tool| tool.get("name").is_some()))
            .ok_or_else(|| format!("tools-only request tool {index} has no function"))?;
        let name = function
            .get("name")
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("tools-only request tool {index} has no function name"))?;
        if named_choice.is_some_and(|chosen| chosen != name) {
            continue;
        }
        let description = match function.get("description") {
            None | Some(serde_json::Value::Null) => "",
            Some(serde_json::Value::String(description)) => description,
            Some(_) => {
                return Err(format!(
                    "tools-only request tool {index} has a non-string description"
                ))
            }
        };
        let parameters = match function.get("parameters") {
            None | Some(serde_json::Value::Null) => serde_json::json!({
                "type": "object",
                "properties": {},
            }),
            Some(serde_json::Value::Object(parameters)) => {
                serde_json::Value::Object(parameters.clone())
            }
            Some(_) => {
                return Err(format!(
                    "tools-only request tool {index} has non-object parameters"
                ))
            }
        };
        projected_tools.push(serde_json::json!({
            "name": name,
            "description": description,
            "parameters": parameters,
        }));
    }
    if projected_tools.is_empty() {
        return Err("tools-only request selected no model-visible tools".to_owned());
    }

    Ok(serde_json::json!({
        "query": query,
        "tools": projected_tools,
    }))
}

fn tools_only_single_user_query(messages: &[serde_json::Value]) -> Result<String, String> {
    if messages.len() != 1
        || messages[0].get("role").and_then(serde_json::Value::as_str) != Some("user")
    {
        return Err("tools-only request requires exactly one user message".to_owned());
    }
    let content = messages[0]
        .get("content")
        .ok_or_else(|| "tools-only user message has no content".to_owned())?;
    let query = match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) if !parts.is_empty() => {
            let mut text = Vec::with_capacity(parts.len());
            for (index, part) in parts.iter().enumerate() {
                if part.get("type").and_then(serde_json::Value::as_str) != Some("text") {
                    return Err(format!("tools-only user message part {index} is not text"));
                }
                text.push(
                    part.get("text")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| format!("tools-only user message part {index} has no text"))?
                        .to_owned(),
                );
            }
            text.join("\n")
        }
        _ => return Err("tools-only user message content is not text".to_owned()),
    };
    if query.trim().is_empty() {
        return Err("tools-only request query is empty".to_owned());
    }
    Ok(query)
}

pub const TRANSCRIPTION_RESULT_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_TRANSCRIPTION_RESULT_MAX_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_TRANSCRIPTION_RESULT_MAX_TIMESTAMP_ENTRIES: usize = 1_000_000;
pub const TRANSCRIPTION_RESULT_MAX_LANGUAGE_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptionResult {
    #[serde(rename = "v")]
    pub schema_version: u32,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<TranscriptionTimestamp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<TranscriptionTimestamp>,
}

impl TranscriptionResult {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            schema_version: TRANSCRIPTION_RESULT_SCHEMA_VERSION,
            text: text.into(),
            detected_language: None,
            duration_seconds: None,
            words: Vec::new(),
            segments: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptionTimestamp {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TranscriptionResultLimits {
    pub max_bytes: usize,
    pub max_timestamp_entries: usize,
}

impl Default for TranscriptionResultLimits {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_TRANSCRIPTION_RESULT_MAX_BYTES,
            max_timestamp_entries: DEFAULT_TRANSCRIPTION_RESULT_MAX_TIMESTAMP_ENTRIES,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TranscriptionResultError {
    UnsupportedSchemaVersion(u32),
    InvalidText,
    InvalidLanguage,
    TooManyTimestampEntries {
        max: u64,
        got: u64,
    },
    InvalidTimestamp {
        kind: &'static str,
        index: u64,
        reason: &'static str,
    },
    PayloadTooLarge {
        max: u64,
        got: u64,
    },
    LengthOverflow,
    Json(String),
    Payload(PayloadChunkError),
}

impl fmt::Display for TranscriptionResultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchemaVersion(version) => {
                write!(f, "unsupported transcription result schema version {version}")
            }
            Self::InvalidText => write!(f, "transcription result text must not be empty"),
            Self::InvalidLanguage => write!(
                f,
                "transcription result language must contain 1..={TRANSCRIPTION_RESULT_MAX_LANGUAGE_BYTES} bytes"
            ),
            Self::TooManyTimestampEntries { max, got } => write!(
                f,
                "transcription result has {got} timestamp entries, exceeding the {max}-entry limit"
            ),
            Self::InvalidTimestamp {
                kind,
                index,
                reason,
            } => write!(
                f,
                "transcription result {kind} timestamp {index} is invalid: {reason}"
            ),
            Self::PayloadTooLarge { max, got } => write!(
                f,
                "transcription result is {got} bytes, exceeding the {max}-byte limit"
            ),
            Self::LengthOverflow => write!(f, "transcription result length overflowed"),
            Self::Json(message) => write!(f, "transcription result JSON error: {message}"),
            Self::Payload(error) => write!(f, "transcription result payload error: {error}"),
        }
    }
}

impl std::error::Error for TranscriptionResultError {}

impl From<PayloadChunkError> for TranscriptionResultError {
    fn from(error: PayloadChunkError) -> Self {
        Self::Payload(error)
    }
}

pub fn validate_transcription_result(
    result: &TranscriptionResult,
    limits: TranscriptionResultLimits,
) -> Result<(), TranscriptionResultError> {
    if result.schema_version != TRANSCRIPTION_RESULT_SCHEMA_VERSION {
        return Err(TranscriptionResultError::UnsupportedSchemaVersion(
            result.schema_version,
        ));
    }
    if result.text.trim().is_empty() {
        return Err(TranscriptionResultError::InvalidText);
    }
    if result.detected_language.as_ref().is_some_and(|language| {
        language.trim().is_empty() || language.len() > TRANSCRIPTION_RESULT_MAX_LANGUAGE_BYTES
    }) {
        return Err(TranscriptionResultError::InvalidLanguage);
    }
    if result
        .duration_seconds
        .is_some_and(|duration| !duration.is_finite() || duration <= 0.0)
    {
        return Err(TranscriptionResultError::InvalidTimestamp {
            kind: "duration",
            index: 0,
            reason: "duration must be finite and positive",
        });
    }
    let timestamp_entries = result
        .words
        .len()
        .checked_add(result.segments.len())
        .ok_or(TranscriptionResultError::LengthOverflow)?;
    if timestamp_entries > limits.max_timestamp_entries {
        return Err(TranscriptionResultError::TooManyTimestampEntries {
            max: u64::try_from(limits.max_timestamp_entries)
                .map_err(|_| TranscriptionResultError::LengthOverflow)?,
            got: u64::try_from(timestamp_entries)
                .map_err(|_| TranscriptionResultError::LengthOverflow)?,
        });
    }
    validate_transcription_timestamps("word", &result.words, result.duration_seconds)?;
    validate_transcription_timestamps("segment", &result.segments, result.duration_seconds)?;
    validate_transcription_timestamp_structure(&result.words, &result.segments)?;
    let bytes = serde_json::to_vec(result)
        .map_err(|error| TranscriptionResultError::Json(error.to_string()))?;
    if bytes.len() > limits.max_bytes {
        return Err(TranscriptionResultError::PayloadTooLarge {
            max: u64::try_from(limits.max_bytes)
                .map_err(|_| TranscriptionResultError::LengthOverflow)?,
            got: u64::try_from(bytes.len())
                .map_err(|_| TranscriptionResultError::LengthOverflow)?,
        });
    }
    Ok(())
}

fn validate_transcription_timestamps(
    kind: &'static str,
    entries: &[TranscriptionTimestamp],
    duration_seconds: Option<f64>,
) -> Result<(), TranscriptionResultError> {
    let mut previous_end = None;
    for (index, entry) in entries.iter().enumerate() {
        let index = u64::try_from(index).map_err(|_| TranscriptionResultError::LengthOverflow)?;
        let reason = if entry.text.trim().is_empty() {
            Some("text must not be empty")
        } else if !entry.start.is_finite() || !entry.end.is_finite() {
            Some("start and end must be finite")
        } else if entry.start < 0.0 || entry.end < 0.0 {
            Some("start and end must be non-negative")
        } else if entry.end <= entry.start {
            Some("end must be greater than start")
        } else if previous_end.is_some_and(|previous_end| entry.start < previous_end) {
            Some("entries must be ordered and non-overlapping")
        } else if duration_seconds.is_some_and(|duration| entry.end > duration) {
            Some("end must not exceed duration")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(TranscriptionResultError::InvalidTimestamp {
                kind,
                index,
                reason,
            });
        }
        previous_end = Some(entry.end);
    }
    Ok(())
}

fn validate_transcription_timestamp_structure(
    words: &[TranscriptionTimestamp],
    segments: &[TranscriptionTimestamp],
) -> Result<(), TranscriptionResultError> {
    if words.is_empty() || segments.is_empty() {
        return Ok(());
    }

    let mut segment_index = 0;
    let mut segment_has_word = false;
    for (word_index, word) in words.iter().enumerate() {
        while segment_index < segments.len() && word.start >= segments[segment_index].end {
            if !segment_has_word {
                return Err(TranscriptionResultError::InvalidTimestamp {
                    kind: "segment",
                    index: u64::try_from(segment_index)
                        .map_err(|_| TranscriptionResultError::LengthOverflow)?,
                    reason: "segment must contain at least one word",
                });
            }
            segment_index += 1;
            segment_has_word = false;
        }

        let Some(segment) = segments.get(segment_index) else {
            return Err(TranscriptionResultError::InvalidTimestamp {
                kind: "word",
                index: u64::try_from(word_index)
                    .map_err(|_| TranscriptionResultError::LengthOverflow)?,
                reason: "word must be contained within a segment",
            });
        };
        if word.start < segment.start || word.end > segment.end {
            return Err(TranscriptionResultError::InvalidTimestamp {
                kind: "word",
                index: u64::try_from(word_index)
                    .map_err(|_| TranscriptionResultError::LengthOverflow)?,
                reason: "word must be contained within a segment",
            });
        }
        segment_has_word = true;
    }

    if !segment_has_word {
        return Err(TranscriptionResultError::InvalidTimestamp {
            kind: "segment",
            index: u64::try_from(segment_index)
                .map_err(|_| TranscriptionResultError::LengthOverflow)?,
            reason: "segment must contain at least one word",
        });
    }
    segment_index += 1;
    if segment_index < segments.len() {
        return Err(TranscriptionResultError::InvalidTimestamp {
            kind: "segment",
            index: u64::try_from(segment_index)
                .map_err(|_| TranscriptionResultError::LengthOverflow)?,
            reason: "segment must contain at least one word",
        });
    }

    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PayloadChunkManifest {
    #[serde(rename = "v")]
    pub schema_version: u32,
    pub encoding: String,
    pub total_len: u64,
    pub chunk_size: u64,
    pub chunk_count: u64,
    pub blake3: String,
    pub chunks: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PayloadChunk {
    pub i: u64,
    pub offset: u64,
    pub len: u64,
    pub blake3: String,
    pub encoding: String,
    pub data: String,
    #[serde(rename = "final")]
    pub final_chunk: bool,
}

pub const DEFAULT_SESSION_MAX_REASSEMBLED_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_SESSION_MAX_PAYLOAD_CHUNKS: usize = 65_536;

#[derive(Clone, Debug)]
pub struct PayloadChunkCollector {
    max_total_len: usize,
    max_chunks: usize,
    bytes: Vec<u8>,
    chunk_hashes: Vec<String>,
    chunk_lengths: Vec<u64>,
    final_seen: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PayloadChunkError {
    EmptyChunkSize,
    UnsupportedSchemaVersion(u32),
    UnsupportedEncoding(String),
    LengthOverflow,
    InvalidDigest(String),
    InvalidHex(String),
    MissingChunk {
        index: u64,
    },
    DuplicateChunk {
        index: u64,
    },
    ReorderedChunk {
        expected: u64,
        got: u64,
    },
    ChunkHashMismatch {
        index: u64,
        expected: String,
        got: String,
    },
    RootHashMismatch {
        expected: String,
        got: String,
    },
    OffsetMismatch {
        index: u64,
        expected: u64,
        got: u64,
    },
    LenMismatch {
        index: u64,
        expected: u64,
        got: u64,
    },
    FinalFlagMismatch {
        index: u64,
    },
    TotalLenMismatch {
        expected: u64,
        got: u64,
    },
    ChunkCountMismatch {
        expected: u64,
        got: u64,
    },
    PayloadTooLarge {
        max: u64,
        got: u64,
    },
    TooManyChunks {
        max: u64,
        got: u64,
    },
    ChunkAfterFinal {
        index: u64,
    },
    Json(String),
}

impl fmt::Display for PayloadChunkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyChunkSize => write!(f, "payload chunk size must be greater than zero"),
            Self::UnsupportedSchemaVersion(version) => {
                write!(f, "unsupported payload chunk schema version {version}")
            }
            Self::UnsupportedEncoding(encoding) => {
                write!(f, "unsupported payload chunk encoding {encoding}")
            }
            Self::LengthOverflow => write!(f, "payload length overflowed target integer"),
            Self::InvalidDigest(label) => write!(f, "{label} must be a 32-byte hex digest"),
            Self::InvalidHex(label) => write!(f, "{label} is not valid hex"),
            Self::MissingChunk { index } => write!(f, "payload chunk {index} is missing"),
            Self::DuplicateChunk { index } => write!(f, "payload chunk {index} is duplicated"),
            Self::ReorderedChunk { expected, got } => {
                write!(
                    f,
                    "payload chunks are reordered: expected {expected}, got {got}"
                )
            }
            Self::ChunkHashMismatch {
                index,
                expected,
                got,
            } => write!(
                f,
                "payload chunk {index} blake3 mismatch: expected {expected}, got {got}"
            ),
            Self::RootHashMismatch { expected, got } => {
                write!(f, "payload blake3 mismatch: expected {expected}, got {got}")
            }
            Self::OffsetMismatch {
                index,
                expected,
                got,
            } => write!(
                f,
                "payload chunk {index} offset mismatch: expected {expected}, got {got}"
            ),
            Self::LenMismatch {
                index,
                expected,
                got,
            } => write!(
                f,
                "payload chunk {index} length mismatch: expected {expected}, got {got}"
            ),
            Self::FinalFlagMismatch { index } => {
                write!(f, "payload chunk {index} final flag mismatch")
            }
            Self::TotalLenMismatch { expected, got } => {
                write!(
                    f,
                    "payload total length mismatch: expected {expected}, got {got}"
                )
            }
            Self::ChunkCountMismatch { expected, got } => {
                write!(
                    f,
                    "payload chunk count mismatch: expected {expected}, got {got}"
                )
            }
            Self::PayloadTooLarge { max, got } => {
                write!(f, "payload length {got} exceeds the {max}-byte limit")
            }
            Self::TooManyChunks { max, got } => {
                write!(f, "payload chunk count {got} exceeds the {max}-chunk limit")
            }
            Self::ChunkAfterFinal { index } => {
                write!(f, "payload chunk {index} arrived after the final chunk")
            }
            Self::Json(message) => write!(f, "payload JSON error: {message}"),
        }
    }
}

impl std::error::Error for PayloadChunkError {}

impl PayloadChunkCollector {
    #[must_use]
    pub fn new(max_total_len: usize, max_chunks: usize) -> Self {
        Self {
            max_total_len,
            max_chunks,
            bytes: Vec::new(),
            chunk_hashes: Vec::new(),
            chunk_lengths: Vec::new(),
            final_seen: false,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.chunk_hashes.len()
    }

    pub fn push(&mut self, chunk: PayloadChunk) -> Result<(), PayloadChunkError> {
        validate_payload_chunk(&chunk)?;
        if self.final_seen {
            return Err(PayloadChunkError::ChunkAfterFinal { index: chunk.i });
        }
        let next_count = self
            .chunk_hashes
            .len()
            .checked_add(1)
            .ok_or(PayloadChunkError::LengthOverflow)?;
        if next_count > self.max_chunks {
            return Err(PayloadChunkError::TooManyChunks {
                max: u64::try_from(self.max_chunks)
                    .map_err(|_| PayloadChunkError::LengthOverflow)?,
                got: u64::try_from(next_count).map_err(|_| PayloadChunkError::LengthOverflow)?,
            });
        }
        let expected_index = u64::try_from(self.chunk_hashes.len())
            .map_err(|_| PayloadChunkError::LengthOverflow)?;
        if chunk.i != expected_index {
            return Err(PayloadChunkError::ReorderedChunk {
                expected: expected_index,
                got: chunk.i,
            });
        }
        let expected_offset =
            u64::try_from(self.bytes.len()).map_err(|_| PayloadChunkError::LengthOverflow)?;
        if chunk.offset != expected_offset {
            return Err(PayloadChunkError::OffsetMismatch {
                index: chunk.i,
                expected: expected_offset,
                got: chunk.offset,
            });
        }
        let declared_len =
            usize::try_from(chunk.len).map_err(|_| PayloadChunkError::LengthOverflow)?;
        let expected_hex_len = declared_len
            .checked_mul(2)
            .ok_or(PayloadChunkError::LengthOverflow)?;
        if chunk.data.len() != expected_hex_len {
            return Err(PayloadChunkError::LenMismatch {
                index: chunk.i,
                expected: chunk.len,
                got: u64::try_from(chunk.data.len() / 2)
                    .map_err(|_| PayloadChunkError::LengthOverflow)?,
            });
        }
        let next_len = self
            .bytes
            .len()
            .checked_add(declared_len)
            .ok_or(PayloadChunkError::LengthOverflow)?;
        if next_len > self.max_total_len {
            return Err(PayloadChunkError::PayloadTooLarge {
                max: u64::try_from(self.max_total_len)
                    .map_err(|_| PayloadChunkError::LengthOverflow)?,
                got: u64::try_from(next_len).map_err(|_| PayloadChunkError::LengthOverflow)?,
            });
        }
        let decoded = hex_decode(&chunk.data, "payload chunk data")?;
        if decoded.len() != declared_len {
            return Err(PayloadChunkError::LenMismatch {
                index: chunk.i,
                expected: chunk.len,
                got: u64::try_from(decoded.len()).map_err(|_| PayloadChunkError::LengthOverflow)?,
            });
        }
        let actual_hash = blake3_hex(&decoded);
        if actual_hash != chunk.blake3 {
            return Err(PayloadChunkError::ChunkHashMismatch {
                index: chunk.i,
                expected: chunk.blake3,
                got: actual_hash,
            });
        }
        self.bytes.extend_from_slice(&decoded);
        self.chunk_hashes.push(actual_hash);
        self.chunk_lengths.push(chunk.len);
        self.final_seen = chunk.final_chunk;
        Ok(())
    }

    pub fn finish_bytes(
        self,
        manifest: &PayloadChunkManifest,
    ) -> Result<Vec<u8>, PayloadChunkError> {
        validate_payload_manifest(manifest)?;
        let total_len =
            usize::try_from(manifest.total_len).map_err(|_| PayloadChunkError::LengthOverflow)?;
        if total_len > self.max_total_len {
            return Err(PayloadChunkError::PayloadTooLarge {
                max: u64::try_from(self.max_total_len)
                    .map_err(|_| PayloadChunkError::LengthOverflow)?,
                got: manifest.total_len,
            });
        }
        let chunk_count =
            usize::try_from(manifest.chunk_count).map_err(|_| PayloadChunkError::LengthOverflow)?;
        if chunk_count > self.max_chunks {
            return Err(PayloadChunkError::TooManyChunks {
                max: u64::try_from(self.max_chunks)
                    .map_err(|_| PayloadChunkError::LengthOverflow)?,
                got: manifest.chunk_count,
            });
        }
        let got_count = u64::try_from(self.chunk_hashes.len())
            .map_err(|_| PayloadChunkError::LengthOverflow)?;
        if got_count != manifest.chunk_count {
            return Err(PayloadChunkError::ChunkCountMismatch {
                expected: manifest.chunk_count,
                got: got_count,
            });
        }
        if manifest.chunk_count > 0 && !self.final_seen {
            return Err(PayloadChunkError::FinalFlagMismatch {
                index: manifest.chunk_count.saturating_sub(1),
            });
        }
        for (index, (actual, expected)) in self
            .chunk_hashes
            .iter()
            .zip(manifest.chunks.iter())
            .enumerate()
        {
            if actual != expected {
                return Err(PayloadChunkError::ChunkHashMismatch {
                    index: u64::try_from(index).map_err(|_| PayloadChunkError::LengthOverflow)?,
                    expected: expected.clone(),
                    got: actual.clone(),
                });
            }
        }
        for (index, length) in self.chunk_lengths.iter().enumerate() {
            let last = index + 1 == self.chunk_lengths.len();
            if (!last && *length != manifest.chunk_size) || (last && *length > manifest.chunk_size)
            {
                return Err(PayloadChunkError::LenMismatch {
                    index: u64::try_from(index).map_err(|_| PayloadChunkError::LengthOverflow)?,
                    expected: manifest.chunk_size,
                    got: *length,
                });
            }
        }
        let got_len =
            u64::try_from(self.bytes.len()).map_err(|_| PayloadChunkError::LengthOverflow)?;
        if got_len != manifest.total_len {
            return Err(PayloadChunkError::TotalLenMismatch {
                expected: manifest.total_len,
                got: got_len,
            });
        }
        let actual_root = blake3_hex(&self.bytes);
        if actual_root != manifest.blake3 {
            return Err(PayloadChunkError::RootHashMismatch {
                expected: manifest.blake3.clone(),
                got: actual_root,
            });
        }
        Ok(self.bytes)
    }

    pub fn finish_json(
        self,
        manifest: &PayloadChunkManifest,
    ) -> Result<serde_json::Value, PayloadChunkError> {
        let bytes = self.finish_bytes(manifest)?;
        serde_json::from_slice(&bytes).map_err(|err| PayloadChunkError::Json(err.to_string()))
    }
}

pub fn chunk_json_payload(
    value: &serde_json::Value,
    chunk_size: usize,
) -> Result<(PayloadChunkManifest, Vec<PayloadChunk>), PayloadChunkError> {
    let bytes = stable_json_bytes(value).map_err(|err| PayloadChunkError::Json(err.to_string()))?;
    chunk_payload_bytes(&bytes, chunk_size)
}

pub fn reassemble_json_payload(
    manifest: &PayloadChunkManifest,
    chunks: &[PayloadChunk],
) -> Result<serde_json::Value, PayloadChunkError> {
    let bytes = reassemble_payload_chunks(manifest, chunks)?;
    serde_json::from_slice(&bytes).map_err(|err| PayloadChunkError::Json(err.to_string()))
}

pub fn chunk_transcription_result(
    result: &TranscriptionResult,
    chunk_size: usize,
    limits: TranscriptionResultLimits,
) -> Result<(PayloadChunkManifest, Vec<PayloadChunk>), TranscriptionResultError> {
    validate_transcription_result(result, limits)?;
    let value = serde_json::to_value(result)
        .map_err(|error| TranscriptionResultError::Json(error.to_string()))?;
    chunk_json_payload(&value, chunk_size).map_err(TranscriptionResultError::from)
}

pub fn reassemble_transcription_result(
    manifest: &PayloadChunkManifest,
    chunks: &[PayloadChunk],
    limits: TranscriptionResultLimits,
) -> Result<TranscriptionResult, TranscriptionResultError> {
    let value = reassemble_json_payload(manifest, chunks)?;
    let result = serde_json::from_value(value)
        .map_err(|error| TranscriptionResultError::Json(error.to_string()))?;
    validate_transcription_result(&result, limits)?;
    Ok(result)
}

pub fn chunk_payload_bytes(
    bytes: &[u8],
    chunk_size: usize,
) -> Result<(PayloadChunkManifest, Vec<PayloadChunk>), PayloadChunkError> {
    let manifest = payload_chunk_manifest(bytes, chunk_size)?;
    let mut chunks = Vec::with_capacity(
        usize::try_from(manifest.chunk_count).map_err(|_| PayloadChunkError::LengthOverflow)?,
    );
    for index in 0..manifest.chunk_count {
        chunks.push(
            payload_chunk_at(bytes, chunk_size, index)?
                .ok_or(PayloadChunkError::MissingChunk { index })?,
        );
    }
    Ok((manifest, chunks))
}

pub fn payload_chunk_manifest(
    bytes: &[u8],
    chunk_size: usize,
) -> Result<PayloadChunkManifest, PayloadChunkError> {
    if chunk_size == 0 {
        return Err(PayloadChunkError::EmptyChunkSize);
    }
    let total_len = u64::try_from(bytes.len()).map_err(|_| PayloadChunkError::LengthOverflow)?;
    let chunk_size_u64 =
        u64::try_from(chunk_size).map_err(|_| PayloadChunkError::LengthOverflow)?;
    let root = blake3_hex(bytes);
    let chunk_hashes = bytes.chunks(chunk_size).map(blake3_hex).collect::<Vec<_>>();
    Ok(PayloadChunkManifest {
        schema_version: SESSION_PAYLOAD_CHUNK_SCHEMA_VERSION,
        encoding: SESSION_PAYLOAD_CHUNK_ENCODING.to_owned(),
        total_len,
        chunk_size: chunk_size_u64,
        chunk_count: u64::try_from(chunk_hashes.len())
            .map_err(|_| PayloadChunkError::LengthOverflow)?,
        blake3: root,
        chunks: chunk_hashes,
    })
}

pub fn payload_chunk_at(
    bytes: &[u8],
    chunk_size: usize,
    index: u64,
) -> Result<Option<PayloadChunk>, PayloadChunkError> {
    if chunk_size == 0 {
        return Err(PayloadChunkError::EmptyChunkSize);
    }
    let index_usize = usize::try_from(index).map_err(|_| PayloadChunkError::LengthOverflow)?;
    let offset = index_usize
        .checked_mul(chunk_size)
        .ok_or(PayloadChunkError::LengthOverflow)?;
    if offset >= bytes.len() {
        return Ok(None);
    }
    let end = offset.saturating_add(chunk_size).min(bytes.len());
    let chunk = &bytes[offset..end];
    Ok(Some(PayloadChunk {
        i: index,
        offset: u64::try_from(offset).map_err(|_| PayloadChunkError::LengthOverflow)?,
        len: u64::try_from(chunk.len()).map_err(|_| PayloadChunkError::LengthOverflow)?,
        blake3: blake3_hex(chunk),
        encoding: SESSION_PAYLOAD_CHUNK_ENCODING.to_owned(),
        data: hex_encode(chunk),
        final_chunk: end == bytes.len(),
    }))
}

pub fn reassemble_payload_chunks(
    manifest: &PayloadChunkManifest,
    chunks: &[PayloadChunk],
) -> Result<Vec<u8>, PayloadChunkError> {
    validate_payload_manifest(manifest)?;
    let got_count = u64::try_from(chunks.len()).map_err(|_| PayloadChunkError::LengthOverflow)?;
    if got_count != manifest.chunk_count {
        return Err(PayloadChunkError::ChunkCountMismatch {
            expected: manifest.chunk_count,
            got: got_count,
        });
    }
    if manifest.chunk_count == 0 {
        if manifest.total_len != 0 {
            return Err(PayloadChunkError::TotalLenMismatch {
                expected: manifest.total_len,
                got: 0,
            });
        }
        let actual = blake3_hex(&[]);
        if actual != manifest.blake3 {
            return Err(PayloadChunkError::RootHashMismatch {
                expected: manifest.blake3.clone(),
                got: actual,
            });
        }
        return Ok(Vec::new());
    }

    let mut seen = vec![false; chunks.len()];
    let mut bytes = Vec::with_capacity(
        usize::try_from(manifest.total_len).map_err(|_| PayloadChunkError::LengthOverflow)?,
    );
    for (expected_index, chunk) in chunks.iter().enumerate() {
        let expected_index_u64 =
            u64::try_from(expected_index).map_err(|_| PayloadChunkError::LengthOverflow)?;
        validate_payload_chunk(chunk)?;
        if chunk.i != expected_index_u64 {
            return Err(PayloadChunkError::ReorderedChunk {
                expected: expected_index_u64,
                got: chunk.i,
            });
        }
        let seen_index = usize::try_from(chunk.i).map_err(|_| PayloadChunkError::LengthOverflow)?;
        if seen_index >= seen.len() {
            return Err(PayloadChunkError::MissingChunk { index: chunk.i });
        }
        if seen[seen_index] {
            return Err(PayloadChunkError::DuplicateChunk { index: chunk.i });
        }
        seen[seen_index] = true;
        let expected_hash =
            manifest
                .chunks
                .get(expected_index)
                .ok_or(PayloadChunkError::MissingChunk {
                    index: expected_index_u64,
                })?;
        if &chunk.blake3 != expected_hash {
            return Err(PayloadChunkError::ChunkHashMismatch {
                index: chunk.i,
                expected: expected_hash.clone(),
                got: chunk.blake3.clone(),
            });
        }
        let expected_offset =
            u64::try_from(bytes.len()).map_err(|_| PayloadChunkError::LengthOverflow)?;
        if chunk.offset != expected_offset {
            return Err(PayloadChunkError::OffsetMismatch {
                index: chunk.i,
                expected: expected_offset,
                got: chunk.offset,
            });
        }
        let decoded = hex_decode(&chunk.data, "payload chunk data")?;
        let decoded_len =
            u64::try_from(decoded.len()).map_err(|_| PayloadChunkError::LengthOverflow)?;
        if decoded_len != chunk.len {
            return Err(PayloadChunkError::LenMismatch {
                index: chunk.i,
                expected: chunk.len,
                got: decoded_len,
            });
        }
        let actual_hash = blake3_hex(&decoded);
        if actual_hash != chunk.blake3 {
            return Err(PayloadChunkError::ChunkHashMismatch {
                index: chunk.i,
                expected: chunk.blake3.clone(),
                got: actual_hash,
            });
        }
        let should_be_final = expected_index + 1 == chunks.len();
        if chunk.final_chunk != should_be_final {
            return Err(PayloadChunkError::FinalFlagMismatch { index: chunk.i });
        }
        bytes.extend_from_slice(&decoded);
    }
    if let Some((index, _)) = seen.iter().enumerate().find(|(_, seen)| !**seen) {
        return Err(PayloadChunkError::MissingChunk {
            index: u64::try_from(index).map_err(|_| PayloadChunkError::LengthOverflow)?,
        });
    }
    let got_len = u64::try_from(bytes.len()).map_err(|_| PayloadChunkError::LengthOverflow)?;
    if got_len != manifest.total_len {
        return Err(PayloadChunkError::TotalLenMismatch {
            expected: manifest.total_len,
            got: got_len,
        });
    }
    let actual_root = blake3_hex(&bytes);
    if actual_root != manifest.blake3 {
        return Err(PayloadChunkError::RootHashMismatch {
            expected: manifest.blake3.clone(),
            got: actual_root,
        });
    }
    Ok(bytes)
}

fn validate_payload_manifest(manifest: &PayloadChunkManifest) -> Result<(), PayloadChunkError> {
    if manifest.schema_version != SESSION_PAYLOAD_CHUNK_SCHEMA_VERSION {
        return Err(PayloadChunkError::UnsupportedSchemaVersion(
            manifest.schema_version,
        ));
    }
    if manifest.encoding != SESSION_PAYLOAD_CHUNK_ENCODING {
        return Err(PayloadChunkError::UnsupportedEncoding(
            manifest.encoding.clone(),
        ));
    }
    if manifest.chunk_size == 0 && manifest.chunk_count > 0 {
        return Err(PayloadChunkError::EmptyChunkSize);
    }
    if !is_hex_len(&manifest.blake3, 64) {
        return Err(PayloadChunkError::InvalidDigest(
            "payload blake3".to_owned(),
        ));
    }
    let declared_count =
        u64::try_from(manifest.chunks.len()).map_err(|_| PayloadChunkError::LengthOverflow)?;
    if declared_count != manifest.chunk_count {
        return Err(PayloadChunkError::ChunkCountMismatch {
            expected: manifest.chunk_count,
            got: declared_count,
        });
    }
    for (index, hash) in manifest.chunks.iter().enumerate() {
        if !is_hex_len(hash, 64) {
            return Err(PayloadChunkError::InvalidDigest(format!(
                "payload chunk {index} blake3"
            )));
        }
    }
    Ok(())
}

fn validate_payload_chunk(chunk: &PayloadChunk) -> Result<(), PayloadChunkError> {
    if chunk.encoding != SESSION_PAYLOAD_CHUNK_ENCODING {
        return Err(PayloadChunkError::UnsupportedEncoding(
            chunk.encoding.clone(),
        ));
    }
    if !is_hex_len(&chunk.blake3, 64) {
        return Err(PayloadChunkError::InvalidDigest(format!(
            "payload chunk {} blake3",
            chunk.i
        )));
    }
    Ok(())
}

fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(value: &str, label: &str) -> Result<Vec<u8>, PayloadChunkError> {
    if value.len() % 2 != 0 {
        return Err(PayloadChunkError::InvalidHex(label.to_owned()));
    }
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for index in (0..bytes.len()).step_by(2) {
        let high = hex_nibble(bytes[index])
            .ok_or_else(|| PayloadChunkError::InvalidHex(label.to_owned()))?;
        let low = hex_nibble(bytes[index + 1])
            .ok_or_else(|| PayloadChunkError::InvalidHex(label.to_owned()))?;
        out.push((high << 4) | low);
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_hex_len(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn stable_json_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(stable_json_value).collect())
        }
        serde_json::Value::Object(map) => {
            let mut stable = serde_json::Map::new();
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (key, value) in entries {
                stable.insert(key.clone(), stable_json_value(value));
            }
            serde_json::Value::Object(stable)
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn locked_rate_map() -> Vec<RateMapEntry> {
        vec![
            RateMapEntry {
                unit: USAGE_INPUT_TOKEN.to_owned(),
                per_unit_au: 20,
                granularity: 1_000,
            },
            RateMapEntry {
                unit: USAGE_OUTPUT_TOKEN.to_owned(),
                per_unit_au: 60,
                granularity: 1_000,
            },
        ]
    }

    #[test]
    fn exposes_crate_name() {
        assert_eq!(CRATE_NAME, "mayhem-proto");
    }

    #[test]
    fn at_most_integer_canonicalization_never_exceeds_the_request() {
        let mut enumerated = EndpointAttributeSpec::new(EndpointValueType::Integer);
        enumerated.enum_values = (9_u64..=121).step_by(8).map(Value::from).collect();
        assert_eq!(
            canonicalize_positive_integer_at_most_for_spec(96, &enumerated).unwrap(),
            89
        );
        assert!(
            canonicalize_positive_integer_at_most_for_spec(8, &enumerated)
                .unwrap_err()
                .contains("no value at or below")
        );

        let mut ranged = EndpointAttributeSpec::new(EndpointValueType::Integer);
        ranged.minimum = Some(8.0);
        ranged.maximum = Some(128.0);
        ranged.multiple_of = Some(8.0);
        assert_eq!(
            canonicalize_positive_integer_at_most_for_spec(95, &ranged).unwrap(),
            88
        );
        assert_eq!(
            canonicalize_positive_integer_at_most_for_spec(200, &ranged).unwrap(),
            128
        );
    }

    #[test]
    fn nearest_integer_canonicalization_remains_available_for_explicit_values() {
        let mut spec = EndpointAttributeSpec::new(EndpointValueType::Integer);
        spec.enum_values = (9_u64..=121).step_by(8).map(Value::from).collect();
        assert_eq!(
            canonicalize_positive_integer_for_spec(96, &spec).unwrap(),
            97
        );
    }

    fn binary_speciality_descriptor() -> ModelSpecialityDescriptor {
        ModelSpecialityDescriptor {
            name: "optional_capability".to_owned(),
            mechanism: "boolean".to_owned(),
            default_level: "off".to_owned(),
            levels: vec![
                ModelSpecialityLevel {
                    name: "off".to_owned(),
                    rank: 0,
                    native_value: json!(false),
                    default_max_output_tokens: None,
                    max_reasoning_tokens: None,
                },
                ModelSpecialityLevel {
                    name: "on".to_owned(),
                    rank: 1,
                    native_value: json!(true),
                    default_max_output_tokens: None,
                    max_reasoning_tokens: None,
                },
            ],
            calibration_modalities: Vec::new(),
            research_evidence: vec!["test fixture".to_owned()],
        }
    }

    #[test]
    fn exact_speciality_selector_keeps_existing_mapping_json_byte_identical() {
        let original = r#"{"request_path":"reasoning_effort","target":"chat_template_kwarg","native_path":"reasoning_effort"}"#;
        let mapping: EndpointSpecialityMapping = serde_json::from_str(original).unwrap();

        assert_eq!(mapping.selector, EndpointSpecialitySelector::Exact);
        assert_eq!(serde_json::to_string(&mapping).unwrap(), original);
    }

    #[test]
    fn non_empty_speciality_selector_maps_presence_to_binary_native_values() {
        let descriptor = binary_speciality_descriptor();
        let mapping = EndpointSpecialityMapping {
            request_path: "negative_prompt".to_owned(),
            target: EndpointSpecialityTarget::BackendParameter,
            native_path: "negative_prompt".to_owned(),
            selector: EndpointSpecialitySelector::NonEmpty,
        };
        for empty in [
            None,
            Some(&Value::Null),
            Some(&json!("")),
            Some(&json!([])),
            Some(&json!({})),
        ] {
            assert_eq!(
                endpoint_speciality_level_for_request(&descriptor, &mapping, empty)
                    .map(|level| level.name.as_str()),
                Some("off")
            );
        }
        for non_empty in [
            Some(&json!("detail")),
            Some(&json!(["detail"])),
            Some(&json!({"value": "detail"})),
            Some(&json!(0)),
            Some(&json!(false)),
        ] {
            assert_eq!(
                endpoint_speciality_level_for_request(&descriptor, &mapping, non_empty)
                    .map(|level| level.name.as_str()),
                Some("on")
            );
        }
    }

    #[test]
    fn context_bracket_schedule_selects_active_version() {
        let mut schedule = default_ctx_bracket_schedule();
        assert_eq!(
            ctx_bracket_for_tokens_in_schedule(9_000, &schedule, 0),
            Some(("le32k".to_owned(), CTX_BRACKET_TABLE_VERSION))
        );
        schedule.pending = Some(CtxBracketTableRecord {
            ver: 2,
            effective_at: 100,
            submitted_at: 10,
            brackets: vec![
                CtxBracketEntry {
                    id: "le16k".to_owned(),
                    max_ctx: Some(16_384),
                },
                CtxBracketEntry {
                    id: "gt16k".to_owned(),
                    max_ctx: None,
                },
            ],
        });
        validate_ctx_bracket_schedule(&schedule).unwrap();
        assert_eq!(
            ctx_bracket_for_tokens_in_schedule(9_000, &schedule, 99),
            Some(("le32k".to_owned(), CTX_BRACKET_TABLE_VERSION))
        );
        assert_eq!(
            ctx_bracket_for_tokens_in_schedule(9_000, &schedule, 100),
            Some(("le16k".to_owned(), 2))
        );

        let mut invalid = schedule.current.clone();
        invalid.brackets[0].max_ctx = None;
        assert!(validate_ctx_bracket_table(&invalid)
            .unwrap_err()
            .contains("only the last"));
    }

    #[test]
    fn runtime_config_requires_explicit_model_class() {
        let missing = serde_json::from_value::<AttestationRuntimeConfig>(json!({
            "backend": "llama.cpp",
            "ctx": 8192,
            "tp_degree": 1
        }));

        assert!(missing.is_err());
    }

    #[test]
    fn catalog_enclave_id_binds_model_identity_but_not_runtime_release() {
        let base = CatalogEnclaveIdentity {
            admin_pubkey: "admin".to_owned(),
            model_id: "model".to_owned(),
            artifact_root: "artifact".to_owned(),
            artifact_sidecar_roots: BTreeMap::new(),
            manifest_hash: "manifest".to_owned(),
            binary_hash: "binary".to_owned(),
        };
        let mut changed = base.clone();
        changed.manifest_hash = "other-manifest".to_owned();

        assert_ne!(catalog_enclave_id(&base), catalog_enclave_id(&changed));

        let mut changed_binary = base.clone();
        changed_binary.binary_hash = "other-binary".to_owned();
        assert_eq!(
            catalog_enclave_id(&base),
            catalog_enclave_id(&changed_binary)
        );
    }

    #[test]
    fn catalog_enclave_id_separates_shifted_field_boundaries() {
        let first = CatalogEnclaveIdentity {
            admin_pubkey: "ab".to_owned(),
            model_id: "c".to_owned(),
            artifact_root: "artifact".to_owned(),
            artifact_sidecar_roots: BTreeMap::from([("adapter".to_owned(), "root".to_owned())]),
            manifest_hash: "manifest".to_owned(),
            binary_hash: "binary".to_owned(),
        };
        let second = CatalogEnclaveIdentity {
            admin_pubkey: "a".to_owned(),
            model_id: "bc".to_owned(),
            ..first.clone()
        };

        assert_ne!(catalog_enclave_id(&first), catalog_enclave_id(&second));
    }

    #[test]
    fn catalog_enclave_id_matches_canonical_five_sidecar_vector() {
        let identity = CatalogEnclaveIdentity {
            admin_pubkey: "admin".to_owned(),
            model_id: "nvidia/parakeet-tdt-0.6b-v3".to_owned(),
            artifact_root: "aa".repeat(32),
            artifact_sidecar_roots: BTreeMap::from([
                ("transformers_config".to_owned(), "11".repeat(32)),
                ("transformers_generation_config".to_owned(), "22".repeat(32)),
                ("transformers_processor_config".to_owned(), "33".repeat(32)),
                ("transformers_tokenizer_config".to_owned(), "44".repeat(32)),
                ("transformers_tokenizer_json".to_owned(), "55".repeat(32)),
            ]),
            manifest_hash: "66".repeat(32),
            binary_hash: String::new(),
        };

        assert_eq!(
            catalog_enclave_id(&identity),
            "92f874cc308be95ad33bc745139891d73c4f0e7fc873d5a49c4f5b4e86745db5"
        );
    }

    #[test]
    fn hardware_quote_binding_includes_freshness_and_identity() {
        let mut body = AttestationBody {
            schema_version: ATTESTATION_SCHEMA_VERSION,
            alg: ATTESTATION_ALG.to_owned(),
            enclave_id: "enclave".to_owned(),
            enclave_pubkey: "enclave-pub".to_owned(),
            provider_pubkey: "provider-pub".to_owned(),
            manifest_hash: "manifest".to_owned(),
            binary_hash: "binary".to_owned(),
            att_tier: TIER2_DEVICE_IDENTITY_TIER,
            hw_quote: None,
            boot_epoch: 1,
            report_ts: 2,
            nonce_u: "aa".repeat(32),
            runtime_config: AttestationRuntimeConfig::default(),
        };
        let base = hardware_quote_binding(&body).unwrap();
        body.hw_quote = Some(HardwareQuote {
            kind: HardwareQuoteKind::NvidiaGb10DeviceJwt,
            evidence: "jwt.invalid.parts".to_owned(),
            binding: base.clone(),
            endorsements: Vec::new(),
            metadata: serde_json::Value::Null,
        });
        assert_eq!(hardware_quote_binding(&body).unwrap(), base);
        body.nonce_u = "bb".repeat(32);
        assert_ne!(hardware_quote_binding(&body).unwrap(), base);
        body.nonce_u = "aa".repeat(32);
        body.report_ts = 99;
        assert_ne!(hardware_quote_binding(&body).unwrap(), base);
        body.report_ts = 2;
        body.manifest_hash = "other-manifest".to_owned();
        assert_ne!(hardware_quote_binding(&body).unwrap(), base);
    }

    #[test]
    fn tpm2_ek_quote_kind_is_tier2_and_snake_case() {
        assert_eq!(
            HardwareQuoteKind::Tpm2QuoteEk.attestation_tier(),
            TIER2_DEVICE_IDENTITY_TIER
        );
        let encoded = serde_json::to_string(&HardwareQuoteKind::Tpm2QuoteEk).unwrap();
        assert_eq!(encoded, "\"tpm2_quote_ek\"");
        let decoded: HardwareQuoteKind = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, HardwareQuoteKind::Tpm2QuoteEk);
    }

    #[test]
    fn attestation_verifier_profiles_are_fixed_to_quote_kinds() {
        let profiles = [
            AttestationVerifierProfile::AppleAppAttestNativeV1,
            AttestationVerifierProfile::AmdSevSnpVcekV1,
            AttestationVerifierProfile::IntelTdxDcapV1,
            AttestationVerifierProfile::NvidiaGb10DeviceV1,
            AttestationVerifierProfile::NvidiaNrasCompositeV1,
            AttestationVerifierProfile::NvidiaNvtrustOfflineCompositeV1,
            AttestationVerifierProfile::Tpm2EkActivateCredentialV1,
        ];

        assert_eq!(
            profiles.map(AttestationVerifierProfile::quote_kind),
            HardwareQuoteKind::ALL
        );
    }

    #[test]
    fn enclave_attestation_binding_is_exact_and_excludes_runtime_release_identity() {
        let binding = AdminEnclaveAttestationBinding {
            enclave_id: "11".repeat(32),
            kind: HardwareQuoteKind::NvidiaNrasJwt,
            platform: Some("nvidia-h100-cc".to_owned()),
            measurement_trust_data: BTreeMap::from([
                (
                    AttestationMeasurementLayer::Cpu,
                    "cpu-measurement".to_owned(),
                ),
                (
                    AttestationMeasurementLayer::Gpu,
                    "gpu-measurement".to_owned(),
                ),
                (
                    AttestationMeasurementLayer::Workload,
                    "workload-measurement".to_owned(),
                ),
            ]),
        };
        let value = serde_json::to_value(&binding).unwrap();
        assert_eq!(
            value["measurement_trust_data"],
            json!({
                "cpu": "cpu-measurement",
                "gpu": "gpu-measurement",
                "workload": "workload-measurement"
            })
        );
        assert!(value.get("binary_hash").is_none());

        let mut with_binary_approval = value.as_object().unwrap().clone();
        with_binary_approval.insert("binary_hash".to_owned(), json!("22".repeat(32)));
        assert!(
            serde_json::from_value::<AdminEnclaveAttestationBinding>(Value::Object(
                with_binary_approval
            ))
            .is_err()
        );
    }

    #[test]
    fn hardware_quote_route_wire_has_no_provider_trust_source_fields() {
        let route = HardwareQuoteRouteAdvertisement {
            kind: HardwareQuoteKind::Tpm2QuoteEk,
            declared_platform: Some("windows-11-tpm2".to_owned()),
        };
        let value = serde_json::to_value(&route).unwrap();
        assert_eq!(value["kind"], "tpm2_quote_ek");
        assert!(value.get("url").is_none());
        assert!(value.get("jku").is_none());

        for (field, value) in [
            ("jku", json!("https://provider.invalid/keys")),
            ("trust_roots", json!(["provider-root"])),
            ("attestation_policy", json!({"enabled": true})),
            ("attestation_policy_chain", json!([])),
        ] {
            let mut advertised = serde_json::to_value(&route)
                .unwrap()
                .as_object()
                .unwrap()
                .clone();
            advertised.insert(field.to_owned(), value);
            assert!(
                serde_json::from_value::<HardwareQuoteRouteAdvertisement>(Value::Object(
                    advertised
                ))
                .is_err(),
                "provider route accepted forbidden field {field}"
            );
        }
    }

    #[test]
    fn hardware_quote_rejects_provider_supplied_trust_authority() {
        let quote = HardwareQuote {
            kind: HardwareQuoteKind::Tpm2QuoteEk,
            evidence: "provider-evidence".to_owned(),
            binding: "11".repeat(32),
            endorsements: Vec::new(),
            metadata: Value::Null,
        };
        let encoded = serde_json::to_value(&quote).unwrap();

        for forbidden in [
            "verifier",
            "verifier_code",
            "trust_roots",
            "jwks",
            "policy",
            "golden_values",
        ] {
            let mut object = encoded.as_object().unwrap().clone();
            object.insert(forbidden.to_owned(), json!("provider-controlled"));
            assert!(
                serde_json::from_value::<HardwareQuote>(Value::Object(object)).is_err(),
                "accepted forbidden provider field {forbidden}"
            );
        }
    }

    #[test]
    fn admin_attestation_policy_rejects_unknown_executable_fields() {
        let policy = json!({
            "schema_version": ATTESTATION_POLICY_SCHEMA_VERSION,
            "sequence": 1,
            "issued_epoch": 10,
            "effective_epoch": 11,
            "min_verifier_version": 1,
            "quote_kinds": [],
            "hardware_quote_verifier_command": "provider-controlled-verifier"
        });

        assert!(serde_json::from_value::<AdminAttestationPolicy>(policy).is_err());
    }

    #[test]
    fn tpm_activate_credential_wire_round_trips_and_rejects_jku() {
        let challenge = TpmActivateCredentialChallenge {
            schema_version: TPM_ACTIVATE_CREDENTIAL_SCHEMA_VERSION,
            challenge_id: "11".repeat(32),
            ek_public_sha256: "22".repeat(32),
            ak_name_b64: "AAs=".to_owned(),
            quote_binding: "33".repeat(32),
            credential_blob_b64: "AAE=".to_owned(),
            encrypted_secret_b64: "AAI=".to_owned(),
            issued_at_unix: 100,
            expires_at_unix: 130,
        };
        let encoded = serde_json::to_value(&challenge).unwrap();
        assert_eq!(
            serde_json::from_value::<TpmActivateCredentialChallenge>(encoded.clone()).unwrap(),
            challenge
        );

        let mut object = encoded.as_object().unwrap().clone();
        object.insert("jku".to_owned(), json!("https://provider.invalid/tpm-keys"));
        assert!(
            serde_json::from_value::<TpmActivateCredentialChallenge>(Value::Object(object))
                .is_err()
        );
    }

    #[test]
    fn tpm_activation_frames_reject_unknown_fields() {
        let challenge = TpmActivateCredentialChallenge {
            schema_version: TPM_ACTIVATE_CREDENTIAL_SCHEMA_VERSION,
            challenge_id: "11".repeat(32),
            ek_public_sha256: "22".repeat(32),
            ak_name_b64: "AAs=".to_owned(),
            quote_binding: "33".repeat(32),
            credential_blob_b64: "AAE=".to_owned(),
            encrypted_secret_b64: "AAI=".to_owned(),
            issued_at_unix: 100,
            expires_at_unix: 130,
        };
        let response = TpmActivateCredentialResponse {
            schema_version: TPM_ACTIVATE_CREDENTIAL_SCHEMA_VERSION,
            challenge_id: challenge.challenge_id.clone(),
            ak_name_b64: challenge.ak_name_b64.clone(),
            quote_binding: challenge.quote_binding.clone(),
            activated_secret_b64: "AAM=".to_owned(),
        };
        let challenge_frame = TpmActivateCredentialChallengeFrame {
            frame_type: TPM_ACTIVATE_CREDENTIAL_CHALLENGE_FRAME_TYPE.to_owned(),
            version: TPM_ACTIVATE_CREDENTIAL_FRAME_VERSION,
            session_id: challenge.challenge_id.clone(),
            provider: "provider".to_owned(),
            enclave_id: "enclave".to_owned(),
            room_id: "room".to_owned(),
            challenge,
        };
        let response_frame = TpmActivateCredentialResponseFrame {
            frame_type: TPM_ACTIVATE_CREDENTIAL_RESPONSE_FRAME_TYPE.to_owned(),
            version: TPM_ACTIVATE_CREDENTIAL_FRAME_VERSION,
            session_id: response.challenge_id.clone(),
            provider: challenge_frame.provider.clone(),
            enclave_id: challenge_frame.enclave_id.clone(),
            room_id: challenge_frame.room_id.clone(),
            response,
        };

        let mut encoded_challenge = serde_json::to_value(&challenge_frame)
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        encoded_challenge.insert("admin_override".to_owned(), json!(true));
        assert!(
            serde_json::from_value::<TpmActivateCredentialChallengeFrame>(Value::Object(
                encoded_challenge
            ))
            .is_err()
        );

        let mut encoded_response = serde_json::to_value(&response_frame)
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        encoded_response.insert("admin_override".to_owned(), json!(true));
        assert!(
            serde_json::from_value::<TpmActivateCredentialResponseFrame>(Value::Object(
                encoded_response
            ))
            .is_err()
        );

        assert_eq!(
            serde_json::from_value::<TpmActivateCredentialChallengeFrame>(
                serde_json::to_value(&challenge_frame).unwrap()
            )
            .unwrap(),
            challenge_frame
        );
        assert_eq!(
            serde_json::from_value::<TpmActivateCredentialResponseFrame>(
                serde_json::to_value(&response_frame).unwrap()
            )
            .unwrap(),
            response_frame
        );
    }

    #[test]
    fn tpm_quote_evidence_rejects_provider_policy_and_root_fields() {
        let evidence = TpmQuoteEvidence {
            schema_version: TPM_QUOTE_EVIDENCE_SCHEMA_VERSION,
            ak_public_b64: "AAE=".to_owned(),
            ak_name_b64: "AAs=".to_owned(),
            quote_attest_b64: "AAI=".to_owned(),
            quote_signature_b64: "AAM=".to_owned(),
            pcr_values: vec![TpmPcrValue {
                hash_algorithm: TpmHashAlgorithm::Sha256,
                index: 7,
                digest: "11".repeat(32),
            }],
        };
        let encoded = serde_json::to_value(&evidence).unwrap();
        assert_eq!(
            serde_json::from_value::<TpmQuoteEvidence>(encoded.clone()).unwrap(),
            evidence
        );

        for forbidden in ["trust_roots", "policy", "verifier", "jku", "golden_values"] {
            let mut object = encoded.as_object().unwrap().clone();
            object.insert(forbidden.to_owned(), json!("provider-controlled"));
            assert!(
                serde_json::from_value::<TpmQuoteEvidence>(Value::Object(object)).is_err(),
                "accepted forbidden provider field {forbidden}"
            );
        }
    }

    #[test]
    fn voucher_and_receipt_signing_payloads_are_bound_to_terms() {
        let voucher = SpendVoucherBody {
            schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
            session_id: "sess".to_owned(),
            billing_id: "11".repeat(32),
            billing_attempt: 0,
            billing_prior_usage: ReceiptUsage::default(),
            billing_prior_au_owed_cum: 0,
            billing_epoch: 7,
            reservation_id: "22".repeat(32),
            reservation_expires_after_epoch: 31,
            reservation_receipt_grace_epochs: 6,
            user: "33".repeat(32),
            provider: "44".repeat(32),
            payout_revision: "55".repeat(32),
            model_id: "model".to_owned(),
            rules_ver: 1,
            rail: "fiat".to_owned(),
            enclave_id: "enclave".to_owned(),
            price_ver: 1,
            locked_rate_map: locked_rate_map(),
            locked_per_req_au: 7,
            locked_min_session_au: 11,
            served_ctx: 8192,
            required_modalities: vec!["text".to_owned()],
            required_specialities: BTreeMap::new(),
            workflow: None,
            ctx_bracket: Some("le8k".to_owned()),
            ctx_bracket_table_ver: Some(CTX_BRACKET_TABLE_VERSION),
            max_spend_au: 5000,
            checkpoint_every: CheckpointPolicy {
                tokens: 8192,
                ms: 30000,
            },
        };
        let mut changed = voucher.clone();
        changed.price_ver = 2;
        assert_ne!(
            spend_voucher_signing_bytes(&voucher).unwrap(),
            spend_voucher_signing_bytes(&changed).unwrap()
        );
        let mut changed = voucher.clone();
        changed.required_modalities = vec!["image".to_owned(), "text".to_owned()];
        assert_ne!(
            spend_voucher_signing_bytes(&voucher).unwrap(),
            spend_voucher_signing_bytes(&changed).unwrap()
        );
        let workflow = WorkflowBinding {
            endpoint_family: "comfy_workflow".to_owned(),
            graph_hash: "66".repeat(32),
            runtime_id: "comfyui.cuda124".to_owned(),
            outcome_class: "image.batch".to_owned(),
            quoted_usage: ReceiptUsage::from_units([(USAGE_IMAGE, 1), (USAGE_STEP, 20)]),
        };
        let mut workflow_voucher = voucher.clone();
        workflow_voucher.workflow = Some(workflow.clone());
        assert_ne!(
            spend_voucher_signing_bytes(&voucher).unwrap(),
            spend_voucher_signing_bytes(&workflow_voucher).unwrap()
        );
        let mut changed = workflow_voucher.clone();
        changed.workflow.as_mut().unwrap().graph_hash = "77".repeat(32);
        assert_ne!(
            spend_voucher_signing_bytes(&workflow_voucher).unwrap(),
            spend_voucher_signing_bytes(&changed).unwrap()
        );

        let receipt = ReceiptBody {
            schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
            session_id: "sess".to_owned(),
            billing_id: voucher.billing_id.clone(),
            billing_attempt: 0,
            billing_prior_usage: ReceiptUsage::default(),
            billing_prior_au_owed_cum: 0,
            billing_epoch: voucher.billing_epoch,
            reservation_id: voucher.reservation_id.clone(),
            reservation_expires_after_epoch: voucher.reservation_expires_after_epoch,
            reservation_receipt_grace_epochs: voucher.reservation_receipt_grace_epochs,
            payout_revision: voucher.payout_revision.clone(),
            seq: 1,
            final_receipt: false,
            rail: "fiat".to_owned(),
            user: "user".to_owned(),
            provider: "provider".to_owned(),
            enclave_id: "enclave".to_owned(),
            model_id: "model".to_owned(),
            price_ver: 1,
            locked_rate_map: locked_rate_map(),
            locked_per_req_au: 7,
            locked_min_session_au: 11,
            served_ctx: voucher.served_ctx,
            ctx_bracket: voucher.ctx_bracket.clone(),
            ctx_bracket_table_ver: voucher.ctx_bracket_table_ver,
            rules_ver: 1,
            workflow: None,
            workflow_output: None,
            usage: ReceiptUsage::text(3, 5),
            usage_attribution: BTreeMap::new(),
            au_owed_cum: 1,
            prompt_hash: "hash".to_owned(),
            ts: 10,
        };
        let mut changed = receipt.clone();
        changed.au_owed_cum = 2;
        assert_ne!(
            receipt_signing_bytes(&receipt).unwrap(),
            receipt_signing_bytes(&changed).unwrap()
        );
        changed = receipt.clone();
        changed.served_ctx = 4096;
        assert_ne!(
            receipt_signing_bytes(&receipt).unwrap(),
            receipt_signing_bytes(&changed).unwrap()
        );
        changed = receipt.clone();
        changed.ctx_bracket = Some("le32k".to_owned());
        assert_ne!(
            receipt_signing_bytes(&receipt).unwrap(),
            receipt_signing_bytes(&changed).unwrap()
        );
        changed = receipt.clone();
        changed.billing_epoch += 1;
        assert_ne!(
            receipt_signing_bytes(&receipt).unwrap(),
            receipt_signing_bytes(&changed).unwrap()
        );
        changed = receipt.clone();
        changed.reservation_expires_after_epoch += 1;
        assert_ne!(
            receipt_signing_bytes(&receipt).unwrap(),
            receipt_signing_bytes(&changed).unwrap()
        );
        changed = receipt.clone();
        changed.reservation_receipt_grace_epochs += 1;
        assert_ne!(
            receipt_signing_bytes(&receipt).unwrap(),
            receipt_signing_bytes(&changed).unwrap()
        );
        let mut workflow_receipt = receipt.clone();
        workflow_receipt.workflow = Some(workflow);
        workflow_receipt.workflow_output = Some(WorkflowOutputBinding {
            output_modalities: vec!["image".to_owned()],
            metrics: BTreeMap::from([
                ("bytes".to_owned(), 512_000),
                ("height".to_owned(), 1024),
                ("width".to_owned(), 1024),
            ]),
        });
        assert_ne!(
            receipt_signing_bytes(&receipt).unwrap(),
            receipt_signing_bytes(&workflow_receipt).unwrap()
        );
        let mut changed = workflow_receipt.clone();
        changed
            .workflow_output
            .as_mut()
            .unwrap()
            .metrics
            .insert("bytes".to_owned(), 513_000);
        assert_ne!(
            receipt_signing_bytes(&workflow_receipt).unwrap(),
            receipt_signing_bytes(&changed).unwrap()
        );
    }

    #[test]
    fn receipt_usage_aliases_parse_to_current_canonical_units() {
        let aliased_usage: ReceiptUsage =
            serde_json::from_value(serde_json::json!({ "in": 3, "out_tokens": 5 })).unwrap();
        assert_eq!(aliased_usage, ReceiptUsage::text(3, 5));
        assert_eq!(
            serde_json::to_value(&aliased_usage).unwrap(),
            serde_json::json!({ "input_token": 3, "output_token": 5 })
        );
        let cached_usage: ReceiptUsage = serde_json::from_value(serde_json::json!({
            "input_tokens": 7,
            "cached_prompt_tokens": 11,
            "completion_tokens": 13
        }))
        .unwrap();
        assert_eq!(cached_usage, ReceiptUsage::text_with_cached(7, 11, 13));
        assert_eq!(cached_usage.prompt_tokens(), 18);
        assert_eq!(
            serde_json::to_value(&cached_usage).unwrap(),
            serde_json::json!({
                "cached_input_token": 11,
                "input_token": 7,
                "output_token": 13
            })
        );
        let mixed_alias_usage: ReceiptUsage = serde_json::from_value(serde_json::json!({
            "in": 3,
            "input_token": 2,
            "out_tokens": 5,
            "completion_tokens": 7
        }))
        .unwrap();
        assert_eq!(mixed_alias_usage, ReceiptUsage::text(5, 12));
    }

    #[test]
    fn signing_payloads_use_current_version_only() {
        let voucher = SpendVoucherBody {
            schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
            session_id: "sess".to_owned(),
            billing_id: "11".repeat(32),
            billing_attempt: 0,
            billing_prior_usage: ReceiptUsage::default(),
            billing_prior_au_owed_cum: 0,
            billing_epoch: 7,
            reservation_id: "22".repeat(32),
            reservation_expires_after_epoch: 31,
            reservation_receipt_grace_epochs: 6,
            user: "33".repeat(32),
            provider: "44".repeat(32),
            payout_revision: "55".repeat(32),
            model_id: "model".to_owned(),
            rules_ver: 1,
            rail: "fiat".to_owned(),
            enclave_id: "enclave".to_owned(),
            price_ver: 1,
            locked_rate_map: locked_rate_map(),
            locked_per_req_au: 7,
            locked_min_session_au: 11,
            served_ctx: 8192,
            required_modalities: vec!["text".to_owned()],
            required_specialities: BTreeMap::new(),
            workflow: None,
            ctx_bracket: Some("le8k".to_owned()),
            ctx_bracket_table_ver: Some(CTX_BRACKET_TABLE_VERSION),
            max_spend_au: 5000,
            checkpoint_every: CheckpointPolicy {
                tokens: 8192,
                ms: 30000,
            },
        };
        let current_voucher = spend_voucher_signing_bytes(&voucher).unwrap();
        assert!(String::from_utf8(current_voucher.clone())
            .unwrap()
            .contains("\"signing_version\":2"));

        let receipt = ReceiptBody {
            schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
            session_id: "sess".to_owned(),
            billing_id: voucher.billing_id.clone(),
            billing_attempt: 0,
            billing_prior_usage: ReceiptUsage::default(),
            billing_prior_au_owed_cum: 0,
            billing_epoch: voucher.billing_epoch,
            reservation_id: voucher.reservation_id.clone(),
            reservation_expires_after_epoch: voucher.reservation_expires_after_epoch,
            reservation_receipt_grace_epochs: voucher.reservation_receipt_grace_epochs,
            payout_revision: voucher.payout_revision.clone(),
            seq: 1,
            final_receipt: false,
            rail: "fiat".to_owned(),
            user: "user".to_owned(),
            provider: "provider".to_owned(),
            enclave_id: "enclave".to_owned(),
            model_id: "model".to_owned(),
            price_ver: 1,
            locked_rate_map: locked_rate_map(),
            locked_per_req_au: 7,
            locked_min_session_au: 11,
            served_ctx: voucher.served_ctx,
            ctx_bracket: voucher.ctx_bracket.clone(),
            ctx_bracket_table_ver: voucher.ctx_bracket_table_ver,
            rules_ver: 1,
            workflow: None,
            workflow_output: None,
            usage: ReceiptUsage::text(3, 5),
            usage_attribution: BTreeMap::new(),
            au_owed_cum: 1,
            prompt_hash: "hash".to_owned(),
            ts: 10,
        };
        let current_receipt = receipt_signing_bytes(&receipt).unwrap();
        assert!(String::from_utf8(current_receipt.clone())
            .unwrap()
            .contains("\"signing_version\":2"));
    }

    #[test]
    fn js_contract_atto_money_signing_fixture_matches_rust() {
        let locked_rate_map = vec![
            RateMapEntry {
                unit: USAGE_INPUT_TOKEN.to_owned(),
                per_unit_au: 10_000_000,
                granularity: 1,
            },
            RateMapEntry {
                unit: USAGE_OUTPUT_TOKEN.to_owned(),
                per_unit_au: 2_500_000_000_000_000,
                granularity: 1_000,
            },
        ];
        let voucher = SpendVoucherBody {
            schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
            session_id: "sess-au-roundtrip".to_owned(),
            billing_id: "44".repeat(32),
            billing_attempt: 0,
            billing_prior_usage: ReceiptUsage::default(),
            billing_prior_au_owed_cum: 0,
            billing_epoch: 12,
            reservation_id: "55".repeat(32),
            reservation_expires_after_epoch: 31,
            reservation_receipt_grace_epochs: 6,
            user: "11".repeat(32),
            provider: "22".repeat(32),
            payout_revision: "66".repeat(32),
            model_id: "model/atto-roundtrip".to_owned(),
            rules_ver: 7,
            rail: "fiat".to_owned(),
            enclave_id: "enclave-au-roundtrip".to_owned(),
            price_ver: 9,
            locked_rate_map: locked_rate_map.clone(),
            locked_per_req_au: 1,
            locked_min_session_au: 2_000_000_000_000_000_000_000_000,
            served_ctx: 131_072,
            required_modalities: vec!["text".to_owned()],
            required_specialities: BTreeMap::new(),
            workflow: None,
            ctx_bracket: Some("le128k".to_owned()),
            ctx_bracket_table_ver: Some(CTX_BRACKET_TABLE_VERSION),
            max_spend_au: 2_000_000_000_000_000_000_000_001,
            checkpoint_every: CheckpointPolicy {
                tokens: 4096,
                ms: 30000,
            },
        };
        let expected_voucher = concat!(
            "{\"domain\":\"mayhem-spend-voucher\",\"signing_version\":2,\"body\":{",
            "\"schema_version\":11,\"session_id\":\"sess-au-roundtrip\",",
            "\"billing_id\":\"4444444444444444444444444444444444444444444444444444444444444444\",",
            "\"billing_attempt\":0,\"billing_prior_usage\":{},\"billing_prior_au_owed_cum\":\"0\",",
            "\"billing_epoch\":12,",
            "\"reservation_id\":\"5555555555555555555555555555555555555555555555555555555555555555\",",
            "\"reservation_expires_after_epoch\":31,\"reservation_receipt_grace_epochs\":6,",
            "\"user\":\"1111111111111111111111111111111111111111111111111111111111111111\",",
            "\"provider\":\"2222222222222222222222222222222222222222222222222222222222222222\",",
            "\"payout_revision\":\"6666666666666666666666666666666666666666666666666666666666666666\",",
            "\"rail\":\"fiat\",\"enclave_id\":\"enclave-au-roundtrip\",",
            "\"model_id\":\"model/atto-roundtrip\",",
            "\"price_ver\":9,\"locked_rate_map\":[",
            "{\"unit\":\"input_token\",\"per_unit_au\":\"10000000\",\"granularity\":1},",
            "{\"unit\":\"output_token\",\"per_unit_au\":\"2500000000000000\",\"granularity\":1000}",
            "],\"locked_per_req_au\":\"1\",\"locked_min_session_au\":\"2000000000000000000000000\",",
            "\"served_ctx\":131072,\"required_modalities\":[\"text\"],",
            "\"ctx_bracket\":\"le128k\",\"ctx_bracket_table_ver\":1,",
            "\"rules_ver\":7,",
            "\"max_spend_au\":\"2000000000000000000000001\",",
            "\"checkpoint_every\":{\"tokens\":4096,\"ms\":30000}}}"
        );
        assert_eq!(
            String::from_utf8(spend_voucher_signing_bytes(&voucher).unwrap()).unwrap(),
            expected_voucher
        );

        let receipt = ReceiptBody {
            schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
            session_id: "sess-au-roundtrip".to_owned(),
            billing_id: "44".repeat(32),
            billing_attempt: 0,
            billing_prior_usage: ReceiptUsage::default(),
            billing_prior_au_owed_cum: 0,
            billing_epoch: voucher.billing_epoch,
            reservation_id: voucher.reservation_id.clone(),
            reservation_expires_after_epoch: voucher.reservation_expires_after_epoch,
            reservation_receipt_grace_epochs: voucher.reservation_receipt_grace_epochs,
            payout_revision: voucher.payout_revision.clone(),
            seq: 2,
            final_receipt: true,
            rail: "fiat".to_owned(),
            user: "11".repeat(32),
            provider: "22".repeat(32),
            enclave_id: "enclave-au-roundtrip".to_owned(),
            model_id: "model/atto-roundtrip".to_owned(),
            price_ver: 9,
            locked_rate_map,
            locked_per_req_au: voucher.locked_per_req_au,
            locked_min_session_au: voucher.locked_min_session_au,
            served_ctx: voucher.served_ctx,
            ctx_bracket: voucher.ctx_bracket,
            ctx_bracket_table_ver: voucher.ctx_bracket_table_ver,
            rules_ver: 7,
            workflow: None,
            workflow_output: None,
            usage: ReceiptUsage::text(3, 5),
            usage_attribution: BTreeMap::new(),
            au_owed_cum: voucher.max_spend_au,
            prompt_hash: "33".repeat(32),
            ts: 1_783_517_300,
        };
        let expected_receipt = concat!(
            "{\"domain\":\"mayhem-session-receipt\",\"signing_version\":2,\"body\":{",
            "\"schema_version\":11,\"session_id\":\"sess-au-roundtrip\",",
            "\"billing_id\":\"4444444444444444444444444444444444444444444444444444444444444444\",",
            "\"billing_attempt\":0,\"billing_prior_usage\":{},\"billing_prior_au_owed_cum\":\"0\",",
            "\"billing_epoch\":12,",
            "\"reservation_id\":\"5555555555555555555555555555555555555555555555555555555555555555\",",
            "\"reservation_expires_after_epoch\":31,\"reservation_receipt_grace_epochs\":6,",
            "\"payout_revision\":\"6666666666666666666666666666666666666666666666666666666666666666\",",
            "\"seq\":2,\"final\":true,",
            "\"rail\":\"fiat\",\"user\":\"1111111111111111111111111111111111111111111111111111111111111111\",",
            "\"provider\":\"2222222222222222222222222222222222222222222222222222222222222222\",",
            "\"enclave_id\":\"enclave-au-roundtrip\",\"model_id\":\"model/atto-roundtrip\",",
            "\"price_ver\":9,\"locked_rate_map\":[",
            "{\"unit\":\"input_token\",\"per_unit_au\":\"10000000\",\"granularity\":1},",
            "{\"unit\":\"output_token\",\"per_unit_au\":\"2500000000000000\",\"granularity\":1000}",
            "],\"locked_per_req_au\":\"1\",\"locked_min_session_au\":\"2000000000000000000000000\",",
            "\"served_ctx\":131072,\"ctx_bracket\":\"le128k\",\"ctx_bracket_table_ver\":1,",
            "\"rules_ver\":7,\"usage\":{\"input_token\":3,\"output_token\":5},",
            "\"au_owed_cum\":\"2000000000000000000000001\",",
            "\"prompt_hash\":\"3333333333333333333333333333333333333333333333333333333333333333\",",
            "\"ts\":1783517300}}"
        );
        assert_eq!(
            String::from_utf8(receipt_signing_bytes(&receipt).unwrap()).unwrap(),
            expected_receipt
        );
    }

    #[test]
    fn session_accept_signing_payload_is_stable_bound_and_sig_excluded() {
        let mut frame = json!({
            "t": "s.accept",
            "v": 1,
            "session_id": "aa".repeat(32),
            "open_head": "bb".repeat(32),
            "att_nonce": "88".repeat(32),
            "att_report": {
                "provider_pubkey": "55".repeat(32),
                "enclave_id": "11".repeat(32),
                "sig_provider": "66".repeat(64)
            },
            "engine": { "mode": "provider-session-server-v1", "ctx": 8192 },
            "ts": 123,
            "nonce": "77".repeat(32)
        });
        let payload = session_accept_signing_bytes(&frame).unwrap();

        frame["sig"] = json!("88".repeat(64));
        assert_eq!(session_accept_signing_bytes(&frame).unwrap(), payload);

        let reordered = json!({
            "nonce": "77".repeat(32),
            "ts": 123,
            "engine": { "ctx": 8192, "mode": "provider-session-server-v1" },
            "att_nonce": "88".repeat(32),
            "open_head": "bb".repeat(32),
            "att_report": {
                "sig_provider": "66".repeat(64),
                "enclave_id": "11".repeat(32),
                "provider_pubkey": "55".repeat(32)
            },
            "session_id": "aa".repeat(32),
            "v": 1,
            "t": "s.accept"
        });
        assert_eq!(session_accept_signing_bytes(&reordered).unwrap(), payload);

        frame["session_id"] = json!("bb".repeat(32));
        assert_ne!(session_accept_signing_bytes(&frame).unwrap(), payload);
    }

    #[test]
    fn session_frame_head_is_stable_and_exact_frame_bound() {
        let frame = json!({
            "t": "s.open",
            "session_id": "aa".repeat(32),
            "voucher": { "price_ver": 1, "max_spend_au": "1000" },
            "sig": "11".repeat(64),
        });
        let reordered = json!({
            "sig": "11".repeat(64),
            "voucher": { "max_spend_au": "1000", "price_ver": 1 },
            "session_id": "aa".repeat(32),
            "t": "s.open",
        });
        assert_eq!(
            session_frame_head(&frame).unwrap(),
            session_frame_head(&reordered).unwrap()
        );

        let mut changed = frame;
        changed["sig"] = json!("22".repeat(64));
        assert_ne!(
            session_frame_head(&changed).unwrap(),
            session_frame_head(&reordered).unwrap()
        );
    }

    #[test]
    fn normalized_request_prompt_units_are_stable_and_byte_derived() {
        let request = json!({
            "tools": [{"type": "function", "function": {"name": "lookup"}}],
            "messages": [{"role": "user", "content": "find it"}],
        });
        let reordered = json!({
            "messages": [{"content": "find it", "role": "user"}],
            "tools": [{"function": {"name": "lookup"}, "type": "function"}],
        });
        let bytes = stable_json_bytes(&request).unwrap();
        let expected =
            (u64::try_from(bytes.len()).unwrap() + NORMALIZED_REQUEST_BYTES_PER_PROMPT_UNIT - 1)
                / NORMALIZED_REQUEST_BYTES_PER_PROMPT_UNIT;
        assert_eq!(normalized_request_prompt_units(&request).unwrap(), expected);
        assert_eq!(
            normalized_request_prompt_units(&request).unwrap(),
            normalized_request_prompt_units(&reordered).unwrap()
        );
    }

    #[test]
    fn tools_only_model_input_metering_ignores_transport_envelope_fields() {
        let request = json!({
            "model": "admin/needle",
            "messages": [{"role": "user", "content": "find it"}],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "lookup",
                    "description": "Look up a value.",
                    "parameters": {
                        "type": "object",
                        "properties": {"key": {"type": "string"}},
                        "required": ["key"],
                        "additionalProperties": false
                    }
                }
            }],
            "metadata": {"trace": "a"},
            "user": "buyer-a",
            "stream": false,
            "max_tokens": 32
        });
        let mut changed_envelope = request.clone();
        changed_envelope["model"] = json!("other/model");
        changed_envelope["metadata"] = json!({"trace": "completely different"});
        changed_envelope["user"] = json!("buyer-b");
        changed_envelope["stream"] = json!(true);
        changed_envelope["max_tokens"] = json!(512);

        let expected =
            tools_only_model_input_prompt_units(ENDPOINT_OPENAI_CHAT_COMPLETIONS, &request)
                .unwrap();
        assert_eq!(
            expected,
            tools_only_model_input_prompt_units(
                ENDPOINT_OPENAI_CHAT_COMPLETIONS,
                &changed_envelope
            )
            .unwrap()
        );

        let responses = json!({
            "model": "admin/needle",
            "input": "find it",
            "tools": [{
                "type": "function",
                "name": "lookup",
                "description": "Look up a value.",
                "parameters": {
                    "type": "object",
                    "properties": {"key": {"type": "string"}},
                    "required": ["key"],
                    "additionalProperties": false
                }
            }],
            "metadata": {"different": true},
            "max_output_tokens": 512
        });
        assert_eq!(
            expected,
            tools_only_model_input_prompt_units(ENDPOINT_OPENAI_RESPONSES, &responses).unwrap()
        );
    }

    #[test]
    fn tools_only_model_input_metering_tracks_only_consumed_query_and_tools() {
        let request = json!({
            "messages": [{"role": "user", "content": "find it"}],
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "lookup",
                        "description": "Look up a value.",
                        "parameters": {"type": "object", "properties": {}}
                    }
                },
                {
                    "type": "function",
                    "function": {
                        "name": "ignore_me",
                        "description": "This tool is not selected.",
                        "parameters": {"type": "object", "properties": {}}
                    }
                }
            ],
            "tool_choice": {"type": "function", "function": {"name": "lookup"}}
        });
        let selected =
            tools_only_model_input_prompt_units(ENDPOINT_OPENAI_CHAT_COMPLETIONS, &request)
                .unwrap();

        let mut ignored_tool_changed = request.clone();
        ignored_tool_changed["tools"][1]["function"]["description"] = json!("x".repeat(1024));
        assert_eq!(
            selected,
            tools_only_model_input_prompt_units(
                ENDPOINT_OPENAI_CHAT_COMPLETIONS,
                &ignored_tool_changed
            )
            .unwrap()
        );

        let mut query_changed = request.clone();
        query_changed["messages"][0]["content"] = json!("find a much longer value");
        assert_ne!(
            selected,
            tools_only_model_input_prompt_units(ENDPOINT_OPENAI_CHAT_COMPLETIONS, &query_changed)
                .unwrap()
        );

        let mut selected_tool_changed = request;
        selected_tool_changed["tools"][0]["function"]["description"] =
            json!("A substantially longer model-visible description.");
        assert_ne!(
            selected,
            tools_only_model_input_prompt_units(
                ENDPOINT_OPENAI_CHAT_COMPLETIONS,
                &selected_tool_changed
            )
            .unwrap()
        );
    }

    #[test]
    fn payload_chunks_roundtrip_stable_json() {
        let value = json!({
            "z": "tail",
            "a": ["hello", "world", { "nested": true }],
            "long": "x".repeat(DEFAULT_SESSION_PAYLOAD_CHUNK_BYTES + 17),
        });
        let (manifest, chunks) = chunk_json_payload(&value, 1024).unwrap();
        assert!(chunks.len() > 1);
        assert_eq!(manifest.chunk_count, chunks.len() as u64);
        let restored = reassemble_json_payload(&manifest, &chunks).unwrap();
        assert_eq!(restored, stable_json_value(&value));
    }

    fn transcription_timestamp(text: &str, start: f64, end: f64) -> TranscriptionTimestamp {
        TranscriptionTimestamp {
            text: text.to_owned(),
            start,
            end,
        }
    }

    fn assert_invalid_transcription_timestamp(
        result: &TranscriptionResult,
        kind: &'static str,
        index: u64,
        reason: &'static str,
    ) {
        assert_eq!(
            validate_transcription_result(result, TranscriptionResultLimits::default()),
            Err(TranscriptionResultError::InvalidTimestamp {
                kind,
                index,
                reason,
            })
        );
    }

    #[test]
    fn transcription_result_allows_optional_timestamp_metadata() {
        let text_only = TranscriptionResult::text("hello mayhem");
        validate_transcription_result(&text_only, TranscriptionResultLimits::default()).unwrap();
        let encoded = serde_json::to_value(&text_only).unwrap();
        assert!(encoded.get("words").is_none());
        assert!(encoded.get("segments").is_none());
        assert_eq!(
            serde_json::from_value::<TranscriptionResult>(json!({
                "v": TRANSCRIPTION_RESULT_SCHEMA_VERSION,
                "text": "hello mayhem",
            }))
            .unwrap(),
            text_only
        );

        let mut words_only = TranscriptionResult::text("hello mayhem");
        words_only.duration_seconds = Some(1.0);
        words_only.words = vec![
            transcription_timestamp("hello", 0.0, 0.5),
            transcription_timestamp("mayhem", 0.5, 1.0),
        ];
        validate_transcription_result(&words_only, TranscriptionResultLimits::default()).unwrap();

        let mut segments_only = TranscriptionResult::text("hello mayhem");
        segments_only.duration_seconds = Some(1.0);
        segments_only.segments = vec![transcription_timestamp("hello mayhem", 0.0, 1.0)];
        validate_transcription_result(&segments_only, TranscriptionResultLimits::default())
            .unwrap();
    }

    #[test]
    fn transcription_result_roundtrips_through_bounded_chunks() {
        let result = TranscriptionResult {
            schema_version: TRANSCRIPTION_RESULT_SCHEMA_VERSION,
            text: "hello mayhem".to_owned(),
            detected_language: Some("en".to_owned()),
            duration_seconds: Some(1.5),
            words: vec![
                TranscriptionTimestamp {
                    text: "hello".to_owned(),
                    start: 0.0,
                    end: 0.6,
                },
                TranscriptionTimestamp {
                    text: "mayhem".to_owned(),
                    start: 0.7,
                    end: 1.5,
                },
            ],
            segments: vec![TranscriptionTimestamp {
                text: "hello mayhem".to_owned(),
                start: 0.0,
                end: 1.5,
            }],
        };
        let limits = TranscriptionResultLimits {
            max_bytes: 4096,
            max_timestamp_entries: 8,
        };

        let (manifest, chunks) = chunk_transcription_result(&result, 32, limits).unwrap();

        assert!(chunks.len() > 1);
        assert_eq!(
            reassemble_transcription_result(&manifest, &chunks, limits).unwrap(),
            result
        );
    }

    #[test]
    fn transcription_result_rejects_non_finite_empty_or_non_positive_timestamps() {
        let invalid_entries = [
            (
                transcription_timestamp("  ", 0.0, 0.5),
                "text must not be empty",
            ),
            (
                transcription_timestamp("hello", f64::NAN, 0.5),
                "start and end must be finite",
            ),
            (
                transcription_timestamp("hello", 0.0, f64::INFINITY),
                "start and end must be finite",
            ),
            (
                transcription_timestamp("hello", -0.1, 0.5),
                "start and end must be non-negative",
            ),
            (
                transcription_timestamp("hello", 0.5, 0.5),
                "end must be greater than start",
            ),
            (
                transcription_timestamp("hello", 0.6, 0.5),
                "end must be greater than start",
            ),
        ];
        for (entry, reason) in invalid_entries {
            let mut result = TranscriptionResult::text("hello");
            result.words = vec![entry];
            assert_invalid_transcription_timestamp(&result, "word", 0, reason);
        }

        for duration in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut result = TranscriptionResult::text("hello");
            result.duration_seconds = Some(duration);
            assert_invalid_transcription_timestamp(
                &result,
                "duration",
                0,
                "duration must be finite and positive",
            );
        }
    }

    #[test]
    fn transcription_result_rejects_overlapping_or_duration_exceeding_timestamps() {
        let mut result = TranscriptionResult::text("hello mayhem");
        result.duration_seconds = Some(1.0);
        result.words = vec![
            transcription_timestamp("hello", 0.0, 0.6),
            transcription_timestamp("mayhem", 0.5, 0.9),
        ];
        assert_invalid_transcription_timestamp(
            &result,
            "word",
            1,
            "entries must be ordered and non-overlapping",
        );

        result.words[1] = transcription_timestamp("mayhem", 0.6, 1.1);
        assert_invalid_transcription_timestamp(&result, "word", 1, "end must not exceed duration");

        result.words.clear();
        result.segments = vec![transcription_timestamp("hello mayhem", 0.0, 1.1)];
        assert_invalid_transcription_timestamp(
            &result,
            "segment",
            0,
            "end must not exceed duration",
        );
    }

    #[test]
    fn transcription_result_rejects_inconsistent_word_and_segment_ranges() {
        let mut result = TranscriptionResult::text("hello");
        result.duration_seconds = Some(1.0);
        result.words = vec![transcription_timestamp("hello", 0.4, 0.6)];
        result.segments = vec![
            transcription_timestamp("hel", 0.0, 0.5),
            transcription_timestamp("lo", 0.5, 1.0),
        ];
        assert_invalid_transcription_timestamp(
            &result,
            "word",
            0,
            "word must be contained within a segment",
        );

        result.words = vec![transcription_timestamp("hello", 0.5, 0.8)];
        result.segments = vec![
            transcription_timestamp("unused", 0.0, 0.4),
            transcription_timestamp("hello", 0.4, 1.0),
        ];
        assert_invalid_transcription_timestamp(
            &result,
            "segment",
            0,
            "segment must contain at least one word",
        );
    }

    #[test]
    fn transcription_result_rejects_unordered_or_unbounded_timestamp_counts() {
        let mut result = TranscriptionResult::text("hello mayhem");
        result.words = vec![
            TranscriptionTimestamp {
                text: "mayhem".to_owned(),
                start: 1.0,
                end: 1.5,
            },
            TranscriptionTimestamp {
                text: "hello".to_owned(),
                start: 0.0,
                end: 0.5,
            },
        ];
        assert!(matches!(
            validate_transcription_result(&result, TranscriptionResultLimits::default()),
            Err(TranscriptionResultError::InvalidTimestamp { .. })
        ));

        result.words.truncate(1);
        assert!(matches!(
            validate_transcription_result(
                &result,
                TranscriptionResultLimits {
                    max_bytes: 4096,
                    max_timestamp_entries: 0,
                },
            ),
            Err(TranscriptionResultError::TooManyTimestampEntries { .. })
        ));
    }

    #[test]
    fn payload_chunks_reject_missing_duplicate_reordered_and_corrupt_chunks() {
        let bytes = b"chunk me into enough pieces to test the guard rails";
        let (manifest, chunks) = chunk_payload_bytes(bytes, 8).unwrap();

        let missing = &chunks[..chunks.len() - 1];
        assert!(matches!(
            reassemble_payload_chunks(&manifest, missing).unwrap_err(),
            PayloadChunkError::ChunkCountMismatch { .. }
        ));

        let mut duplicate = chunks.clone();
        duplicate[1] = duplicate[0].clone();
        assert!(matches!(
            reassemble_payload_chunks(&manifest, &duplicate).unwrap_err(),
            PayloadChunkError::ReorderedChunk { .. } | PayloadChunkError::DuplicateChunk { .. }
        ));

        let mut reordered = chunks.clone();
        reordered.swap(0, 1);
        assert!(matches!(
            reassemble_payload_chunks(&manifest, &reordered).unwrap_err(),
            PayloadChunkError::ReorderedChunk { .. }
        ));

        let mut corrupt = chunks.clone();
        corrupt[0].data = "00".repeat(8);
        assert!(matches!(
            reassemble_payload_chunks(&manifest, &corrupt).unwrap_err(),
            PayloadChunkError::ChunkHashMismatch { .. }
        ));
    }

    #[test]
    fn incremental_payload_collector_bounds_and_reassembles_large_json() {
        let value = json!({
            "messages": [{"role": "user", "content": "x".repeat(3 * 1024 * 1024)}]
        });
        let (manifest, chunks) = chunk_json_payload(&value, 16 * 1024).unwrap();
        let mut collector = PayloadChunkCollector::new(4 * 1024 * 1024, 4096);
        for chunk in chunks {
            collector.push(chunk).unwrap();
        }
        assert_eq!(collector.chunk_count(), manifest.chunk_count as usize);
        assert_eq!(collector.finish_json(&manifest).unwrap(), value);

        let (_manifest, chunks) = chunk_payload_bytes(b"0123456789", 5).unwrap();
        let mut too_small = PayloadChunkCollector::new(9, 8);
        too_small.push(chunks[0].clone()).unwrap();
        assert!(matches!(
            too_small.push(chunks[1].clone()).unwrap_err(),
            PayloadChunkError::PayloadTooLarge { .. }
        ));

        let mut too_few = PayloadChunkCollector::new(64, 1);
        too_few.push(chunks[0].clone()).unwrap();
        assert!(matches!(
            too_few.push(chunks[1].clone()).unwrap_err(),
            PayloadChunkError::TooManyChunks { .. }
        ));
    }

    #[test]
    fn incremental_payload_collector_rejects_replay_and_post_final_data() {
        let (_manifest, chunks) = chunk_payload_bytes(b"0123456789", 5).unwrap();
        let mut replay = PayloadChunkCollector::new(64, 8);
        replay.push(chunks[0].clone()).unwrap();
        assert!(matches!(
            replay.push(chunks[0].clone()).unwrap_err(),
            PayloadChunkError::ReorderedChunk { .. }
        ));

        let mut after_final = PayloadChunkCollector::new(64, 8);
        after_final.push(chunks[0].clone()).unwrap();
        after_final.push(chunks[1].clone()).unwrap();
        assert!(matches!(
            after_final.push(chunks[1].clone()).unwrap_err(),
            PayloadChunkError::ChunkAfterFinal { .. }
        ));
    }
}
