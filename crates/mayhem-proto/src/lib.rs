#![forbid(unsafe_code)]

use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};

pub const CRATE_NAME: &str = "mayhem-proto";
pub const CONTRACT_VERSION: u32 = 2;
pub const ATTESTATION_SCHEMA_VERSION: u32 = 1;
pub const ATTESTATION_ALG: &str = "ed25519";
pub const SESSION_RECEIPT_SCHEMA_VERSION: u32 = 3;
pub const NEXT_SESSION_RECEIPT_SCHEMA_VERSION: u32 = 4;
pub const SIGNING_MESSAGE_VERSION: u32 = 2;
pub const SUPPORTED_SIGNING_MESSAGE_VERSIONS: &[u32] = &[SIGNING_MESSAGE_VERSION, 1];
pub const HARDWARE_QUOTE_BINDING_DOMAIN: &str = "mayhem-hardware-quote-binding-v1";
pub const SESSION_ACCEPT_SIGNING_DOMAIN: &str = "mayhem/session-accept/v1";
pub const DEFAULT_MODEL_CLASS: &str = "text-generation";
pub const USAGE_INPUT_TOKEN: &str = "input_token";
pub const USAGE_OUTPUT_TOKEN: &str = "output_token";
pub const USAGE_IMAGE: &str = "image";
pub const USAGE_STEP: &str = "step";
pub const USAGE_INPUT_CHARACTER: &str = "input_character";
pub const USAGE_AUDIO_SECOND: &str = "audio_second";
pub const DEFAULT_SESSION_MAX_FRAME_BYTES: usize = 256 * 1024;
pub const DEFAULT_SESSION_PAYLOAD_CHUNK_BYTES: usize = 16 * 1024;
pub const SESSION_PAYLOAD_CHUNK_SCHEMA_VERSION: u32 = 1;
pub const SESSION_PAYLOAD_CHUNK_ENCODING: &str = "hex";
pub const TIER1_SOFTWARE_ATTESTATION_TIER: u8 = 1;
pub const TIER2_DEVICE_IDENTITY_TIER: u8 = 2;
pub const TIER3_CONFIDENTIAL_COMPUTE_TIER: u8 = 3;
pub const TIER4_PROVIDER_KYB_TIER: u8 = 4;

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
    #[serde(default = "default_model_class")]
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
}

impl HardwareQuoteKind {
    pub fn attestation_tier(&self) -> u8 {
        match self {
            Self::AppleAppAttestJwt | Self::NvidiaGb10DeviceJwt => TIER2_DEVICE_IDENTITY_TIER,
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
    pub per_unit_mu: u64,
    pub granularity: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SpendVoucherBody {
    pub session_id: String,
    pub rail: String,
    pub enclave_id: String,
    pub price_ver: u64,
    pub locked_rate_map: Vec<RateMapEntry>,
    pub max_spend_mu: u64,
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
    pub rules_ver: u64,
    pub usage: ReceiptUsage,
    pub mu_owed_cum: u64,
    pub prompt_hash: String,
    pub ts: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionReceipt {
    #[serde(flatten)]
    pub body: ReceiptBody,
    pub enclave_sig: String,
    pub user_sig: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceiptSchemaMigrationError {
    Unsupported { from: u32, to: u32 },
}

impl fmt::Display for ReceiptSchemaMigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { from, to } => {
                write!(f, "unsupported receipt schema migration {from} -> {to}")
            }
        }
    }
}

impl std::error::Error for ReceiptSchemaMigrationError {}

pub fn migrate_receipt_body(
    body: &ReceiptBody,
) -> Result<ReceiptBody, ReceiptSchemaMigrationError> {
    migrate_receipt_body_to_schema(body, SESSION_RECEIPT_SCHEMA_VERSION)
}

pub fn migrate_receipt_body_to_schema(
    body: &ReceiptBody,
    target_schema_version: u32,
) -> Result<ReceiptBody, ReceiptSchemaMigrationError> {
    let mut migrated = body.clone();
    if migrated.schema_version > target_schema_version {
        return Err(ReceiptSchemaMigrationError::Unsupported {
            from: migrated.schema_version,
            to: target_schema_version,
        });
    }

    while migrated.schema_version < target_schema_version {
        match migrated.schema_version {
            1 => {
                migrated.usage = ReceiptUsage::new(migrated.usage.units().clone());
                migrated.schema_version = 2;
            }
            2 => migrated.schema_version = 3,
            3 => migrated.schema_version = 4,
            from => {
                return Err(ReceiptSchemaMigrationError::Unsupported {
                    from,
                    to: target_schema_version,
                });
            }
        }
    }

    Ok(migrated)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReceiptAck {
    pub session_id: String,
    pub seq: u64,
    pub user_sig: String,
}

#[derive(Serialize)]
struct SpendVoucherSigningEnvelopeV1<'a> {
    domain: &'static str,
    body: &'a SpendVoucherBody,
}

#[derive(Serialize)]
struct SpendVoucherSigningEnvelopeV2<'a> {
    domain: &'static str,
    signing_version: u32,
    body: &'a SpendVoucherBody,
}

#[derive(Serialize)]
struct ReceiptSigningEnvelopeV1<'a> {
    domain: &'static str,
    body: &'a ReceiptBody,
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
    hasher.update(identity.binary_hash.as_bytes());
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
    spend_voucher_signing_bytes_for_version(body, SIGNING_MESSAGE_VERSION)
}

pub fn spend_voucher_signing_bytes_for_version(
    body: &SpendVoucherBody,
    signing_version: u32,
) -> Result<Vec<u8>, serde_json::Error> {
    match signing_version {
        1 => serde_json::to_vec(&SpendVoucherSigningEnvelopeV1 {
            domain: "mayhem-spend-voucher-v1",
            body,
        }),
        2 => serde_json::to_vec(&SpendVoucherSigningEnvelopeV2 {
            domain: "mayhem-spend-voucher",
            signing_version: 2,
            body,
        }),
        _ => serde_json::to_vec(&SpendVoucherSigningEnvelopeV2 {
            domain: "mayhem-spend-voucher-unsupported",
            signing_version,
            body,
        }),
    }
}

pub fn supported_spend_voucher_signing_bytes(
    body: &SpendVoucherBody,
) -> Result<Vec<Vec<u8>>, serde_json::Error> {
    SUPPORTED_SIGNING_MESSAGE_VERSIONS
        .iter()
        .map(|version| spend_voucher_signing_bytes_for_version(body, *version))
        .collect()
}

pub fn receipt_signing_bytes(body: &ReceiptBody) -> Result<Vec<u8>, serde_json::Error> {
    receipt_signing_bytes_for_version(body, SIGNING_MESSAGE_VERSION)
}

pub fn receipt_signing_bytes_for_version(
    body: &ReceiptBody,
    signing_version: u32,
) -> Result<Vec<u8>, serde_json::Error> {
    match signing_version {
        1 => serde_json::to_vec(&ReceiptSigningEnvelopeV1 {
            domain: "mayhem-session-receipt-v1",
            body,
        }),
        2 => serde_json::to_vec(&ReceiptSigningEnvelopeV2 {
            domain: "mayhem-session-receipt",
            signing_version: 2,
            body,
        }),
        _ => serde_json::to_vec(&ReceiptSigningEnvelopeV2 {
            domain: "mayhem-session-receipt-unsupported",
            signing_version,
            body,
        }),
    }
}

pub fn supported_receipt_signing_bytes(
    body: &ReceiptBody,
) -> Result<Vec<Vec<u8>>, serde_json::Error> {
    SUPPORTED_SIGNING_MESSAGE_VERSIONS
        .iter()
        .map(|version| receipt_signing_bytes_for_version(body, *version))
        .collect()
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
            Self::Json(message) => write!(f, "payload JSON error: {message}"),
        }
    }
}

impl std::error::Error for PayloadChunkError {}

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
    if chunk_size == 0 {
        return Err(PayloadChunkError::EmptyChunkSize);
    }
    let total_len = u64::try_from(bytes.len()).map_err(|_| PayloadChunkError::LengthOverflow)?;
    let chunk_size_u64 =
        u64::try_from(chunk_size).map_err(|_| PayloadChunkError::LengthOverflow)?;
    let root = blake3_hex(bytes);
    let mut chunks = Vec::new();
    if bytes.is_empty() {
        return Ok((
            PayloadChunkManifest {
                schema_version: SESSION_PAYLOAD_CHUNK_SCHEMA_VERSION,
                encoding: SESSION_PAYLOAD_CHUNK_ENCODING.to_owned(),
                total_len,
                chunk_size: chunk_size_u64,
                chunk_count: 0,
                blake3: root,
                chunks: Vec::new(),
            },
            chunks,
        ));
    }
    for (index, chunk) in bytes.chunks(chunk_size).enumerate() {
        let index_u64 = u64::try_from(index).map_err(|_| PayloadChunkError::LengthOverflow)?;
        let offset = index
            .checked_mul(chunk_size)
            .ok_or(PayloadChunkError::LengthOverflow)?;
        let offset_u64 = u64::try_from(offset).map_err(|_| PayloadChunkError::LengthOverflow)?;
        let len_u64 = u64::try_from(chunk.len()).map_err(|_| PayloadChunkError::LengthOverflow)?;
        let hash = blake3_hex(chunk);
        chunks.push(PayloadChunk {
            i: index_u64,
            offset: offset_u64,
            len: len_u64,
            blake3: hash,
            encoding: SESSION_PAYLOAD_CHUNK_ENCODING.to_owned(),
            data: hex_encode(chunk),
            final_chunk: false,
        });
    }
    if let Some(last) = chunks.last_mut() {
        last.final_chunk = true;
    }
    let manifest = PayloadChunkManifest {
        schema_version: SESSION_PAYLOAD_CHUNK_SCHEMA_VERSION,
        encoding: SESSION_PAYLOAD_CHUNK_ENCODING.to_owned(),
        total_len,
        chunk_size: chunk_size_u64,
        chunk_count: u64::try_from(chunks.len()).map_err(|_| PayloadChunkError::LengthOverflow)?,
        blake3: root,
        chunks: chunks.iter().map(|chunk| chunk.blake3.clone()).collect(),
    };
    Ok((manifest, chunks))
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
                per_unit_mu: 20,
                granularity: 1_000,
            },
            RateMapEntry {
                unit: USAGE_OUTPUT_TOKEN.to_owned(),
                per_unit_mu: 60,
                granularity: 1_000,
            },
        ]
    }

    #[test]
    fn exposes_crate_name() {
        assert_eq!(CRATE_NAME, "mayhem-proto");
    }

    #[test]
    fn runtime_config_defaults_model_class_for_legacy_text_reports() {
        let config: AttestationRuntimeConfig = serde_json::from_value(json!({
            "backend": "llama.cpp",
            "ctx": 8192,
            "tp_degree": 1
        }))
        .unwrap();

        assert_eq!(config.model_class, DEFAULT_MODEL_CLASS);
    }

    #[test]
    fn catalog_enclave_id_changes_when_bound_fields_change() {
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
        });
        assert_eq!(hardware_quote_binding(&body).unwrap(), base);
        body.nonce_u = "bb".repeat(32);
        body.report_ts = 99;
        assert_eq!(hardware_quote_binding(&body).unwrap(), base);
        body.manifest_hash = "other-manifest".to_owned();
        assert_ne!(hardware_quote_binding(&body).unwrap(), base);
    }

    #[test]
    fn voucher_and_receipt_signing_payloads_are_bound_to_terms() {
        let voucher = SpendVoucherBody {
            session_id: "sess".to_owned(),
            rail: "fiat".to_owned(),
            enclave_id: "enclave".to_owned(),
            price_ver: 1,
            locked_rate_map: locked_rate_map(),
            max_spend_mu: 5000,
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
            rules_ver: 1,
            usage: ReceiptUsage::text(3, 5),
            mu_owed_cum: 1,
            prompt_hash: "hash".to_owned(),
            ts: 10,
        };
        let mut changed = receipt.clone();
        changed.mu_owed_cum = 2;
        assert_ne!(
            receipt_signing_bytes(&receipt).unwrap(),
            receipt_signing_bytes(&changed).unwrap()
        );
    }

    #[test]
    fn receipt_schema_migration_accepts_v1_for_v2_nodes() {
        let legacy_usage: ReceiptUsage =
            serde_json::from_value(serde_json::json!({ "in": 3, "out_tokens": 5 })).unwrap();
        assert_eq!(legacy_usage, ReceiptUsage::text(3, 5));
        assert_eq!(
            serde_json::to_value(&legacy_usage).unwrap(),
            serde_json::json!({ "input_token": 3, "output_token": 5 })
        );
        let mixed_alias_usage: ReceiptUsage = serde_json::from_value(serde_json::json!({
            "in": 3,
            "input_token": 2,
            "out_tokens": 5,
            "completion_tokens": 7
        }))
        .unwrap();
        assert_eq!(mixed_alias_usage, ReceiptUsage::text(5, 12));

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
            rules_ver: 1,
            usage: ReceiptUsage::text(3, 5),
            mu_owed_cum: 1,
            prompt_hash: "hash".to_owned(),
            ts: 10,
        };

        assert_eq!(migrate_receipt_body(&receipt).unwrap(), receipt);

        let migrated =
            migrate_receipt_body_to_schema(&receipt, NEXT_SESSION_RECEIPT_SCHEMA_VERSION).unwrap();
        assert_eq!(migrated.schema_version, NEXT_SESSION_RECEIPT_SCHEMA_VERSION);
        assert_eq!(migrated.session_id, receipt.session_id);
        assert_eq!(migrated.usage, receipt.usage);

        let mut unsupported = receipt.clone();
        unsupported.schema_version = 99;
        assert_eq!(
            migrate_receipt_body_to_schema(&unsupported, NEXT_SESSION_RECEIPT_SCHEMA_VERSION)
                .unwrap_err(),
            ReceiptSchemaMigrationError::Unsupported {
                from: 99,
                to: NEXT_SESSION_RECEIPT_SCHEMA_VERSION,
            }
        );

        assert_eq!(
            migrate_receipt_body_to_schema(&migrated, SESSION_RECEIPT_SCHEMA_VERSION).unwrap_err(),
            ReceiptSchemaMigrationError::Unsupported {
                from: NEXT_SESSION_RECEIPT_SCHEMA_VERSION,
                to: SESSION_RECEIPT_SCHEMA_VERSION,
            }
        );
    }

    #[test]
    fn signing_payloads_are_versioned_and_keep_legacy_payloads_supported() {
        let voucher = SpendVoucherBody {
            session_id: "sess".to_owned(),
            rail: "fiat".to_owned(),
            enclave_id: "enclave".to_owned(),
            price_ver: 1,
            locked_rate_map: locked_rate_map(),
            max_spend_mu: 5000,
            checkpoint_every: CheckpointPolicy {
                tokens: 8192,
                ms: 30000,
            },
        };
        let current_voucher = spend_voucher_signing_bytes(&voucher).unwrap();
        let legacy_voucher = spend_voucher_signing_bytes_for_version(&voucher, 1).unwrap();
        assert_ne!(current_voucher, legacy_voucher);
        assert!(String::from_utf8(current_voucher.clone())
            .unwrap()
            .contains("\"signing_version\":2"));
        assert!(String::from_utf8(legacy_voucher.clone())
            .unwrap()
            .contains("mayhem-spend-voucher-v1"));
        assert_eq!(
            supported_spend_voucher_signing_bytes(&voucher).unwrap(),
            vec![current_voucher, legacy_voucher]
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
            rules_ver: 1,
            usage: ReceiptUsage::text(3, 5),
            mu_owed_cum: 1,
            prompt_hash: "hash".to_owned(),
            ts: 10,
        };
        let current_receipt = receipt_signing_bytes(&receipt).unwrap();
        let legacy_receipt = receipt_signing_bytes_for_version(&receipt, 1).unwrap();
        assert_ne!(current_receipt, legacy_receipt);
        assert!(String::from_utf8(current_receipt.clone())
            .unwrap()
            .contains("\"signing_version\":2"));
        assert!(String::from_utf8(legacy_receipt.clone())
            .unwrap()
            .contains("mayhem-session-receipt-v1"));
        assert_eq!(
            supported_receipt_signing_bytes(&receipt).unwrap(),
            vec![current_receipt, legacy_receipt]
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
            "voucher": { "price_ver": 1, "max_spend_mu": 1000 },
            "sig": "11".repeat(64),
        });
        let reordered = json!({
            "sig": "11".repeat(64),
            "voucher": { "max_spend_mu": 1000, "price_ver": 1 },
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
}
