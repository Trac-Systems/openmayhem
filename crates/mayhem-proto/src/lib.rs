#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

pub const CRATE_NAME: &str = "mayhem-proto";
pub const ATTESTATION_SCHEMA_VERSION: u32 = 1;
pub const ATTESTATION_ALG: &str = "ed25519";

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
    pub hw_quote: Option<String>,
    pub boot_epoch: u64,
    pub report_ts: u64,
    pub nonce_u: String,
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
    pub hw_quote: Option<String>,
    pub boot_epoch: u64,
    pub report_ts: u64,
    pub nonce_u: String,
    pub sig_enclave: String,
    pub sig_provider: String,
}

#[derive(Serialize)]
struct AttestationSigningEnvelope<'a> {
    domain: &'static str,
    signer: &'static str,
    body: &'a AttestationBody,
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
        }
    }
}

pub fn catalog_enclave_id(identity: &CatalogEnclaveIdentity) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(identity.admin_pubkey.as_bytes());
    hasher.update(identity.model_id.as_bytes());
    hasher.update(identity.artifact_root.as_bytes());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_crate_name() {
        assert_eq!(CRATE_NAME, "mayhem-proto");
    }

    #[test]
    fn catalog_enclave_id_changes_when_bound_fields_change() {
        let base = CatalogEnclaveIdentity {
            admin_pubkey: "admin".to_owned(),
            model_id: "model".to_owned(),
            artifact_root: "artifact".to_owned(),
            manifest_hash: "manifest".to_owned(),
            binary_hash: "binary".to_owned(),
        };
        let mut changed = base.clone();
        changed.manifest_hash = "other-manifest".to_owned();

        assert_ne!(catalog_enclave_id(&base), catalog_enclave_id(&changed));
    }
}
