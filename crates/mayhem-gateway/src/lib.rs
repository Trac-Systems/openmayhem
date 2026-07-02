#![forbid(unsafe_code)]

pub mod openai;
pub mod provider_table;
pub use provider_table::*;

use std::collections::{BTreeSet, HashSet, VecDeque};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use mayhem_proto::{
    attestation_report_head, attestation_signing_bytes, catalog_enclave_id, AttestationReport,
    AttestationSigner, CatalogEnclaveIdentity, ATTESTATION_ALG, ATTESTATION_SCHEMA_VERSION,
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
    #[error("heartbeat JSON error: {0}")]
    HeartbeatJson(String),
    #[error("heartbeat must have t=\"hb\" and v={expected_version}")]
    BadHeartbeatSchema { expected_version: u32 },
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
    pub artifact_root: String,
    pub manifest_hash: String,
    pub binary_hash: String,
    pub att_tier: u8,
}

#[derive(Debug)]
pub struct AttestationVerificationRequest<'a> {
    pub report: &'a AttestationReport,
    pub contract: &'a EnclaveContractRecord,
    pub trusted_binary_hashes: &'a BTreeSet<String>,
    pub expected_nonce: &'a str,
    pub expected_provider_pubkey: Option<&'a str>,
    pub now_ts: u64,
    pub max_report_age_secs: u64,
    pub max_report_clock_skew_secs: u64,
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
        now_ts: u64,
    ) -> Self {
        Self {
            report,
            contract,
            trusted_binary_hashes,
            expected_nonce,
            expected_provider_pubkey: None,
            now_ts,
            max_report_age_secs: DEFAULT_MAX_REPORT_AGE_SECS,
            max_report_clock_skew_secs: DEFAULT_MAX_REPORT_CLOCK_SKEW_SECS,
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
    if let Some(expected_provider) = request.expected_provider_pubkey {
        compare_field(
            "provider_pubkey",
            expected_provider,
            &report.provider_pubkey,
        )?;
    }

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
    use mayhem_proto::{AttestationBody, AttestationReport, ATTESTATION_ALG};
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
