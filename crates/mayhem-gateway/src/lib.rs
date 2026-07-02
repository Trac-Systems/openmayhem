#![forbid(unsafe_code)]

use std::collections::BTreeSet;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use mayhem_proto::{
    attestation_report_head, attestation_signing_bytes, catalog_enclave_id, AttestationReport,
    AttestationSigner, CatalogEnclaveIdentity, ATTESTATION_ALG, ATTESTATION_SCHEMA_VERSION,
};
use thiserror::Error;

pub const CRATE_NAME: &str = "mayhem-gateway";
pub const DEFAULT_MAX_REPORT_AGE_SECS: u64 = 24 * 60 * 60;
pub const DEFAULT_MAX_REPORT_CLOCK_SKEW_SECS: u64 = 5 * 60;

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
    use mayhem_proto::{AttestationBody, AttestationReport, ATTESTATION_ALG};

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
}
