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

use std::collections::{BTreeSet, HashSet, VecDeque};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use jsonwebtoken::{
    decode, decode_header,
    jwk::{JwkSet, KeyAlgorithm},
    Algorithm, DecodingKey, Validation,
};
use mayhem_proto::{
    attestation_report_head, attestation_signing_bytes, catalog_enclave_id, hardware_quote_binding,
    AttestationReport, AttestationSigner, CatalogEnclaveIdentity, HardwareQuoteKind,
    ATTESTATION_ALG, ATTESTATION_SCHEMA_VERSION, CONTRACT_VERSION, DEFAULT_MODEL_CLASS,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const CRATE_NAME: &str = "mayhem-gateway";
pub const DEFAULT_MAX_REPORT_AGE_SECS: u64 = 24 * 60 * 60;
pub const DEFAULT_MAX_REPORT_CLOCK_SKEW_SECS: u64 = 5 * 60;
pub const HEARTBEAT_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_HEARTBEAT_MAX_AGE_MILLIS: u64 = 30_000;
pub const DEFAULT_HEARTBEAT_MAX_CLOCK_SKEW_MILLIS: u64 = 5_000;
pub const DEFAULT_HEARTBEAT_REPLAY_CACHE_CAPACITY: usize = 5_000;
const APPLE_APP_ATTEST_ISSUER: &str = "https://appattest.apple.com";
const NVIDIA_LOCAL_VERIFIER_ISSUER: &str = "https://local.verifier.attestation.nvidia.com";
const NVIDIA_NRAS_ISSUER: &str = "https://nras.attestation.nvidia.com";

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
    #[error("mock hardware quotes are disabled")]
    MockHardwareQuoteDisabled,
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
    pub manifest_hash: String,
    pub binary_hash: String,
    pub att_tier: u8,
    pub caps: Value,
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
    pub allow_mock_hardware_quote: bool,
    pub trusted_apple_app_attest_jwks: Option<&'a Value>,
    pub trusted_nvidia_gb10_device_jwks: Option<&'a Value>,
    pub trusted_nvidia_nras_jwks: Option<&'a Value>,
    pub trusted_nvidia_offline_jwks: Option<&'a Value>,
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
    #[serde(default)]
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
    pub caps: HeartbeatCaps,
    pub att: HeartbeatAttestation,
    pub ts: u64,
    pub nonce: String,
    pub sig: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeartbeatSlots {
    pub active: u32,
    pub max: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeartbeatQueue {
    pub depth: u32,
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
            allow_mock_hardware_quote: false,
            trusted_apple_app_attest_jwks: None,
            trusted_nvidia_gb10_device_jwks: None,
            trusted_nvidia_nras_jwks: None,
            trusted_nvidia_offline_jwks: None,
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
    match quote.kind {
        HardwareQuoteKind::AppleAppAttestJwt => verify_apple_app_attest_quote(
            &quote.evidence,
            request.trusted_apple_app_attest_jwks,
            &expected,
            request.report,
        ),
        HardwareQuoteKind::MockDeviceIdentity if request.allow_mock_hardware_quote => Ok(()),
        HardwareQuoteKind::MockDeviceIdentity => Err(GatewayError::MockHardwareQuoteDisabled),
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
    fn legacy_heartbeat_without_contract_version_requires_upgrade_before_signature_check() {
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
        .expect_err("legacy heartbeat must require upgrade");
        assert!(matches!(err, GatewayError::ContractUpgradeRequired { .. }));
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
            "slots": { "active": 1, "max": 4 },
            "q": { "depth": 0, "est_wait_ms": 0 },
            "perf": { "tok_s": 42.0, "ttft_ms": 120 },
            "price_ver": 3,
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
