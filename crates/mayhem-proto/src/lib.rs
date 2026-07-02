#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

pub const CRATE_NAME: &str = "mayhem-proto";
pub const ATTESTATION_SCHEMA_VERSION: u32 = 1;
pub const ATTESTATION_ALG: &str = "ed25519";
pub const SESSION_RECEIPT_SCHEMA_VERSION: u32 = 1;
pub const HARDWARE_QUOTE_BINDING_DOMAIN: &str = "mayhem-hardware-quote-binding-v1";

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
    pub hw_quote: Option<HardwareQuote>,
    pub boot_epoch: u64,
    pub report_ts: u64,
    pub nonce_u: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareQuoteKind {
    AmdSevSnpVcek,
    IntelTdxDcap,
    NvidiaNrasJwt,
    MockTier2,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SpendVoucherBody {
    pub session_id: String,
    pub enclave_id: String,
    pub price_ver: u64,
    pub max_spend_mu: u64,
    pub checkpoint_every: CheckpointPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SpendVoucher {
    #[serde(flatten)]
    pub body: SpendVoucherBody,
    pub user_sig: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReceiptUsage {
    #[serde(rename = "in")]
    pub in_tokens: u64,
    #[serde(rename = "out")]
    pub out_tokens: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReceiptBody {
    pub schema_version: u32,
    pub session_id: String,
    pub seq: u64,
    #[serde(rename = "final")]
    pub final_receipt: bool,
    pub user: String,
    pub provider: String,
    pub enclave_id: String,
    pub model_id: String,
    pub price_ver: u64,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReceiptAck {
    pub session_id: String,
    pub seq: u64,
    pub user_sig: String,
}

#[derive(Serialize)]
struct SpendVoucherSigningEnvelope<'a> {
    domain: &'static str,
    body: &'a SpendVoucherBody,
}

#[derive(Serialize)]
struct ReceiptSigningEnvelope<'a> {
    domain: &'static str,
    body: &'a ReceiptBody,
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
    serde_json::to_vec(&SpendVoucherSigningEnvelope {
        domain: "mayhem-spend-voucher-v1",
        body,
    })
}

pub fn receipt_signing_bytes(body: &ReceiptBody) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&ReceiptSigningEnvelope {
        domain: "mayhem-session-receipt-v1",
        body,
    })
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

    #[test]
    fn hardware_quote_binding_excludes_quote_but_includes_nonce_and_identity() {
        let mut body = AttestationBody {
            schema_version: ATTESTATION_SCHEMA_VERSION,
            alg: ATTESTATION_ALG.to_owned(),
            enclave_id: "enclave".to_owned(),
            enclave_pubkey: "enclave-pub".to_owned(),
            provider_pubkey: "provider-pub".to_owned(),
            manifest_hash: "manifest".to_owned(),
            binary_hash: "binary".to_owned(),
            att_tier: 2,
            hw_quote: None,
            boot_epoch: 1,
            report_ts: 2,
            nonce_u: "aa".repeat(32),
        };
        let base = hardware_quote_binding(&body).unwrap();
        body.hw_quote = Some(HardwareQuote {
            kind: HardwareQuoteKind::MockTier2,
            evidence: "mock".to_owned(),
            binding: base.clone(),
            endorsements: Vec::new(),
        });
        assert_eq!(hardware_quote_binding(&body).unwrap(), base);
        body.nonce_u = "bb".repeat(32);
        assert_ne!(hardware_quote_binding(&body).unwrap(), base);
    }

    #[test]
    fn voucher_and_receipt_signing_payloads_are_bound_to_terms() {
        let voucher = SpendVoucherBody {
            session_id: "sess".to_owned(),
            enclave_id: "enclave".to_owned(),
            price_ver: 1,
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
            user: "user".to_owned(),
            provider: "provider".to_owned(),
            enclave_id: "enclave".to_owned(),
            model_id: "model".to_owned(),
            price_ver: 1,
            rules_ver: 1,
            usage: ReceiptUsage {
                in_tokens: 3,
                out_tokens: 5,
            },
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
}
