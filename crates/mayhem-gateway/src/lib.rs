#![forbid(unsafe_code)]

pub mod audit;
pub mod failover;
pub mod openai;
pub mod pricing;
pub mod provider_table;
pub mod reputation;
pub use audit::*;
pub use failover::*;
pub use pricing::*;
pub use provider_table::*;
pub use reputation::*;

use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use jsonwebtoken::{
    decode, decode_header,
    jwk::{JwkSet, KeyAlgorithm},
    Algorithm, DecodingKey, Validation,
};
use mayhem_proto::{
    attestation_report_head, attestation_signing_bytes, catalog_enclave_id, hardware_quote_binding,
    AttestationReport, AttestationSigner, CatalogEnclaveIdentity, HardwareQuote, HardwareQuoteKind,
    MoneyAu, ATTESTATION_ALG, ATTESTATION_SCHEMA_VERSION, CONTRACT_VERSION, DEFAULT_MODEL_CLASS,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;
use wait_timeout::ChildExt;

pub const CRATE_NAME: &str = "mayhem-gateway";
pub const DEFAULT_MAX_REPORT_AGE_SECS: u64 = 24 * 60 * 60;
pub const DEFAULT_MAX_REPORT_CLOCK_SKEW_SECS: u64 = 5 * 60;
pub const DEFAULT_HARDWARE_QUOTE_VERIFIER_TIMEOUT_SECS: u64 = 120;
pub const HEARTBEAT_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_HEARTBEAT_MAX_AGE_MILLIS: u64 = 30_000;
pub const DEFAULT_HEARTBEAT_MAX_CLOCK_SKEW_MILLIS: u64 = 5_000;
pub const DEFAULT_HEARTBEAT_REPLAY_CACHE_CAPACITY: usize = 5_000;
const APPLE_APP_ATTEST_ISSUER: &str = "https://appattest.apple.com";
const NVIDIA_LOCAL_VERIFIER_ISSUER: &str = "https://local.verifier.attestation.nvidia.com";
const NVIDIA_NRAS_ISSUER: &str = "https://nras.attestation.nvidia.com";
const TIER3_VENDOR_LAYER: &str = "vendor";
const TIER3_WORKLOAD_LAYER: &str = "workload";

type GoldenTier3Measurements = BTreeMap<String, BTreeMap<String, BTreeSet<String>>>;
type MatchedTier3Measurements = BTreeMap<String, BTreeMap<String, String>>;

type Result<T> = std::result::Result<T, GatewayError>;

#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("attestation schema_version must be {expected}, got {actual}")]
    BadSchemaVersion { expected: u32, actual: u32 },
    #[error("attestation alg must be {expected}, got {actual}")]
    BadAlgorithm {
        expected: &'static str,
        actual: String,
    },
    #[error("attestation field {field} mismatch: expected {expected}, got {actual}")]
    ContractMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
    #[error("attestation enclave_id mismatch: expected {expected}, got {actual}")]
    EnclaveIdMismatch { expected: String, actual: String },
    #[error("attestation binary_hash is not in the trusted release set: {binary_hash}")]
    BinaryHashNotTrusted { binary_hash: String },
    #[error("attestation nonce mismatch: expected {expected}, got {actual}")]
    NonceMismatch { expected: String, actual: String },
    #[error("attestation nonce must be 32 bytes of hex")]
    BadNonce,
    #[error("attestation report is stale: age {age_secs}s exceeds {max_age_secs}s")]
    ReportStale { age_secs: u64, max_age_secs: u64 },
    #[error("attestation report_ts is too far in the future: skew {skew_secs}s exceeds {max_skew_secs}s")]
    ReportFromFuture { skew_secs: u64, max_skew_secs: u64 },
    #[error("attestation {signer} public key is invalid: {reason}")]
    BadPublicKey {
        signer: &'static str,
        reason: String,
    },
    #[error("attestation {signer} signature is invalid: {reason}")]
    BadSignature {
        signer: &'static str,
        reason: String,
    },
    #[error("attestation report hash failed: {0}")]
    ReportHash(String),
    #[error("attestation signing payload failed: {0}")]
    SigningPayload(String),
    #[error("hardware attestation requires a hardware quote")]
    HardwareQuoteRequired,
    #[error("hardware quote evidence is empty")]
    HardwareQuoteEvidenceMissing,
    #[error("hardware quote binding mismatch: expected {expected}, got {actual}")]
    HardwareQuoteBindingMismatch { expected: String, actual: String },
    #[error("hardware quote kind {kind} is not verified by this build")]
    HardwareQuoteUnsupported { kind: String },
    #[error("hardware quote kind {kind} requires a trusted verifier root")]
    HardwareQuoteTrustRootMissing { kind: String },
    #[error("hardware quote kind {kind} is invalid: {reason}")]
    HardwareQuoteInvalid { kind: String, reason: String },
    #[error("heartbeat JSON error: {0}")]
    HeartbeatJson(String),
    #[error("heartbeat must have t=\"hb\" and v={expected_version}")]
    BadHeartbeatSchema { expected_version: u32 },
    #[error("contract upgrade required: expected CONTRACT_VERSION {expected}, got {actual}")]
    ContractUpgradeRequired { expected: u32, actual: u32 },
    #[error("heartbeat field {field} is invalid: {reason}")]
    BadHeartbeatField { field: &'static str, reason: String },
    #[error("heartbeat signature is invalid: {reason}")]
    BadHeartbeatSignature { reason: String },
    #[error("heartbeat nonce was already seen: {nonce}")]
    HeartbeatReplay { nonce: String },
    #[error("heartbeat is stale: age {age_millis}ms exceeds {max_age_millis}ms")]
    HeartbeatStale {
        age_millis: u64,
        max_age_millis: u64,
    },
    #[error("heartbeat timestamp is too far in the future: skew {skew_millis}ms exceeds {max_skew_millis}ms")]
    HeartbeatFromFuture {
        skew_millis: u64,
        max_skew_millis: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnclaveContractRecord {
    pub enclave_id: String,
    pub admin_pubkey: String,
    pub model_id: String,
    pub model_class: String,
    pub artifact_root: String,
    pub artifact_sidecar_roots: BTreeMap<String, String>,
    pub manifest_hash: String,
    pub binary_hash: String,
    pub launch_measurements: Value,
    pub att_tier: u8,
    pub caps: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HardwareQuoteVerifierCommand {
    pub command: PathBuf,
    pub timeout: Duration,
}

#[derive(Debug)]
pub struct AttestationVerificationRequest<'a> {
    pub report: &'a AttestationReport,
    pub contract: &'a EnclaveContractRecord,
    pub trusted_binary_hashes: &'a BTreeSet<String>,
    pub expected_nonce: &'a str,
    pub expected_provider_pubkey: &'a str,
    pub now_ts: u64,
    pub max_report_age_secs: u64,
    pub max_report_clock_skew_secs: u64,
    pub trusted_apple_app_attest_jwks: Option<&'a Value>,
    pub trusted_nvidia_gb10_device_jwks: Option<&'a Value>,
    pub trusted_nvidia_nras_jwks: Option<&'a Value>,
    pub trusted_nvidia_offline_jwks: Option<&'a Value>,
    pub hardware_quote_verifier_command: Option<&'a HardwareQuoteVerifierCommand>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedAttestation {
    pub enclave_id: String,
    pub provider_pubkey: String,
    pub enclave_pubkey: String,
    pub report_head: String,
    pub boot_epoch: u64,
    pub report_ts: u64,
    pub att_tier: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderHeartbeat {
    pub t: String,
    pub v: u32,
    pub contract_version: u32,
    pub provider: String,
    pub enclave_id: String,
    pub model_id: String,
    pub room_id: String,
    pub sat: f64,
    pub slots: HeartbeatSlots,
    pub q: HeartbeatQueue,
    pub perf: HeartbeatPerf,
    pub price_ver: u64,
    #[serde(default, with = "mayhem_proto::decimal_u128")]
    pub min_ask_au: MoneyAu,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_peer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_anchor: Option<String>,
    pub accepting_new: bool,
    pub caps: HeartbeatCaps,
    pub att: HeartbeatAttestation,
    pub ts: u64,
    pub nonce: String,
    pub sig: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeartbeatSlots {
    pub active: u32,
    pub active_requests: u32,
    pub max: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeartbeatQueue {
    pub free_slots: u32,
    pub engine_backlog: u32,
    pub est_wait_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeartbeatPerf {
    pub tok_s: Option<f64>,
    pub ttft_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeartbeatCaps {
    pub tools: bool,
    pub json: bool,
    pub ctx: u32,
    pub vision: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeartbeatAttestation {
    pub epoch: u64,
    pub head: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeartbeatDrop {
    pub provider: Option<String>,
    pub room_id: Option<String>,
    pub nonce: Option<String>,
    pub reason: String,
}

#[derive(Debug)]
pub struct HeartbeatReplayCache {
    capacity: usize,
    seen: HashSet<String>,
    order: VecDeque<String>,
}

#[derive(Debug)]
pub struct HeartbeatReceiver {
    replay_cache: HeartbeatReplayCache,
    drops: Vec<HeartbeatDrop>,
}

#[derive(Debug)]
pub struct HeartbeatValidationRequest<'a> {
    pub raw: &'a Value,
    pub now_millis: u64,
    pub replay_cache: &'a mut HeartbeatReplayCache,
    pub max_age_millis: u64,
    pub max_clock_skew_millis: u64,
}

impl EnclaveContractRecord {
    pub fn identity(&self) -> CatalogEnclaveIdentity {
        CatalogEnclaveIdentity {
            admin_pubkey: self.admin_pubkey.clone(),
            model_id: self.model_id.clone(),
            artifact_root: self.artifact_root.clone(),
            artifact_sidecar_roots: self.artifact_sidecar_roots.clone(),
            manifest_hash: self.manifest_hash.clone(),
            binary_hash: self.binary_hash.clone(),
        }
    }
}

impl<'a> AttestationVerificationRequest<'a> {
    pub fn new(
        report: &'a AttestationReport,
        contract: &'a EnclaveContractRecord,
        trusted_binary_hashes: &'a BTreeSet<String>,
        expected_nonce: &'a str,
        expected_provider_pubkey: &'a str,
        now_ts: u64,
    ) -> Self {
        Self {
            report,
            contract,
            trusted_binary_hashes,
            expected_nonce,
            expected_provider_pubkey,
            now_ts,
            max_report_age_secs: DEFAULT_MAX_REPORT_AGE_SECS,
            max_report_clock_skew_secs: DEFAULT_MAX_REPORT_CLOCK_SKEW_SECS,
            trusted_apple_app_attest_jwks: None,
            trusted_nvidia_gb10_device_jwks: None,
            trusted_nvidia_nras_jwks: None,
            trusted_nvidia_offline_jwks: None,
            hardware_quote_verifier_command: None,
        }
    }
}

impl Default for HeartbeatReplayCache {
    fn default() -> Self {
        Self::new(DEFAULT_HEARTBEAT_REPLAY_CACHE_CAPACITY)
    }
}

impl HeartbeatReplayCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            seen: HashSet::new(),
            order: VecDeque::new(),
        }
    }

    pub fn remember(&mut self, nonce: &str) -> bool {
        if self.seen.contains(nonce) {
            return false;
        }
        let nonce = nonce.to_owned();
        self.seen.insert(nonce.clone());
        self.order.push_back(nonce);
        while self.order.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.seen.remove(&oldest);
            }
        }
        true
    }
}

impl Default for HeartbeatReceiver {
    fn default() -> Self {
        Self::new()
    }
}

impl HeartbeatReceiver {
    pub fn new() -> Self {
        Self {
            replay_cache: HeartbeatReplayCache::default(),
            drops: Vec::new(),
        }
    }

    pub fn receive(&mut self, raw: &Value, now_millis: u64) -> Option<ProviderHeartbeat> {
        match validate_provider_heartbeat(&mut HeartbeatValidationRequest {
            raw,
            now_millis,
            replay_cache: &mut self.replay_cache,
            max_age_millis: DEFAULT_HEARTBEAT_MAX_AGE_MILLIS,
            max_clock_skew_millis: DEFAULT_HEARTBEAT_MAX_CLOCK_SKEW_MILLIS,
        }) {
            Ok(heartbeat) => Some(heartbeat),
            Err(err) => {
                self.drops.push(HeartbeatDrop {
                    provider: raw
                        .get("provider")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    room_id: raw
                        .get("room_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    nonce: raw.get("nonce").and_then(Value::as_str).map(str::to_owned),
                    reason: err.to_string(),
                });
                None
            }
        }
    }

    pub fn drops(&self) -> &[HeartbeatDrop] {
        &self.drops
    }
}

pub fn validate_provider_heartbeat(
    request: &mut HeartbeatValidationRequest<'_>,
) -> Result<ProviderHeartbeat> {
    let heartbeat: ProviderHeartbeat = serde_json::from_value(request.raw.clone())
        .map_err(|err| GatewayError::HeartbeatJson(err.to_string()))?;
    if heartbeat.t != "hb" || heartbeat.v != HEARTBEAT_SCHEMA_VERSION {
        return Err(GatewayError::BadHeartbeatSchema {
            expected_version: HEARTBEAT_SCHEMA_VERSION,
        });
    }
    if heartbeat.contract_version != CONTRACT_VERSION {
        return Err(GatewayError::ContractUpgradeRequired {
            expected: CONTRACT_VERSION,
            actual: heartbeat.contract_version,
        });
    }

    validate_heartbeat_fields(&heartbeat)?;
    validate_heartbeat_time(
        heartbeat.ts,
        request.now_millis,
        request.max_age_millis,
        request.max_clock_skew_millis,
    )?;
    verify_heartbeat_signature(request.raw, &heartbeat.provider, &heartbeat.sig)?;

    if !request.replay_cache.remember(&heartbeat.nonce) {
        return Err(GatewayError::HeartbeatReplay {
            nonce: heartbeat.nonce,
        });
    }

    Ok(heartbeat)
}

pub fn verify_tier1_attestation(
    request: &AttestationVerificationRequest<'_>,
) -> Result<VerifiedAttestation> {
    let report = request.report;
    if report.schema_version != ATTESTATION_SCHEMA_VERSION {
        return Err(GatewayError::BadSchemaVersion {
            expected: ATTESTATION_SCHEMA_VERSION,
            actual: report.schema_version,
        });
    }
    if report.alg != ATTESTATION_ALG {
        return Err(GatewayError::BadAlgorithm {
            expected: ATTESTATION_ALG,
            actual: report.alg.clone(),
        });
    }

    validate_hex_nonce(request.expected_nonce)?;
    validate_hex_nonce(&report.nonce_u)?;
    if report.nonce_u != request.expected_nonce {
        return Err(GatewayError::NonceMismatch {
            expected: request.expected_nonce.to_owned(),
            actual: report.nonce_u.clone(),
        });
    }
    compare_field(
        "provider_pubkey",
        request.expected_provider_pubkey,
        &report.provider_pubkey,
    )?;

    compare_field(
        "manifest_hash",
        &request.contract.manifest_hash,
        &report.manifest_hash,
    )?;
    compare_field(
        "binary_hash",
        &request.contract.binary_hash,
        &report.binary_hash,
    )?;
    compare_field(
        "att_tier",
        &request.contract.att_tier.to_string(),
        &report.att_tier.to_string(),
    )?;
    let expected_model_class = if request.contract.model_class.trim().is_empty() {
        DEFAULT_MODEL_CLASS
    } else {
        request.contract.model_class.as_str()
    };
    compare_field(
        "runtime_config.model_class",
        expected_model_class,
        &report.runtime_config.model_class,
    )?;
    compare_field(
        "runtime_config.tp_degree",
        &expected_tp_degree(&request.contract.caps).to_string(),
        &report.runtime_config.tp_degree.to_string(),
    )?;

    let expected_enclave_id = catalog_enclave_id(&request.contract.identity());
    if request.contract.enclave_id != expected_enclave_id {
        return Err(GatewayError::EnclaveIdMismatch {
            expected: expected_enclave_id,
            actual: request.contract.enclave_id.clone(),
        });
    }
    if report.enclave_id != request.contract.enclave_id {
        return Err(GatewayError::EnclaveIdMismatch {
            expected: request.contract.enclave_id.clone(),
            actual: report.enclave_id.clone(),
        });
    }

    if !request
        .trusted_binary_hashes
        .iter()
        .any(|trusted| trusted.eq_ignore_ascii_case(&report.binary_hash))
    {
        return Err(GatewayError::BinaryHashNotTrusted {
            binary_hash: report.binary_hash.clone(),
        });
    }

    validate_report_time(
        report.report_ts,
        request.now_ts,
        request.max_report_age_secs,
        request.max_report_clock_skew_secs,
    )?;
    verify_hardware_quote(request)?;
    verify_report_signature(report, AttestationSigner::Enclave)?;
    verify_report_signature(report, AttestationSigner::Provider)?;

    let report_head =
        attestation_report_head(report).map_err(|err| GatewayError::ReportHash(err.to_string()))?;
    Ok(VerifiedAttestation {
        enclave_id: report.enclave_id.clone(),
        provider_pubkey: report.provider_pubkey.clone(),
        enclave_pubkey: report.enclave_pubkey.clone(),
        report_head,
        boot_epoch: report.boot_epoch,
        report_ts: report.report_ts,
        att_tier: report.att_tier,
    })
}

pub fn verify_attestation(
    request: &AttestationVerificationRequest<'_>,
) -> Result<VerifiedAttestation> {
    verify_tier1_attestation(request)
}

fn verify_hardware_quote(request: &AttestationVerificationRequest<'_>) -> Result<()> {
    if request.contract.att_tier < 2 {
        return Ok(());
    }
    let quote = request
        .report
        .hw_quote
        .as_ref()
        .ok_or(GatewayError::HardwareQuoteRequired)?;
    if quote.evidence.is_empty() {
        return Err(GatewayError::HardwareQuoteEvidenceMissing);
    }
    let expected_quote_tier = quote.kind.attestation_tier();
    if request.report.att_tier != expected_quote_tier {
        return Err(GatewayError::ContractMismatch {
            field: "hw_quote.att_tier",
            expected: expected_quote_tier.to_string(),
            actual: request.report.att_tier.to_string(),
        });
    }
    let expected = hardware_quote_binding(&request.report.body())
        .map_err(|err| GatewayError::SigningPayload(err.to_string()))?;
    if quote.binding != expected {
        return Err(GatewayError::HardwareQuoteBindingMismatch {
            expected,
            actual: quote.binding.clone(),
        });
    }
    let kind = hardware_quote_kind_name(&quote.kind);
    if quote.kind.attestation_tier() >= 3 && request.hardware_quote_verifier_command.is_none() {
        return Err(hardware_quote_invalid(
            kind,
            "Tier-3 confidential compute requires an admin hardware quote verifier command that checks GPU/CPU roots and golden launch measurements",
        ));
    }
    if matches!(quote.kind, HardwareQuoteKind::Tpm2QuoteEk)
        && request.hardware_quote_verifier_command.is_none()
    {
        return Err(hardware_quote_invalid(
            kind,
            "TPM 2.0 EK quotes require an admin hardware quote verifier command that validates the EK certificate chain, TPM quote signature, nonce binding, and EK device key",
        ));
    }
    if let Some(verifier) = request.hardware_quote_verifier_command {
        return verify_hardware_quote_with_command(request, quote, &expected, verifier);
    }
    match quote.kind {
        HardwareQuoteKind::AppleAppAttestJwt => verify_apple_app_attest_quote(
            &quote.evidence,
            request.trusted_apple_app_attest_jwks,
            &expected,
            request.report,
        ),
        HardwareQuoteKind::AmdSevSnpVcek => Err(GatewayError::HardwareQuoteUnsupported {
            kind: "amd_sev_snp_vcek".to_owned(),
        }),
        HardwareQuoteKind::IntelTdxDcap => Err(GatewayError::HardwareQuoteUnsupported {
            kind: "intel_tdx_dcap".to_owned(),
        }),
        HardwareQuoteKind::NvidiaGb10DeviceJwt => verify_nvidia_gb10_device_quote(
            &quote.evidence,
            request.trusted_nvidia_gb10_device_jwks,
            &expected,
            request.report,
        ),
        HardwareQuoteKind::NvidiaNrasJwt => {
            verify_nvidia_nras_quote(&quote.evidence, request.trusted_nvidia_nras_jwks, &expected)
        }
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt => verify_nvidia_nvtrust_offline_quote(
            &quote.evidence,
            request.trusted_nvidia_offline_jwks,
            &expected,
        ),
        HardwareQuoteKind::Tpm2QuoteEk => Err(GatewayError::HardwareQuoteUnsupported {
            kind: "tpm2_quote_ek".to_owned(),
        }),
    }
}

#[derive(Debug, Deserialize)]
struct HardwareQuoteVerifierCommandOutput {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    binding: Option<String>,
    #[serde(default)]
    att_tier: Option<u8>,
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default)]
    verified_roots: Vec<String>,
    #[serde(default)]
    matched_measurement_name: Option<String>,
    #[serde(default)]
    matched_measurement: Option<String>,
    #[serde(default)]
    matched_measurements: Value,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    platform_id: Option<String>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    unknown_measurement: Option<String>,
    #[serde(default)]
    snp_chip_family: Option<String>,
    #[serde(default)]
    snp_chip_id: Option<String>,
    #[serde(default)]
    snp_tcb: Option<String>,
    #[serde(default)]
    snp_firmware_svn: Option<String>,
    #[serde(default)]
    gpu_model: Option<String>,
    #[serde(default)]
    gpu_driver: Option<String>,
    #[serde(default)]
    gpu_vbios: Option<String>,
    #[serde(default)]
    device_key: Option<String>,
    #[serde(default)]
    alert: Value,
}

fn verify_hardware_quote_with_command(
    request: &AttestationVerificationRequest<'_>,
    quote: &HardwareQuote,
    expected_binding: &str,
    verifier: &HardwareQuoteVerifierCommand,
) -> Result<()> {
    const KIND: &str = "external_verifier";
    if verifier.timeout.is_zero() {
        return Err(hardware_quote_invalid(
            KIND,
            "admin verifier timeout must be positive",
        ));
    }
    let kind = hardware_quote_kind_name(&quote.kind);
    let golden_measurements = golden_tier3_measurements(&request.contract.launch_measurements);
    let declared_platform = if quote.kind.attestation_tier() >= 3 {
        let platform = tier3_quote_platform_id(kind, quote)?;
        require_tier3_workload_measurements(kind, &golden_measurements)?;
        Some(platform.to_owned())
    } else {
        None
    };
    let input = json!({
        "schema_version": 1,
        "kind": kind,
        "expected_binding": expected_binding,
        "declared_platform": declared_platform.as_deref(),
        "golden_measurement_layers": &golden_measurements,
        "quote": quote,
        "report": request.report,
        "contract": {
            "enclave_id": &request.contract.enclave_id,
            "admin_pubkey": &request.contract.admin_pubkey,
            "model_id": &request.contract.model_id,
            "model_class": &request.contract.model_class,
            "artifact_root": &request.contract.artifact_root,
            "artifact_sidecar_roots": &request.contract.artifact_sidecar_roots,
            "manifest_hash": &request.contract.manifest_hash,
            "binary_hash": &request.contract.binary_hash,
            "launch_measurements": &request.contract.launch_measurements,
            "att_tier": request.contract.att_tier,
            "caps": &request.contract.caps,
        }
    });
    let mut input = serde_json::to_vec(&input).map_err(|err| {
        hardware_quote_invalid(KIND, format!("admin verifier input JSON failed: {err}"))
    })?;
    input.push(b'\n');
    let mut child = Command::new(&verifier.command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("MAYHEM_HW_VERIFY_KIND", kind)
        .env("MAYHEM_HW_VERIFY_BINDING", expected_binding)
        .env(
            "MAYHEM_HW_VERIFY_PLATFORM",
            declared_platform.as_deref().unwrap_or(""),
        )
        .env(
            "MAYHEM_HW_VERIFY_ATTESTATION_TIER",
            quote.kind.attestation_tier().to_string(),
        )
        .spawn()
        .map_err(|err| {
            hardware_quote_invalid(
                KIND,
                format!(
                    "admin verifier {} could not start: {err}",
                    verifier.command.display()
                ),
            )
        })?;
    let write_result = child
        .stdin
        .as_mut()
        .ok_or_else(|| hardware_quote_invalid(KIND, "admin verifier stdin was not available"))
        .and_then(|stdin| {
            stdin.write_all(&input).map_err(|err| {
                hardware_quote_invalid(KIND, format!("admin verifier stdin write failed: {err}"))
            })
        });
    drop(child.stdin.take());
    if let Err(err) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(err);
    }
    let status = match child.wait_timeout(verifier.timeout) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(hardware_quote_invalid(
                KIND,
                format!(
                    "admin verifier {} timed out after {}s",
                    verifier.command.display(),
                    verifier.timeout.as_secs()
                ),
            ));
        }
        Err(err) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(hardware_quote_invalid(
                KIND,
                format!("admin verifier wait failed: {err}"),
            ));
        }
    };
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_string(&mut stdout).map_err(|err| {
            hardware_quote_invalid(KIND, format!("admin verifier stdout read failed: {err}"))
        })?;
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_string(&mut stderr).map_err(|err| {
            hardware_quote_invalid(KIND, format!("admin verifier stderr read failed: {err}"))
        })?;
    }
    if !status.success() {
        return Err(hardware_quote_invalid(
            kind,
            format!(
                "admin verifier exited with {status}; stderr: {}",
                short_verifier_stream(&stderr)
            ),
        ));
    }
    let verdict: HardwareQuoteVerifierCommandOutput =
        serde_json::from_str(stdout.trim()).map_err(|err| {
            hardware_quote_invalid(
                kind,
                format!("admin verifier stdout was not verdict JSON: {err}"),
            )
        })?;
    if !verdict.ok {
        return Err(hardware_quote_invalid(
            kind,
            format!(
                "admin verifier rejected quote: {}{}",
                verdict.reason.as_deref().unwrap_or("no reason supplied"),
                tier3_alert_suffix(&verdict, &quote.metadata, None)
            ),
        ));
    }
    if verdict.kind.as_deref().is_some_and(|actual| actual != kind) {
        return Err(hardware_quote_invalid(
            kind,
            format!(
                "admin verifier returned kind {}, expected {kind}",
                verdict.kind.as_deref().unwrap_or_default()
            ),
        ));
    }
    if verdict
        .binding
        .as_deref()
        .is_some_and(|actual| actual != expected_binding)
    {
        return Err(GatewayError::HardwareQuoteBindingMismatch {
            expected: expected_binding.to_owned(),
            actual: verdict.binding.unwrap_or_default(),
        });
    }
    if verdict
        .att_tier
        .is_some_and(|actual| actual != quote.kind.attestation_tier())
    {
        return Err(GatewayError::ContractMismatch {
            field: "hw_quote.verifier.att_tier",
            expected: quote.kind.attestation_tier().to_string(),
            actual: verdict.att_tier.unwrap_or_default().to_string(),
        });
    }
    let roots = verifier_verified_roots(&verdict);
    if roots.is_empty() {
        return Err(hardware_quote_invalid(
            kind,
            "admin verifier did not name any verified vendor root",
        ));
    }
    if !verifier_roots_satisfy_quote_kind(&quote.kind, &roots, declared_platform.as_deref()) {
        return Err(hardware_quote_invalid(
            kind,
            format!(
                "admin verifier roots {:?} do not satisfy {kind}",
                verdict_roots_for_error(&roots)
            ),
        ));
    }
    if matches!(quote.kind, HardwareQuoteKind::Tpm2QuoteEk) {
        verify_tpm2_verdict_device_key(kind, quote, &verdict)?;
    }
    if quote.kind.attestation_tier() >= 3 {
        verify_verdict_matches_golden_measurement(
            kind,
            &verdict,
            &golden_measurements,
            &quote.metadata,
        )?;
    }
    Ok(())
}

fn tier3_quote_platform_id<'a>(kind: &'static str, quote: &'a HardwareQuote) -> Result<&'a str> {
    let platform = quote
        .metadata
        .get("platform_id")
        .or_else(|| quote.metadata.get("platform"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            hardware_quote_invalid(kind, "Tier-3 quote metadata must carry platform_id")
        })?;
    if !platform
        .as_bytes()
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(hardware_quote_invalid(
            kind,
            "Tier-3 quote metadata platform_id is not a safe label",
        ));
    }
    Ok(platform)
}

fn golden_tier3_measurements(value: &Value) -> GoldenTier3Measurements {
    fn insert_measurement(
        out: &mut GoldenTier3Measurements,
        layer: &str,
        name: &str,
        measurement: &str,
    ) {
        let layer = normalize_tier3_measurement_layer(layer);
        let name = normalize_verifier_root(name);
        let measurement = measurement.trim().to_ascii_lowercase();
        if !layer.is_empty() && !name.is_empty() && !measurement.is_empty() {
            out.entry(layer)
                .or_default()
                .entry(name)
                .or_default()
                .insert(measurement);
        }
    }

    fn collect_value(out: &mut GoldenTier3Measurements, layer: &str, name: &str, value: &Value) {
        if let Some(measurement) = value.as_str() {
            insert_measurement(out, layer, name, measurement);
            return;
        }
        if let Some(values) = value.as_array() {
            for value in values {
                collect_value(out, layer, name, value);
            }
            return;
        }
        if let Some(object) = value.as_object() {
            if let Some(measurement) = object.get("measurement").and_then(Value::as_str) {
                insert_measurement(out, layer, name, measurement);
            }
            if let Some(values) = object.get("values") {
                collect_value(out, layer, name, values);
            }
        }
    }

    fn collect_measurement_map(out: &mut GoldenTier3Measurements, layer: &str, value: &Value) {
        let measurements = value
            .get("measurements")
            .and_then(Value::as_object)
            .or_else(|| value.as_object());
        if let Some(measurements) = measurements {
            for (name, value) in measurements {
                if matches!(
                    name.as_str(),
                    "schema_version" | "effective_epoch" | "platform" | "entries" | "layers"
                ) {
                    continue;
                }
                collect_value(out, layer, name, value);
            }
        }
    }

    let mut out = BTreeMap::new();
    if let Some(layers) = value.get("layers").and_then(Value::as_object) {
        for (layer, layer_value) in layers {
            collect_measurement_map(&mut out, layer, layer_value);
        }
    } else {
        collect_measurement_map(&mut out, TIER3_WORKLOAD_LAYER, value);
    }
    if let Some(entries) = value.get("entries").and_then(Value::as_array) {
        for entry in entries {
            let Some(object) = entry.as_object() else {
                continue;
            };
            let Some(measurement) = object.get("measurement").and_then(Value::as_str) else {
                continue;
            };
            let layer = object
                .get("layer")
                .and_then(Value::as_str)
                .unwrap_or(TIER3_WORKLOAD_LAYER);
            let name = object
                .get("measurement_name")
                .or_else(|| object.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("measurement");
            insert_measurement(&mut out, layer, name, measurement);
        }
    }
    out.retain(|_, measurements| {
        measurements.retain(|_, values| !values.is_empty());
        !measurements.is_empty()
    });
    out
}

fn require_tier3_workload_measurements(
    kind: &'static str,
    golden: &GoldenTier3Measurements,
) -> Result<()> {
    if golden.is_empty() {
        return Err(hardware_quote_invalid(
            kind,
            "Tier-3 enclave has no admin-published measurement layers",
        ));
    }
    if !golden
        .get(TIER3_WORKLOAD_LAYER)
        .is_some_and(|measurements| !measurements.is_empty())
    {
        return Err(hardware_quote_invalid(
            kind,
            "Tier-3 enclave has no admin-published workload PCR/stack measurements",
        ));
    }
    Ok(())
}

fn normalize_tier3_measurement_layer(layer: &str) -> String {
    normalize_verifier_root(layer)
}

fn split_tier3_measurement_name<'a>(name: &'a str) -> (String, &'a str) {
    for (prefix, layer) in [
        ("vendor.", TIER3_VENDOR_LAYER),
        ("vendor:", TIER3_VENDOR_LAYER),
        ("workload.", TIER3_WORKLOAD_LAYER),
        ("workload:", TIER3_WORKLOAD_LAYER),
    ] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return (layer.to_owned(), rest);
        }
    }
    (TIER3_WORKLOAD_LAYER.to_owned(), name)
}

fn insert_matched_measurement(
    matched: &mut MatchedTier3Measurements,
    layer: &str,
    name: &str,
    value: &str,
) {
    let layer = normalize_tier3_measurement_layer(layer);
    let name = normalize_verifier_root(name);
    let value = value.trim().to_ascii_lowercase();
    if !layer.is_empty() && !name.is_empty() && !value.is_empty() {
        matched.entry(layer).or_default().insert(name, value);
    }
}

fn collect_matched_measurements_value(
    matched: &mut MatchedTier3Measurements,
    default_layer: &str,
    name: &str,
    value: &Value,
) {
    if let Some(value) = value.as_str() {
        let (layer, name) = split_tier3_measurement_name(name);
        let layer = if default_layer == TIER3_WORKLOAD_LAYER {
            layer
        } else {
            default_layer.to_owned()
        };
        insert_matched_measurement(matched, &layer, name, value);
        return;
    }
    if let Some(object) = value.as_object() {
        if let Some(measurement) = object.get("measurement").and_then(Value::as_str) {
            let layer = object
                .get("layer")
                .and_then(Value::as_str)
                .unwrap_or(default_layer);
            let name = object
                .get("measurement_name")
                .or_else(|| object.get("name"))
                .and_then(Value::as_str)
                .unwrap_or(name);
            insert_matched_measurement(matched, layer, name, measurement);
        }
        if let Some(values) = object.get("values").and_then(Value::as_array) {
            for value in values {
                collect_matched_measurements_value(matched, default_layer, name, value);
            }
        }
    }
}

fn verify_verdict_matches_golden_measurement(
    kind: &'static str,
    verdict: &HardwareQuoteVerifierCommandOutput,
    golden: &GoldenTier3Measurements,
    quote_metadata: &Value,
) -> Result<()> {
    let matched = verifier_matched_measurements(verdict);
    if matched.is_empty() {
        return Err(hardware_quote_invalid(
            kind,
            "admin verifier did not report matched Tier-3 measurements",
        ));
    }
    for (layer, required) in golden {
        if required.is_empty() {
            continue;
        }
        let Some(actual) = matched.get(layer) else {
            return Err(hardware_quote_invalid(
                kind,
                format!(
                    "hardware quote did not report a matched {layer} measurement{}",
                    tier3_alert_suffix(verdict, quote_metadata, Some(&matched))
                ),
            ));
        };
        if !tier3_layer_has_measurement_match(required, actual) {
            let label = if layer == TIER3_WORKLOAD_LAYER {
                "workload PCR/stack"
            } else {
                layer
            };
            return Err(hardware_quote_invalid(
                kind,
                format!(
                    "hardware quote {label} measurement does not match the admin-published golden set{}",
                    tier3_alert_suffix(verdict, quote_metadata, Some(&matched))
                ),
            ));
        }
    }
    if !matched
        .get(TIER3_WORKLOAD_LAYER)
        .is_some_and(|values| !values.is_empty())
    {
        return Err(hardware_quote_invalid(
            kind,
            "admin verifier did not report a matched workload PCR/stack measurement",
        ));
    }
    Ok(())
}

fn tier3_layer_has_measurement_match(
    required: &BTreeMap<String, BTreeSet<String>>,
    actual: &BTreeMap<String, String>,
) -> bool {
    let value_matches_any =
        |measurement: &str| required.values().any(|values| values.contains(measurement));
    for (name, measurement) in actual {
        if required
            .get(name)
            .is_some_and(|values| values.contains(measurement))
            || (name == "measurement" && value_matches_any(measurement))
        {
            return true;
        }
    }
    false
}

fn verifier_matched_measurements(
    verdict: &HardwareQuoteVerifierCommandOutput,
) -> MatchedTier3Measurements {
    let mut matched = BTreeMap::new();
    if let Some(value) = verdict.matched_measurement.as_deref() {
        let raw_name = verdict
            .matched_measurement_name
            .as_deref()
            .unwrap_or("measurement");
        let (layer, name) = split_tier3_measurement_name(raw_name);
        insert_matched_measurement(&mut matched, &layer, name, value);
    }
    if let Some(object) = verdict.matched_measurements.as_object() {
        for (name, value) in object {
            let normalized = normalize_tier3_measurement_layer(name);
            if matches!(
                normalized.as_str(),
                TIER3_VENDOR_LAYER | TIER3_WORKLOAD_LAYER
            ) {
                if let Some(layer_object) = value.as_object() {
                    for (measurement_name, measurement_value) in layer_object {
                        collect_matched_measurements_value(
                            &mut matched,
                            &normalized,
                            measurement_name,
                            measurement_value,
                        );
                    }
                    continue;
                }
            }
            collect_matched_measurements_value(&mut matched, TIER3_WORKLOAD_LAYER, name, value);
        }
    }
    matched.retain(|_, values| {
        values.retain(|_, value| !value.is_empty());
        !values.is_empty()
    });
    matched
}

fn tier3_alert_suffix(
    verdict: &HardwareQuoteVerifierCommandOutput,
    quote_metadata: &Value,
    matched: Option<&MatchedTier3Measurements>,
) -> String {
    let mut parts = Vec::new();
    push_alert_part(
        &mut parts,
        "platform",
        verifier_string(verdict.platform_id.as_deref())
            .or_else(|| verifier_string(verdict.platform.as_deref()))
            .or_else(|| json_string_path(&verdict.alert, &["platform_id"]))
            .or_else(|| json_string_path(&verdict.alert, &["platform"]))
            .or_else(|| json_string_path(quote_metadata, &["platform_id"]))
            .or_else(|| json_string_path(quote_metadata, &["platform"]))
            .as_deref(),
    );
    push_alert_part(
        &mut parts,
        "region",
        verifier_string(verdict.region.as_deref())
            .or_else(|| json_string_path(&verdict.alert, &["region"]))
            .or_else(|| json_string_path(quote_metadata, &["region"]))
            .as_deref(),
    );
    push_alert_part(
        &mut parts,
        "unknown_measurement",
        verifier_string(verdict.unknown_measurement.as_deref())
            .or_else(|| json_string_path(&verdict.alert, &["unknown_measurement"]))
            .or_else(|| matched.and_then(first_matched_tier3_measurement))
            .as_deref(),
    );
    push_alert_part(
        &mut parts,
        "snp_chip_family",
        verifier_string(verdict.snp_chip_family.as_deref())
            .or_else(|| json_string_path(&verdict.alert, &["snp", "chip_family"]))
            .or_else(|| json_string_path(quote_metadata, &["snp", "chip_family"]))
            .as_deref(),
    );
    push_alert_part(
        &mut parts,
        "snp_chip_id",
        verifier_string(verdict.snp_chip_id.as_deref())
            .or_else(|| json_string_path(&verdict.alert, &["snp", "chip_id"]))
            .or_else(|| json_string_path(quote_metadata, &["snp", "chip_id"]))
            .as_deref(),
    );
    push_alert_part(
        &mut parts,
        "snp_tcb",
        verifier_string(verdict.snp_tcb.as_deref())
            .or_else(|| json_string_path(&verdict.alert, &["snp", "tcb"]))
            .or_else(|| json_string_path(quote_metadata, &["snp", "tcb"]))
            .as_deref(),
    );
    push_alert_part(
        &mut parts,
        "snp_firmware_svn",
        verifier_string(verdict.snp_firmware_svn.as_deref())
            .or_else(|| json_string_path(&verdict.alert, &["snp", "firmware_svn"]))
            .or_else(|| json_string_path(quote_metadata, &["snp", "firmware_svn"]))
            .as_deref(),
    );
    push_alert_part(
        &mut parts,
        "gpu_model",
        verifier_string(verdict.gpu_model.as_deref())
            .or_else(|| json_string_path(&verdict.alert, &["gpu", "model"]))
            .or_else(|| json_string_path(quote_metadata, &["gpu", "model"]))
            .as_deref(),
    );
    push_alert_part(
        &mut parts,
        "gpu_driver",
        verifier_string(verdict.gpu_driver.as_deref())
            .or_else(|| json_string_path(&verdict.alert, &["gpu", "driver"]))
            .or_else(|| json_string_path(quote_metadata, &["gpu", "driver"]))
            .as_deref(),
    );
    push_alert_part(
        &mut parts,
        "gpu_vbios",
        verifier_string(verdict.gpu_vbios.as_deref())
            .or_else(|| json_string_path(&verdict.alert, &["gpu", "vbios"]))
            .or_else(|| json_string_path(quote_metadata, &["gpu", "vbios"]))
            .as_deref(),
    );
    if parts.is_empty() {
        String::new()
    } else {
        format!("; Tier-3 measurement alert: {}", parts.join(", "))
    }
}

fn push_alert_part(parts: &mut Vec<String>, name: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        parts.push(format!("{name}={value}"));
    }
}

fn verifier_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn json_string_path(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            current
                .as_u64()
                .map(|value| value.to_string())
                .or_else(|| current.as_i64().map(|value| value.to_string()))
        })
}

fn first_matched_tier3_measurement(matched: &MatchedTier3Measurements) -> Option<String> {
    matched
        .values()
        .find_map(|measurements| measurements.values().next().cloned())
}

fn hardware_quote_kind_name(kind: &HardwareQuoteKind) -> &'static str {
    match kind {
        HardwareQuoteKind::AppleAppAttestJwt => "apple_app_attest_jwt",
        HardwareQuoteKind::AmdSevSnpVcek => "amd_sev_snp_vcek",
        HardwareQuoteKind::IntelTdxDcap => "intel_tdx_dcap",
        HardwareQuoteKind::NvidiaGb10DeviceJwt => "nvidia_gb10_device_jwt",
        HardwareQuoteKind::NvidiaNrasJwt => "nvidia_nras_jwt",
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt => "nvidia_nvtrust_offline_jwt",
        HardwareQuoteKind::Tpm2QuoteEk => "tpm2_quote_ek",
    }
}

fn verifier_verified_roots(verdict: &HardwareQuoteVerifierCommandOutput) -> Vec<String> {
    let mut roots = verdict
        .root
        .iter()
        .chain(verdict.roots.iter())
        .chain(verdict.verified_roots.iter())
        .map(|root| normalize_verifier_root(root))
        .filter(|root| !root.is_empty())
        .collect::<Vec<_>>();
    roots.sort();
    roots.dedup();
    roots
}

fn normalize_verifier_root(root: &str) -> String {
    root.trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

fn verifier_roots_satisfy_quote_kind(
    kind: &HardwareQuoteKind,
    roots: &[String],
    declared_platform: Option<&str>,
) -> bool {
    match kind {
        HardwareQuoteKind::AppleAppAttestJwt => root_has_all(roots, &["apple", "app", "attest"]),
        HardwareQuoteKind::AmdSevSnpVcek => has_amd_sev_snp_vcek_root(roots),
        HardwareQuoteKind::IntelTdxDcap => has_intel_tdx_dcap_root(roots),
        HardwareQuoteKind::NvidiaGb10DeviceJwt => {
            root_has_all(roots, &["nvidia", "gb10"])
                || root_has_all(roots, &["nvidia", "device", "identity"])
        }
        HardwareQuoteKind::Tpm2QuoteEk => has_tpm2_ek_root(roots),
        HardwareQuoteKind::NvidiaNrasJwt => {
            root_has_all(roots, &["nvidia", "nras"])
                || (has_raw_nvidia_gpu_roots(roots)
                    && has_confidential_cpu_root(roots, declared_platform))
        }
        HardwareQuoteKind::NvidiaNvtrustOfflineJwt => {
            has_raw_nvidia_gpu_roots(roots) && has_confidential_cpu_root(roots, declared_platform)
        }
    }
}

fn has_tpm2_ek_root(roots: &[String]) -> bool {
    root_has_all(roots, &["tpm2", "ek"])
        || root_has_all(roots, &["tpm", "ek", "cert"])
        || root_has_all(roots, &["tpm", "endorsement", "key"])
        || root_has_all(roots, &["tpm", "manufacturer"])
}

fn has_confidential_cpu_root(roots: &[String], declared_platform: Option<&str>) -> bool {
    has_amd_sev_snp_vcek_root(roots)
        || has_intel_tdx_dcap_root(roots)
        || has_azure_maa_cpu_root(roots, declared_platform)
}

fn has_amd_sev_snp_vcek_root(roots: &[String]) -> bool {
    root_has_all(roots, &["amd", "sev", "snp", "vcek"])
        || root_has_all(roots, &["amd", "ark", "genoa"])
}

fn has_intel_tdx_dcap_root(roots: &[String]) -> bool {
    root_has_all(roots, &["intel", "tdx", "dcap"])
}

fn has_azure_maa_cpu_root(roots: &[String], declared_platform: Option<&str>) -> bool {
    let Some(platform) = declared_platform.map(normalize_verifier_root) else {
        return false;
    };
    if !(platform == "azure_ncc" || platform.starts_with("azure_")) {
        return false;
    }
    root_has_all(
        roots,
        &["azure", "maa", "jwt", "jwks", "issuer", "nonce", "claims"],
    )
}

fn has_raw_nvidia_gpu_roots(roots: &[String]) -> bool {
    (root_has_all(roots, &["nvidia", "gpu", "cert", "chain"])
        || root_has_all(roots, &["nvidia", "gpu", "attestation", "report"]))
        && root_has_all(roots, &["nvidia", "driver", "rim"])
        && root_has_all(roots, &["nvidia", "vbios", "rim"])
}

fn root_has_all(roots: &[String], parts: &[&str]) -> bool {
    roots
        .iter()
        .any(|root| parts.iter().all(|part| root.contains(part)))
}

fn verdict_roots_for_error(roots: &[String]) -> Vec<&str> {
    roots.iter().map(String::as_str).take(8).collect()
}

fn is_hex_len(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn short_verifier_stream(stream: &str) -> String {
    let stream = stream.trim();
    if stream.len() <= 512 {
        stream.to_owned()
    } else {
        format!("{}...", stream.chars().take(512).collect::<String>())
    }
}

fn verify_apple_app_attest_quote(
    evidence: &str,
    trusted_jwks: Option<&Value>,
    expected_binding: &str,
    report: &AttestationReport,
) -> Result<()> {
    const KIND: &str = "apple_app_attest_jwt";
    let claims =
        decode_vendor_identity_jwt(evidence, trusted_jwks, KIND, &[APPLE_APP_ATTEST_ISSUER])?;
    require_string_claim_ci_for_kind(&claims, "sub", "APPLE-APP-ATTEST", KIND)?;
    require_string_claim_ci_for_kind(
        &claims,
        "x-mayhem-attestation-mechanism",
        "apple_app_attest",
        KIND,
    )?;
    require_claim_matches_for_kind(&claims, "eat_nonce", expected_binding, KIND)?;
    require_claim_matches_for_kind(&claims, "x-mayhem-enclave-id", &report.enclave_id, KIND)?;
    require_claim_matches_for_kind(&claims, "x-mayhem-binary-hash", &report.binary_hash, KIND)?;
    for field in [
        "x-apple-app-attest-root-verified",
        "x-apple-app-attest-app-id-bound",
        "x-apple-app-attest-client-hash-bound",
        "x-apple-app-attest-signature-verified",
        "x-apple-app-attest-counter-valid",
    ] {
        require_bool_claim_for_kind(&claims, field, KIND)?;
    }
    require_bool_claim_value_for_kind(&claims, "x-mayhem-prompt-confidentiality", false, KIND)?;
    Ok(())
}

fn verify_nvidia_gb10_device_quote(
    evidence: &str,
    trusted_jwks: Option<&Value>,
    expected_binding: &str,
    report: &AttestationReport,
) -> Result<()> {
    const KIND: &str = "nvidia_gb10_device_jwt";
    let claims = decode_vendor_identity_jwt(evidence, trusted_jwks, KIND, &[NVIDIA_NRAS_ISSUER])?;
    require_string_claim_ci_for_kind(&claims, "sub", "NVIDIA-GB10-DEVICE-ATTESTATION", KIND)?;
    require_string_claim_ci_for_kind(&claims, "x-nvidia-device-type", "gpu", KIND)?;
    require_claim_matches_for_kind(&claims, "eat_nonce", expected_binding, KIND)?;
    require_claim_matches_for_kind(&claims, "x-mayhem-enclave-id", &report.enclave_id, KIND)?;
    require_claim_matches_for_kind(&claims, "x-mayhem-binary-hash", &report.binary_hash, KIND)?;
    for field in [
        "x-nvidia-gpu-attestation-report-cert-chain-validated",
        "x-nvidia-gpu-attestation-report-parsed",
        "x-nvidia-gpu-attestation-report-signature-verified",
        "x-nvidia-gpu-driver-rim-signature-verified",
        "x-nvidia-gpu-vbios-rim-signature-verified",
        "x-nvidia-gpu-driver-rim-measurements-available",
        "x-nvidia-gpu-vbios-rim-measurements-available",
        "x-nvidia-gpu-nonce-match",
        "x-nvidia-gpu-arch-check",
    ] {
        require_bool_claim_for_kind(&claims, field, KIND)?;
    }
    require_gb10_device_model_claim(&claims)?;
    Ok(())
}

fn decode_vendor_identity_jwt(
    evidence: &str,
    trusted_jwks: Option<&Value>,
    kind: &'static str,
    issuers: &[&str],
) -> Result<Value> {
    let trusted_jwks = trusted_jwks.ok_or_else(|| GatewayError::HardwareQuoteTrustRootMissing {
        kind: kind.to_owned(),
    })?;
    let token = evidence.trim();
    if !looks_like_jwt(token) {
        return Err(hardware_quote_invalid(
            kind,
            "vendor identity evidence must be a compact JWT",
        ));
    }
    let jwks = parse_vendor_jwks(trusted_jwks, kind)?;
    let header = decode_header(token)
        .map_err(|err| hardware_quote_invalid(kind, format!("JWT header decode failed: {err}")))?;
    if header.alg != Algorithm::ES384 {
        return Err(hardware_quote_invalid(
            kind,
            "vendor identity JWT must use ES384",
        ));
    }
    let kid = header
        .kid
        .as_deref()
        .ok_or_else(|| hardware_quote_invalid(kind, "vendor identity JWT is missing kid"))?;
    let jwk = jwks.find(kid).ok_or_else(|| {
        hardware_quote_invalid(kind, "vendor identity JWT kid is not in trusted JWKS")
    })?;
    if jwk
        .common
        .key_algorithm
        .as_ref()
        .is_some_and(|alg| *alg != KeyAlgorithm::ES384)
    {
        return Err(hardware_quote_invalid(
            kind,
            "trusted vendor JWK declares a non-ES384 alg",
        ));
    }
    let decoding_key = DecodingKey::from_jwk(jwk).map_err(|err| {
        hardware_quote_invalid(kind, format!("trusted vendor JWK is invalid: {err}"))
    })?;
    let mut validation = Validation::new(Algorithm::ES384);
    validation.validate_aud = false;
    validation.set_issuer(issuers);
    validation.set_required_spec_claims(&["exp", "iss"]);
    decode::<Value>(token, &decoding_key, &validation)
        .map(|token| token.claims)
        .map_err(|err| {
            hardware_quote_invalid(
                kind,
                format!("vendor identity JWT verification failed: {err}"),
            )
        })
}

fn parse_vendor_jwks(value: &Value, kind: &'static str) -> Result<JwkSet> {
    let jwks_value = value.get("jwks").unwrap_or(value).clone();
    serde_json::from_value(jwks_value).map_err(|err| GatewayError::HardwareQuoteInvalid {
        kind: kind.to_owned(),
        reason: format!("trusted JWKS is invalid: {err}"),
    })
}

fn require_claim_matches_for_kind(
    claims: &Value,
    field: &'static str,
    expected: &str,
    kind: &'static str,
) -> Result<()> {
    if claims
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
    {
        Ok(())
    } else {
        Err(hardware_quote_invalid(
            kind,
            format!("{field} does not match the Mayhem quote binding"),
        ))
    }
}

fn require_bool_claim_for_kind(
    claims: &Value,
    field: &'static str,
    kind: &'static str,
) -> Result<()> {
    require_bool_claim_value_for_kind(claims, field, true, kind)
}

fn require_bool_claim_value_for_kind(
    claims: &Value,
    field: &'static str,
    expected: bool,
    kind: &'static str,
) -> Result<()> {
    if claims.get(field).and_then(Value::as_bool) == Some(expected) {
        Ok(())
    } else {
        Err(hardware_quote_invalid(
            kind,
            format!("{field} is not {expected}"),
        ))
    }
}

fn require_string_claim_ci_for_kind(
    claims: &Value,
    field: &'static str,
    expected: &'static str,
    kind: &'static str,
) -> Result<()> {
    if claims
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
    {
        Ok(())
    } else {
        Err(hardware_quote_invalid(
            kind,
            format!("{field} is not {expected}"),
        ))
    }
}

fn require_gb10_device_model_claim(claims: &Value) -> Result<()> {
    const KIND: &str = "nvidia_gb10_device_jwt";
    let model = ["hwmodel", "x-nvidia-gpu-model", "x-nvidia-gpu-product"]
        .iter()
        .find_map(|field| claims.get(*field).and_then(Value::as_str))
        .unwrap_or("");
    let model_lc = model.to_ascii_lowercase();
    if model_lc.contains("gb10") || model_lc.contains("dgx spark") {
        Ok(())
    } else {
        Err(hardware_quote_invalid(
            KIND,
            "NVIDIA device evidence is not for GB10/DGX Spark hardware",
        ))
    }
}

fn verify_tpm2_verdict_device_key(
    kind: &'static str,
    quote: &HardwareQuote,
    verdict: &HardwareQuoteVerifierCommandOutput,
) -> Result<()> {
    let verifier_device_key = verdict
        .device_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            hardware_quote_invalid(
                kind,
                "admin verifier must return the TPM EK device_key for ban enforcement",
            )
        })?;
    if !is_hex_len(verifier_device_key, 64) {
        return Err(hardware_quote_invalid(
            kind,
            "admin verifier returned a TPM EK device_key that is not a 32-byte hex digest",
        ));
    }
    let quote_device_key =
        hardware_quote_metadata_device_key(&quote.metadata).ok_or_else(|| {
            hardware_quote_invalid(
                kind,
                "TPM quote metadata must carry the EK device_key submitted to the contract",
            )
        })?;
    if !quote_device_key.eq_ignore_ascii_case(verifier_device_key) {
        return Err(hardware_quote_invalid(
            kind,
            "TPM EK device_key in quote metadata does not match the verifier-confirmed EK",
        ));
    }
    Ok(())
}

fn hardware_quote_metadata_device_key(metadata: &Value) -> Option<&str> {
    metadata
        .get("device_key")
        .or_else(|| metadata.get("ek_fingerprint"))
        .or_else(|| metadata.get("tpm_ek_sha256"))
        .and_then(Value::as_str)
        .or_else(|| {
            metadata
                .get("tpm")
                .and_then(|tpm| {
                    tpm.get("device_key")
                        .or_else(|| tpm.get("ek_fingerprint"))
                        .or_else(|| tpm.get("ek_sha256"))
                })
                .and_then(Value::as_str)
        })
}

fn verify_nvidia_nras_quote(
    evidence: &str,
    trusted_jwks: Option<&Value>,
    expected_nonce: &str,
) -> Result<()> {
    let trusted_jwks = trusted_jwks.ok_or_else(|| GatewayError::HardwareQuoteTrustRootMissing {
        kind: "nvidia_nras_jwt".to_owned(),
    })?;
    let jwks = parse_nvidia_jwks(trusted_jwks)?;
    let tokens = nvidia_eat_tokens(evidence)?;
    if tokens.is_empty() {
        return Err(nvidia_quote_invalid("no JWT found in NVIDIA NRAS evidence"));
    }

    let mut saw_gpu_success = false;
    let mut saw_overall_success = false;
    for token in tokens {
        let claims = decode_nvidia_nras_jwt(&token, &jwks)?;
        match nvidia_claim_class(&claims) {
            NvidiaClaimClass::Gpu => {
                validate_nvidia_gpu_claims(&claims, expected_nonce)?;
                saw_gpu_success = true;
            }
            NvidiaClaimClass::Overall => {
                validate_nvidia_overall_claims(&claims, expected_nonce)?;
                saw_overall_success = true;
            }
            NvidiaClaimClass::Other => {}
        }
    }

    if !saw_gpu_success {
        return Err(nvidia_quote_invalid(
            "NVIDIA NRAS evidence did not contain a successful GPU claim",
        ));
    }
    if evidence_contains_detached_eat(evidence) && !saw_overall_success {
        return Err(nvidia_quote_invalid(
            "NVIDIA NRAS detached EAT did not contain a successful overall claim",
        ));
    }
    Ok(())
}

fn verify_nvidia_nvtrust_offline_quote(
    evidence: &str,
    trusted_jwks: Option<&Value>,
    expected_nonce: &str,
) -> Result<()> {
    const KIND: &str = "nvidia_nvtrust_offline_jwt";
    let trusted_jwks = trusted_jwks.ok_or_else(|| GatewayError::HardwareQuoteTrustRootMissing {
        kind: KIND.to_owned(),
    })?;
    let jwks = parse_vendor_jwks(trusted_jwks, KIND)?;
    let issuers = nvidia_offline_issuers(trusted_jwks);
    let tokens = nvidia_eat_tokens_for_kind(evidence, KIND)?;
    if tokens.is_empty() {
        return Err(hardware_quote_invalid(
            KIND,
            "no JWT found in NVIDIA offline verifier evidence",
        ));
    }

    let mut saw_gpu_success = false;
    let mut saw_overall_success = false;
    for token in tokens {
        let claims = decode_vendor_identity_jwt_with_jwks(&token, &jwks, KIND, &issuers)?;
        match nvidia_offline_claim_class(&claims) {
            NvidiaClaimClass::Gpu => {
                validate_nvidia_offline_gpu_claims(&claims, expected_nonce)?;
                saw_gpu_success = true;
            }
            NvidiaClaimClass::Overall => {
                validate_nvidia_offline_overall_claims(&claims, expected_nonce)?;
                saw_overall_success = true;
            }
            NvidiaClaimClass::Other => {}
        }
    }

    if !saw_gpu_success {
        return Err(hardware_quote_invalid(
            KIND,
            "NVIDIA offline evidence did not contain a successful GPU claim",
        ));
    }
    if evidence_contains_detached_eat(evidence) && !saw_overall_success {
        return Err(hardware_quote_invalid(
            KIND,
            "NVIDIA offline detached EAT did not contain a successful overall claim",
        ));
    }
    Ok(())
}

fn nvidia_offline_issuers(trusted_jwks: &Value) -> Vec<String> {
    trusted_jwks
        .get("issuers")
        .and_then(Value::as_array)
        .map(|issuers| {
            issuers
                .iter()
                .filter_map(Value::as_str)
                .filter(|issuer| !issuer.trim().is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|issuers| !issuers.is_empty())
        .unwrap_or_else(|| {
            vec![
                NVIDIA_LOCAL_VERIFIER_ISSUER.to_owned(),
                NVIDIA_NRAS_ISSUER.to_owned(),
            ]
        })
}

fn parse_nvidia_jwks(value: &Value) -> Result<JwkSet> {
    let jwks_value = value.get("jwks").unwrap_or(value).clone();
    serde_json::from_value(jwks_value).map_err(|err| GatewayError::HardwareQuoteInvalid {
        kind: "nvidia_nras_jwt".to_owned(),
        reason: format!("trusted JWKS is invalid: {err}"),
    })
}

fn nvidia_eat_tokens(evidence: &str) -> Result<Vec<String>> {
    nvidia_eat_tokens_for_kind(evidence, "nvidia_nras_jwt")
}

fn nvidia_eat_tokens_for_kind(evidence: &str, kind: &'static str) -> Result<Vec<String>> {
    if looks_like_jwt(evidence.trim()) {
        return Ok(vec![evidence.trim().to_owned()]);
    }
    let value: Value =
        serde_json::from_str(evidence).map_err(|err| GatewayError::HardwareQuoteInvalid {
            kind: kind.to_owned(),
            reason: format!("NVIDIA evidence is neither a JWT nor JSON: {err}"),
        })?;
    let root = value.get("detached_eat").unwrap_or(&value);
    let mut tokens = Vec::new();
    collect_nvidia_eat_tokens(root, &mut tokens);
    tokens.sort();
    tokens.dedup();
    Ok(tokens)
}

fn collect_nvidia_eat_tokens(value: &Value, tokens: &mut Vec<String>) {
    match value {
        Value::String(text) if looks_like_jwt(text) => tokens.push(text.clone()),
        Value::Array(values) => {
            if values.len() == 2
                && values[0].as_str() == Some("JWT")
                && values[1].as_str().is_some_and(looks_like_jwt)
            {
                tokens.push(values[1].as_str().unwrap().to_owned());
            }
            for child in values {
                collect_nvidia_eat_tokens(child, tokens);
            }
        }
        Value::Object(object) => {
            for child in object.values() {
                collect_nvidia_eat_tokens(child, tokens);
            }
        }
        _ => {}
    }
}

fn looks_like_jwt(value: &str) -> bool {
    value.split('.').count() == 3 && !value.chars().any(char::is_whitespace)
}

fn evidence_contains_detached_eat(evidence: &str) -> bool {
    serde_json::from_str::<Value>(evidence)
        .ok()
        .and_then(|value| value.get("detached_eat").cloned())
        .is_some()
}

fn decode_nvidia_nras_jwt(token: &str, jwks: &JwkSet) -> Result<Value> {
    let header = decode_header(token).map_err(|err| GatewayError::HardwareQuoteInvalid {
        kind: "nvidia_nras_jwt".to_owned(),
        reason: format!("JWT header decode failed: {err}"),
    })?;
    if header.alg != Algorithm::ES384 {
        return Err(nvidia_quote_invalid("NVIDIA NRAS JWT must use ES384"));
    }
    let kid = header
        .kid
        .as_deref()
        .ok_or_else(|| nvidia_quote_invalid("NVIDIA NRAS JWT is missing kid"))?;
    let jwk = jwks
        .find(kid)
        .ok_or_else(|| nvidia_quote_invalid("NVIDIA NRAS JWT kid is not in trusted JWKS"))?;
    if jwk
        .common
        .key_algorithm
        .as_ref()
        .is_some_and(|alg| *alg != KeyAlgorithm::ES384)
    {
        return Err(nvidia_quote_invalid(
            "trusted NVIDIA NRAS JWK declares a non-ES384 alg",
        ));
    }
    let decoding_key =
        DecodingKey::from_jwk(jwk).map_err(|err| GatewayError::HardwareQuoteInvalid {
            kind: "nvidia_nras_jwt".to_owned(),
            reason: format!("trusted NVIDIA NRAS JWK is invalid: {err}"),
        })?;
    let mut validation = Validation::new(Algorithm::ES384);
    validation.validate_aud = false;
    validation.set_issuer(&[NVIDIA_NRAS_ISSUER]);
    validation.set_required_spec_claims(&["exp", "iss"]);
    decode::<Value>(token, &decoding_key, &validation)
        .map(|token| token.claims)
        .map_err(|err| GatewayError::HardwareQuoteInvalid {
            kind: "nvidia_nras_jwt".to_owned(),
            reason: format!("NVIDIA NRAS JWT verification failed: {err}"),
        })
}

fn decode_vendor_identity_jwt_with_jwks(
    token: &str,
    jwks: &JwkSet,
    kind: &'static str,
    issuers: &[String],
) -> Result<Value> {
    let header = decode_header(token)
        .map_err(|err| hardware_quote_invalid(kind, format!("JWT header decode failed: {err}")))?;
    if header.alg != Algorithm::ES384 {
        return Err(hardware_quote_invalid(
            kind,
            "vendor identity JWT must use ES384",
        ));
    }
    let kid = header
        .kid
        .as_deref()
        .ok_or_else(|| hardware_quote_invalid(kind, "vendor identity JWT is missing kid"))?;
    let jwk = jwks.find(kid).ok_or_else(|| {
        hardware_quote_invalid(kind, "vendor identity JWT kid is not in trusted JWKS")
    })?;
    if jwk
        .common
        .key_algorithm
        .as_ref()
        .is_some_and(|alg| *alg != KeyAlgorithm::ES384)
    {
        return Err(hardware_quote_invalid(
            kind,
            "trusted vendor JWK declares a non-ES384 alg",
        ));
    }
    let decoding_key = DecodingKey::from_jwk(jwk).map_err(|err| {
        hardware_quote_invalid(kind, format!("trusted vendor JWK is invalid: {err}"))
    })?;
    let mut validation = Validation::new(Algorithm::ES384);
    validation.validate_aud = false;
    validation.set_issuer(issuers);
    validation.set_required_spec_claims(&["exp", "iss"]);
    decode::<Value>(token, &decoding_key, &validation)
        .map(|token| token.claims)
        .map_err(|err| {
            hardware_quote_invalid(
                kind,
                format!("vendor identity JWT verification failed: {err}"),
            )
        })
}

enum NvidiaClaimClass {
    Gpu,
    Overall,
    Other,
}

fn nvidia_claim_class(claims: &Value) -> NvidiaClaimClass {
    if claims
        .get("x-nvidia-device-type")
        .and_then(Value::as_str)
        .is_some_and(|value| value == "gpu")
    {
        NvidiaClaimClass::Gpu
    } else if claims.get("x-nvidia-overall-att-result").is_some()
        || claims
            .get("sub")
            .and_then(Value::as_str)
            .is_some_and(|value| value.contains("NVIDIA") && value.contains("ATTESTATION"))
    {
        NvidiaClaimClass::Overall
    } else {
        NvidiaClaimClass::Other
    }
}

fn nvidia_offline_claim_class(claims: &Value) -> NvidiaClaimClass {
    if claims.get("x-nv-gpu-attestation-report-verified").is_some()
        || claims.get("x-nv-gpu-cert-chain-verified").is_some()
        || claims.get("x-nv-gpu-measurements-match").is_some()
    {
        NvidiaClaimClass::Gpu
    } else {
        nvidia_claim_class(claims)
    }
}

fn validate_nvidia_overall_claims(claims: &Value, expected_nonce: &str) -> Result<()> {
    require_nvidia_nonce(claims, expected_nonce)?;
    require_bool_claim(claims, "x-nvidia-overall-att-result")?;
    Ok(())
}

fn validate_nvidia_offline_overall_claims(claims: &Value, expected_nonce: &str) -> Result<()> {
    const KIND: &str = "nvidia_nvtrust_offline_jwt";
    require_nvidia_nonce_for_kind(claims, expected_nonce, KIND)?;
    require_bool_claim_for_kind(claims, "x-nvidia-overall-att-result", KIND)?;
    Ok(())
}

fn validate_nvidia_gpu_claims(claims: &Value, expected_nonce: &str) -> Result<()> {
    require_nvidia_nonce(claims, expected_nonce)?;
    require_string_claim_ci(claims, "measres", "success")?;
    for field in [
        "x-nvidia-gpu-arch-check",
        "x-nvidia-gpu-attestation-report-cert-chain-fwid-match",
        "x-nvidia-gpu-attestation-report-parsed",
        "x-nvidia-gpu-attestation-report-nonce-match",
        "x-nvidia-gpu-attestation-report-signature-verified",
        "x-nvidia-gpu-driver-rim-fetched",
        "x-nvidia-gpu-driver-rim-schema-validated",
        "x-nvidia-gpu-driver-rim-signature-verified",
        "x-nvidia-gpu-driver-rim-version-match",
        "x-nvidia-gpu-driver-rim-measurements-available",
        "x-nvidia-gpu-vbios-rim-fetched",
        "x-nvidia-gpu-vbios-rim-schema-validated",
        "x-nvidia-gpu-vbios-rim-version-match",
        "x-nvidia-gpu-vbios-rim-signature-verified",
        "x-nvidia-gpu-vbios-rim-measurements-available",
        "x-nvidia-gpu-vbios-index-no-conflict",
    ] {
        require_bool_claim(claims, field)?;
    }
    if claims
        .get("secboot")
        .and_then(Value::as_bool)
        .is_some_and(|secboot| !secboot)
    {
        return Err(nvidia_quote_invalid("NVIDIA GPU claim secboot is false"));
    }
    if claims
        .get("dbgstat")
        .and_then(Value::as_str)
        .is_some_and(|dbgstat| dbgstat != "disabled")
    {
        return Err(nvidia_quote_invalid(
            "NVIDIA GPU claim dbgstat must be disabled when present",
        ));
    }
    Ok(())
}

fn validate_nvidia_offline_gpu_claims(claims: &Value, expected_nonce: &str) -> Result<()> {
    const KIND: &str = "nvidia_nvtrust_offline_jwt";
    require_nvidia_nonce_for_kind(claims, expected_nonce, KIND)?;
    for field in [
        "x-nv-gpu-cert-chain-verified",
        "x-nv-gpu-cert-check-complete",
        "x-nv-gpu-measurement-available",
        "x-nv-gpu-root-cert-available",
        "x-nv-gpu-info-fetched",
        "x-nv-gpu-available",
        "x-nv-gpu-attestation-report-available",
        "x-nv-gpu-attestation-report-driver-version-match",
        "x-nv-gpu-attestation-report-vbios-version-match",
        "x-nv-gpu-attestation-report-verified",
        "x-nv-gpu-driver-rim-schema-fetched",
        "x-nv-gpu-driver-rim-cert-extracted",
        "x-nv-gpu-vbios-rim-cert-extracted",
        "x-nv-gpu-vbios-rim-driver-measurements-available",
        "x-nv-gpu-driver-rim-driver-measurements-available",
        "x-nvidia-gpu-arch-check",
        "x-nvidia-gpu-driver-rim-signature-verified",
        "x-nvidia-gpu-vbios-rim-signature-verified",
        "x-nvidia-gpu-attestation-report-parsed",
        "x-nv-gpu-nonce-match",
    ] {
        require_bool_claim_for_kind(claims, field, KIND)?;
    }
    if !claim_is_success(claims, "x-nv-gpu-measurements-match")
        && !claim_is_success(claims, "measres")
    {
        return Err(hardware_quote_invalid(
            KIND,
            "NVIDIA offline GPU measurements did not match RIM",
        ));
    }
    if claims
        .get("x-nvidia-attestation-warning")
        .and_then(Value::as_bool)
        .is_some_and(|warning| warning)
    {
        return Err(hardware_quote_invalid(
            KIND,
            "NVIDIA offline verifier reported an attestation warning",
        ));
    }
    Ok(())
}

fn claim_is_success(claims: &Value, field: &'static str) -> bool {
    claims
        .get(field)
        .and_then(Value::as_bool)
        .is_some_and(|value| value)
        || claims
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|value| {
                value.eq_ignore_ascii_case("success") || value.eq_ignore_ascii_case("true")
            })
}

fn require_nvidia_nonce(claims: &Value, expected_nonce: &str) -> Result<()> {
    require_nvidia_nonce_for_kind(claims, expected_nonce, "nvidia_nras_jwt")
}

fn require_nvidia_nonce_for_kind(
    claims: &Value,
    expected_nonce: &str,
    kind: &'static str,
) -> Result<()> {
    let nonce = claims
        .get("eat_nonce")
        .and_then(Value::as_str)
        .or_else(|| claims.get("x-nv-gpu-nonce").and_then(Value::as_str))
        .ok_or_else(|| hardware_quote_invalid(kind, "NVIDIA claim is missing nonce"))?;
    if !nonce.eq_ignore_ascii_case(expected_nonce) {
        return Err(hardware_quote_invalid(
            kind,
            "NVIDIA nonce does not match quote binding",
        ));
    }
    Ok(())
}

fn require_bool_claim(claims: &Value, field: &'static str) -> Result<()> {
    if claims.get(field).and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(nvidia_quote_invalid(format!(
            "NVIDIA GPU claim {field} is not true"
        )))
    }
}

fn require_string_claim_ci(
    claims: &Value,
    field: &'static str,
    expected: &'static str,
) -> Result<()> {
    if claims
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
    {
        Ok(())
    } else {
        Err(nvidia_quote_invalid(format!(
            "NVIDIA GPU claim {field} is not {expected}"
        )))
    }
}

fn nvidia_quote_invalid(reason: impl Into<String>) -> GatewayError {
    hardware_quote_invalid("nvidia_nras_jwt", reason)
}

fn hardware_quote_invalid(kind: &'static str, reason: impl Into<String>) -> GatewayError {
    GatewayError::HardwareQuoteInvalid {
        kind: kind.to_owned(),
        reason: reason.into(),
    }
}

fn expected_tp_degree(caps: &Value) -> u32 {
    caps.get("tp_degree")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or(1)
}

fn compare_field(field: &'static str, expected: &str, actual: &str) -> Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(GatewayError::ContractMismatch {
            field,
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        })
    }
}

fn validate_heartbeat_fields(heartbeat: &ProviderHeartbeat) -> Result<()> {
    validate_hex_field("provider", &heartbeat.provider, 32)?;
    validate_hex_field("enclave_id", &heartbeat.enclave_id, 32)?;
    validate_hex_field("room_id", &heartbeat.room_id, 16)?;
    if let Some(transport_peer) = heartbeat.transport_peer.as_deref() {
        validate_hex_field("transport_peer", transport_peer, 32)?;
    }
    if let Some(identity_anchor) = heartbeat.identity_anchor.as_deref() {
        validate_identity_anchor(identity_anchor)?;
    }
    validate_hex_field("att.head", &heartbeat.att.head, 32)?;
    validate_hex_field("nonce", &heartbeat.nonce, 32)?;
    validate_hex_field("sig", &heartbeat.sig, 64)?;
    if heartbeat.model_id.trim().is_empty() {
        return Err(GatewayError::BadHeartbeatField {
            field: "model_id",
            reason: "must not be empty".to_owned(),
        });
    }
    if !(0.0..=1.0).contains(&heartbeat.sat) || !heartbeat.sat.is_finite() {
        return Err(GatewayError::BadHeartbeatField {
            field: "sat",
            reason: "must be finite and in [0, 1]".to_owned(),
        });
    }
    if heartbeat.slots.max == 0 {
        return Err(GatewayError::BadHeartbeatField {
            field: "slots.max",
            reason: "must be greater than zero".to_owned(),
        });
    }
    if heartbeat.slots.active > heartbeat.slots.max {
        return Err(GatewayError::BadHeartbeatField {
            field: "slots.active",
            reason: "must not exceed slots.max".to_owned(),
        });
    }
    if heartbeat.slots.active_requests > heartbeat.slots.active {
        return Err(GatewayError::BadHeartbeatField {
            field: "slots.active_requests",
            reason: "must not exceed slots.active".to_owned(),
        });
    }
    if heartbeat.q.free_slots > heartbeat.slots.max {
        return Err(GatewayError::BadHeartbeatField {
            field: "q.free_slots",
            reason: "must not exceed slots.max".to_owned(),
        });
    }
    if heartbeat.caps.ctx == 0 {
        return Err(GatewayError::BadHeartbeatField {
            field: "caps.ctx",
            reason: "must be greater than zero".to_owned(),
        });
    }
    if heartbeat
        .perf
        .tok_s
        .is_some_and(|tok_s| !tok_s.is_finite() || tok_s < 0.0)
    {
        return Err(GatewayError::BadHeartbeatField {
            field: "perf.tok_s",
            reason: "must be finite and non-negative".to_owned(),
        });
    }
    Ok(())
}

fn validate_heartbeat_time(
    ts: u64,
    now_millis: u64,
    max_age_millis: u64,
    max_clock_skew_millis: u64,
) -> Result<()> {
    if ts > now_millis {
        let skew_millis = ts - now_millis;
        if skew_millis > max_clock_skew_millis {
            return Err(GatewayError::HeartbeatFromFuture {
                skew_millis,
                max_skew_millis: max_clock_skew_millis,
            });
        }
        return Ok(());
    }

    let age_millis = now_millis - ts;
    if age_millis > max_age_millis {
        Err(GatewayError::HeartbeatStale {
            age_millis,
            max_age_millis,
        })
    } else {
        Ok(())
    }
}

fn verify_heartbeat_signature(raw: &Value, provider: &str, signature_hex: &str) -> Result<()> {
    let public_key_bytes =
        hex_to_array::<32>(provider).map_err(|err| GatewayError::BadHeartbeatSignature {
            reason: format!("provider public key is invalid: {err}"),
        })?;
    let public_key = VerifyingKey::from_bytes(&public_key_bytes).map_err(|err| {
        GatewayError::BadHeartbeatSignature {
            reason: format!("provider public key is invalid: {err}"),
        }
    })?;
    let signature_bytes =
        hex::decode(signature_hex).map_err(|err| GatewayError::BadHeartbeatSignature {
            reason: err.to_string(),
        })?;
    let signature = Signature::from_slice(&signature_bytes).map_err(|err| {
        GatewayError::BadHeartbeatSignature {
            reason: err.to_string(),
        }
    })?;
    let payload = heartbeat_signing_payload(raw)?;
    public_key
        .verify(&payload, &signature)
        .map_err(|err| GatewayError::BadHeartbeatSignature {
            reason: err.to_string(),
        })
}

pub fn heartbeat_signing_payload(raw: &Value) -> Result<Vec<u8>> {
    let mut unsigned = raw.clone();
    let object = unsigned
        .as_object_mut()
        .ok_or_else(|| GatewayError::HeartbeatJson("heartbeat must be a JSON object".to_owned()))?;
    object.remove("sig");
    canonical_json_bytes(&unsigned)
}

fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>> {
    let mut out = String::new();
    write_canonical_json(value, &mut out)?;
    Ok(out.into_bytes())
}

fn write_canonical_json(value: &Value, out: &mut String) -> Result<()> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Value::Number(number) => out.push_str(&canonical_json_number(number)?),
        Value::String(value) => {
            let encoded = serde_json::to_string(value)
                .map_err(|err| GatewayError::HeartbeatJson(err.to_string()))?;
            out.push_str(&encoded);
        }
        Value::Array(values) => {
            out.push('[');
            for (idx, value) in values.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                write_canonical_json(value, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            for (idx, key) in keys.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                let encoded_key = serde_json::to_string(key)
                    .map_err(|err| GatewayError::HeartbeatJson(err.to_string()))?;
                out.push_str(&encoded_key);
                out.push(':');
                let child = map
                    .get(*key)
                    .ok_or_else(|| GatewayError::HeartbeatJson("missing object key".to_owned()))?;
                write_canonical_json(child, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn canonical_json_number(number: &serde_json::Number) -> Result<String> {
    if let Some(value) = number.as_i64() {
        return Ok(value.to_string());
    }
    if let Some(value) = number.as_u64() {
        return Ok(value.to_string());
    }
    let value = number
        .as_f64()
        .ok_or_else(|| GatewayError::HeartbeatJson("invalid JSON number".to_owned()))?;
    if !value.is_finite() {
        return Err(GatewayError::HeartbeatJson(
            "non-finite JSON number".to_owned(),
        ));
    }
    if value.fract() == 0.0 {
        return Ok(format!("{value:.0}"));
    }
    Ok(number.to_string())
}

fn validate_hex_field(field: &'static str, value: &str, bytes: usize) -> Result<()> {
    if value.len() == bytes * 2 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(GatewayError::BadHeartbeatField {
            field,
            reason: format!("must be {bytes} bytes of hex"),
        })
    }
}

fn validate_identity_anchor(value: &str) -> Result<()> {
    let Some((kind, digest)) = value.split_once(':') else {
        return Err(GatewayError::BadHeartbeatField {
            field: "identity_anchor",
            reason: "must be kind:32-byte-hex".to_owned(),
        });
    };
    if !matches!(kind, "device" | "fingerprint" | "kyb" | "provider") {
        return Err(GatewayError::BadHeartbeatField {
            field: "identity_anchor",
            reason: "kind must be device, fingerprint, kyb, or provider".to_owned(),
        });
    }
    validate_hex_field("identity_anchor", digest, 32)
}

fn validate_hex_nonce(value: &str) -> Result<()> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(GatewayError::BadNonce)
    }
}

fn validate_report_time(
    report_ts: u64,
    now_ts: u64,
    max_age_secs: u64,
    max_skew_secs: u64,
) -> Result<()> {
    if report_ts > now_ts {
        let skew_secs = report_ts - now_ts;
        if skew_secs > max_skew_secs {
            return Err(GatewayError::ReportFromFuture {
                skew_secs,
                max_skew_secs,
            });
        }
        return Ok(());
    }

    let age_secs = now_ts - report_ts;
    if age_secs > max_age_secs {
        Err(GatewayError::ReportStale {
            age_secs,
            max_age_secs,
        })
    } else {
        Ok(())
    }
}

fn verify_report_signature(report: &AttestationReport, signer: AttestationSigner) -> Result<()> {
    let (signer_name, public_key_hex, signature_hex) = match signer {
        AttestationSigner::Enclave => (
            "enclave",
            report.enclave_pubkey.as_str(),
            report.sig_enclave.as_str(),
        ),
        AttestationSigner::Provider => (
            "provider",
            report.provider_pubkey.as_str(),
            report.sig_provider.as_str(),
        ),
    };
    let public_key_bytes =
        hex_to_array::<32>(public_key_hex).map_err(|err| GatewayError::BadPublicKey {
            signer: signer_name,
            reason: err,
        })?;
    let public_key =
        VerifyingKey::from_bytes(&public_key_bytes).map_err(|err| GatewayError::BadPublicKey {
            signer: signer_name,
            reason: err.to_string(),
        })?;
    let signature_bytes = hex::decode(signature_hex).map_err(|err| GatewayError::BadSignature {
        signer: signer_name,
        reason: err.to_string(),
    })?;
    let signature =
        Signature::from_slice(&signature_bytes).map_err(|err| GatewayError::BadSignature {
            signer: signer_name,
            reason: err.to_string(),
        })?;
    let body = report.body();
    let payload = attestation_signing_bytes(&body, signer)
        .map_err(|err| GatewayError::SigningPayload(err.to_string()))?;
    public_key
        .verify(&payload, &signature)
        .map_err(|err| GatewayError::BadSignature {
            signer: signer_name,
            reason: err.to_string(),
        })
}

fn hex_to_array<const N: usize>(value: &str) -> std::result::Result<[u8; N], String> {
    let bytes = hex::decode(value).map_err(|err| err.to_string())?;
    bytes
        .try_into()
        .map_err(|_| format!("expected {N} bytes of hex"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use mayhem_proto::{
        AttestationBody, AttestationReport, AttestationRuntimeConfig, ATTESTATION_ALG,
    };
    use serde_json::json;

    fn sample_report() -> AttestationReport {
        AttestationReport {
            schema_version: ATTESTATION_SCHEMA_VERSION,
            alg: ATTESTATION_ALG.to_owned(),
            enclave_id: "enclave".to_owned(),
            enclave_pubkey: "11".repeat(32),
            provider_pubkey: "22".repeat(32),
            manifest_hash: "manifest".to_owned(),
            binary_hash: "binary".to_owned(),
            att_tier: 1,
            hw_quote: None,
            boot_epoch: 100,
            report_ts: 200,
            nonce_u: "aa".repeat(32),
            runtime_config: AttestationRuntimeConfig::default(),
            sig_enclave: "33".repeat(64),
            sig_provider: "44".repeat(64),
        }
    }

    #[test]
    fn exposes_crate_name() {
        assert_eq!(CRATE_NAME, "mayhem-gateway");
    }

    #[test]
    fn report_body_excludes_signatures() {
        let report = sample_report();
        let body = report.body();

        assert_eq!(
            body,
            AttestationBody {
                schema_version: report.schema_version,
                alg: report.alg,
                enclave_id: report.enclave_id,
                enclave_pubkey: report.enclave_pubkey,
                provider_pubkey: report.provider_pubkey,
                manifest_hash: report.manifest_hash,
                binary_hash: report.binary_hash,
                att_tier: report.att_tier,
                hw_quote: report.hw_quote,
                boot_epoch: report.boot_epoch,
                report_ts: report.report_ts,
                nonce_u: report.nonce_u,
                runtime_config: report.runtime_config,
            }
        );
    }

    #[test]
    fn validates_signed_heartbeat_and_rejects_bad_signature() {
        let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
        let provider = hex::encode(signing_key.verifying_key().to_bytes());
        let now = 1_800_000_000_000;
        let heartbeat = signed_heartbeat(&signing_key, &provider, now, "aa");
        let mut cache = HeartbeatReplayCache::default();

        let accepted = validate_provider_heartbeat(&mut HeartbeatValidationRequest {
            raw: &heartbeat,
            now_millis: now,
            replay_cache: &mut cache,
            max_age_millis: DEFAULT_HEARTBEAT_MAX_AGE_MILLIS,
            max_clock_skew_millis: DEFAULT_HEARTBEAT_MAX_CLOCK_SKEW_MILLIS,
        })
        .expect("valid heartbeat");
        assert_eq!(accepted.provider, provider);
        assert_eq!(accepted.att.head, "44".repeat(32));

        let mut bad = heartbeat;
        bad["sat"] = json!(0.25);
        let err = validate_provider_heartbeat(&mut HeartbeatValidationRequest {
            raw: &bad,
            now_millis: now,
            replay_cache: &mut HeartbeatReplayCache::default(),
            max_age_millis: DEFAULT_HEARTBEAT_MAX_AGE_MILLIS,
            max_clock_skew_millis: DEFAULT_HEARTBEAT_MAX_CLOCK_SKEW_MILLIS,
        })
        .expect_err("tampered heartbeat must fail");
        assert!(matches!(err, GatewayError::BadHeartbeatSignature { .. }));
    }

    #[test]
    fn heartbeat_signature_survives_js_integer_float_normalization() {
        let signing_key = SigningKey::from_bytes(&[9_u8; 32]);
        let provider = hex::encode(signing_key.verifying_key().to_bytes());
        let now = 1_800_000_000_000;
        let mut heartbeat = signed_heartbeat(&signing_key, &provider, now, "ab");

        heartbeat["perf"]["tok_s"] = json!(42);

        validate_provider_heartbeat(&mut HeartbeatValidationRequest {
            raw: &heartbeat,
            now_millis: now,
            replay_cache: &mut HeartbeatReplayCache::default(),
            max_age_millis: DEFAULT_HEARTBEAT_MAX_AGE_MILLIS,
            max_clock_skew_millis: DEFAULT_HEARTBEAT_MAX_CLOCK_SKEW_MILLIS,
        })
        .expect("JS-normalized heartbeat");
    }

    #[test]
    fn heartbeat_without_contract_version_is_rejected_before_signature_check() {
        let signing_key = SigningKey::from_bytes(&[10_u8; 32]);
        let provider = hex::encode(signing_key.verifying_key().to_bytes());
        let now = 1_800_000_000_000;
        let mut heartbeat = signed_heartbeat(&signing_key, &provider, now, "ad");
        heartbeat
            .as_object_mut()
            .expect("heartbeat object")
            .remove("contract_version");
        heartbeat["sig"] = json!("00".repeat(64));

        let err = validate_provider_heartbeat(&mut HeartbeatValidationRequest {
            raw: &heartbeat,
            now_millis: now,
            replay_cache: &mut HeartbeatReplayCache::default(),
            max_age_millis: DEFAULT_HEARTBEAT_MAX_AGE_MILLIS,
            max_clock_skew_millis: DEFAULT_HEARTBEAT_MAX_CLOCK_SKEW_MILLIS,
        })
        .expect_err("heartbeat without current contract version must fail");
        assert!(matches!(err, GatewayError::HeartbeatJson(_)));
        assert!(err.to_string().contains("contract_version"));
    }

    #[test]
    fn heartbeat_without_accepting_new_is_rejected_before_signature_check() {
        let signing_key = SigningKey::from_bytes(&[11_u8; 32]);
        let provider = hex::encode(signing_key.verifying_key().to_bytes());
        let now = 1_800_000_000_000;
        let mut heartbeat = signed_heartbeat(&signing_key, &provider, now, "ae");
        heartbeat
            .as_object_mut()
            .expect("heartbeat object")
            .remove("accepting_new");
        heartbeat["sig"] = json!("00".repeat(64));

        let err = validate_provider_heartbeat(&mut HeartbeatValidationRequest {
            raw: &heartbeat,
            now_millis: now,
            replay_cache: &mut HeartbeatReplayCache::default(),
            max_age_millis: DEFAULT_HEARTBEAT_MAX_AGE_MILLIS,
            max_clock_skew_millis: DEFAULT_HEARTBEAT_MAX_CLOCK_SKEW_MILLIS,
        })
        .expect_err("heartbeat without accepting_new must fail");
        assert!(matches!(err, GatewayError::HeartbeatJson(_)));
        assert!(err.to_string().contains("accepting_new"));
    }

    #[test]
    fn heartbeat_receiver_logs_bad_signature_and_replay_drops() {
        let signing_key = SigningKey::from_bytes(&[8_u8; 32]);
        let provider = hex::encode(signing_key.verifying_key().to_bytes());
        let now = 1_800_000_000_000;
        let heartbeat = signed_heartbeat(&signing_key, &provider, now, "bb");
        let mut receiver = HeartbeatReceiver::new();

        assert!(receiver.receive(&heartbeat, now).is_some());
        assert!(receiver.receive(&heartbeat, now).is_none());
        assert_eq!(receiver.drops().len(), 1);
        assert!(receiver.drops()[0].reason.contains("already seen"));

        let mut bad = signed_heartbeat(&signing_key, &provider, now, "cc");
        bad["sig"] = json!("00".repeat(64));
        assert!(receiver.receive(&bad, now).is_none());
        assert_eq!(receiver.drops().len(), 2);
        assert!(receiver.drops()[1].reason.contains("signature"));
    }

    fn signed_heartbeat(
        signing_key: &SigningKey,
        provider: &str,
        now_millis: u64,
        nonce_prefix: &str,
    ) -> Value {
        let mut heartbeat = json!({
            "t": "hb",
            "v": HEARTBEAT_SCHEMA_VERSION,
            "contract_version": CONTRACT_VERSION,
            "provider": provider,
            "enclave_id": "11".repeat(32),
            "model_id": "model/test@4bit",
            "room_id": "22".repeat(16),
            "sat": 0.1,
            "slots": { "active": 1, "active_requests": 0, "max": 4 },
            "q": { "free_slots": 1, "engine_backlog": 0, "est_wait_ms": 0 },
            "perf": { "tok_s": 42.0, "ttft_ms": 120 },
            "price_ver": 3,
            "min_ask_au": "0",
            "accepting_new": true,
            "caps": { "tools": true, "json": true, "ctx": 8192, "vision": false },
            "att": { "epoch": 81, "head": "44".repeat(32) },
            "ts": now_millis,
            "nonce": format!("{nonce_prefix}{}", "00".repeat(31)),
        });
        let payload = heartbeat_signing_payload(&heartbeat).expect("signing payload");
        let signature = signing_key.sign(&payload);
        heartbeat["sig"] = json!(hex::encode(signature.to_bytes()));
        heartbeat
    }
}
