#![forbid(unsafe_code)]

use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;

mod endpoint_contract;

pub use endpoint_contract::{
    endpoint_attribute_value_matches, endpoint_contract_fingerprint,
    endpoint_family_contract_template, endpoint_request_fingerprint,
    generate_endpoint_calibration_cases, materialize_endpoint_calibration_request,
    materialize_endpoint_request_defaults, validate_endpoint_attribute_value,
    validate_endpoint_request, validate_endpoint_response, EndpointCalibrationCase,
    EndpointCalibrationMutation, EndpointCalibrationValue, EndpointContractViolation,
};

pub const CRATE_NAME: &str = "mayhem-proto";
pub const CONTRACT_VERSION: u32 = 9;
pub const ATTESTATION_SCHEMA_VERSION: u32 = 1;
pub const ATTESTATION_ALG: &str = "ed25519";
pub const SESSION_RECEIPT_SCHEMA_VERSION: u32 = 8;
pub const SIGNING_MESSAGE_VERSION: u32 = 2;
pub const CTX_BRACKET_TABLE_VERSION: u32 = 1;
pub const CTX_BRACKETS: &[(u32, &str)] = &[
    (8_192, "le8k"),
    (32_768, "le32k"),
    (131_072, "le128k"),
    (262_144, "le256k"),
];
pub type MoneyAu = u128;

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
pub const HARDWARE_QUOTE_BINDING_DOMAIN: &str = "mayhem-hardware-quote-binding-v1";
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
pub const ENDPOINT_HF_TEXT_TO_AUDIO: &str = "hf_text_to_audio";
pub const DEFAULT_SESSION_MAX_FRAME_BYTES: usize = 256 * 1024;
pub const DEFAULT_SESSION_PAYLOAD_CHUNK_BYTES: usize = 16 * 1024;
pub const SESSION_PAYLOAD_CHUNK_SCHEMA_VERSION: u32 = 1;
pub const SESSION_PAYLOAD_CHUNK_ENCODING: &str = "hex";
pub const TIER1_SOFTWARE_ATTESTATION_TIER: u8 = 1;
pub const TIER2_DEVICE_IDENTITY_TIER: u8 = 2;
pub const TIER3_CONFIDENTIAL_COMPUTE_TIER: u8 = 3;
pub const TIER4_PROVIDER_KYB_TIER: u8 = 4;

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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EndpointSpecialityMapping {
    pub request_path: String,
    pub target: EndpointSpecialityTarget,
    pub native_path: String,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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
pub struct SpendVoucherBody {
    pub session_id: String,
    pub rail: String,
    pub enclave_id: String,
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
    #[serde(default)]
    pub ctx_bracket: Option<String>,
    #[serde(default)]
    pub ctx_bracket_table_ver: Option<u32>,
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

    pub fn is_monotonic_from(&self, previous: &Self) -> bool {
        previous
            .units()
            .iter()
            .all(|(unit, previous_count)| self.get(unit) >= *previous_count)
    }
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReceiptAck {
    pub session_id: String,
    pub seq: u64,
    pub user_sig: String,
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
    let mut hasher = blake3::Hasher::new();
    hasher.update(identity.admin_pubkey.as_bytes());
    hasher.update(identity.model_id.as_bytes());
    hasher.update(identity.artifact_root.as_bytes());
    for (name, root) in &identity.artifact_sidecar_roots {
        hasher.update(name.as_bytes());
        hasher.update(root.as_bytes());
    }
    hasher.update(identity.manifest_hash.as_bytes());
    hasher.finalize().to_hex().to_string()
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
    bound_body.report_ts = 0;
    bound_body.nonce_u.clear();
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
    fn hardware_quote_binding_excludes_quote_session_fields_but_includes_identity() {
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
        body.report_ts = 99;
        assert_eq!(hardware_quote_binding(&body).unwrap(), base);
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
    fn voucher_and_receipt_signing_payloads_are_bound_to_terms() {
        let voucher = SpendVoucherBody {
            session_id: "sess".to_owned(),
            rail: "fiat".to_owned(),
            enclave_id: "enclave".to_owned(),
            price_ver: 1,
            locked_rate_map: locked_rate_map(),
            locked_per_req_au: 7,
            locked_min_session_au: 11,
            served_ctx: 8192,
            required_modalities: vec!["text".to_owned()],
            required_specialities: BTreeMap::new(),
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

        let receipt = ReceiptBody {
            schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
            session_id: "sess".to_owned(),
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
            session_id: "sess".to_owned(),
            rail: "fiat".to_owned(),
            enclave_id: "enclave".to_owned(),
            price_ver: 1,
            locked_rate_map: locked_rate_map(),
            locked_per_req_au: 7,
            locked_min_session_au: 11,
            served_ctx: 8192,
            required_modalities: vec!["text".to_owned()],
            required_specialities: BTreeMap::new(),
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
            session_id: "sess-au-roundtrip".to_owned(),
            rail: "fiat".to_owned(),
            enclave_id: "enclave-au-roundtrip".to_owned(),
            price_ver: 9,
            locked_rate_map: locked_rate_map.clone(),
            locked_per_req_au: 1,
            locked_min_session_au: 2_000_000_000_000_000_000_000_000,
            served_ctx: 131_072,
            required_modalities: vec!["text".to_owned()],
            required_specialities: BTreeMap::new(),
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
            "\"session_id\":\"sess-au-roundtrip\",\"rail\":\"fiat\",\"enclave_id\":\"enclave-au-roundtrip\",",
            "\"price_ver\":9,\"locked_rate_map\":[",
            "{\"unit\":\"input_token\",\"per_unit_au\":\"10000000\",\"granularity\":1},",
            "{\"unit\":\"output_token\",\"per_unit_au\":\"2500000000000000\",\"granularity\":1000}",
            "],\"locked_per_req_au\":\"1\",\"locked_min_session_au\":\"2000000000000000000000000\",",
            "\"served_ctx\":131072,\"required_modalities\":[\"text\"],",
            "\"ctx_bracket\":\"le128k\",\"ctx_bracket_table_ver\":1,",
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
            usage: ReceiptUsage::text(3, 5),
            usage_attribution: BTreeMap::new(),
            au_owed_cum: voucher.max_spend_au,
            prompt_hash: "33".repeat(32),
            ts: 1_783_517_300,
        };
        let expected_receipt = concat!(
            "{\"domain\":\"mayhem-session-receipt\",\"signing_version\":2,\"body\":{",
            "\"schema_version\":8,\"session_id\":\"sess-au-roundtrip\",\"seq\":2,\"final\":true,",
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
