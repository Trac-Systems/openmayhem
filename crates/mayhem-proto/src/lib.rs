#![forbid(unsafe_code)]

use std::fmt;

use serde::{Deserialize, Serialize};

pub const CRATE_NAME: &str = "mayhem-proto";
pub const CONTRACT_VERSION: u32 = 1;
pub const ATTESTATION_SCHEMA_VERSION: u32 = 1;
pub const ATTESTATION_ALG: &str = "ed25519";
pub const SESSION_RECEIPT_SCHEMA_VERSION: u32 = 1;
pub const NEXT_SESSION_RECEIPT_SCHEMA_VERSION: u32 = 2;
pub const SIGNING_MESSAGE_VERSION: u32 = 2;
pub const SUPPORTED_SIGNING_MESSAGE_VERSIONS: &[u32] = &[SIGNING_MESSAGE_VERSION, 1];
pub const HARDWARE_QUOTE_BINDING_DOMAIN: &str = "mayhem-hardware-quote-binding-v1";
pub const SESSION_ACCEPT_SIGNING_DOMAIN: &str = "mayhem/session-accept/v1";
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
            backend: "unknown".to_owned(),
            ctx: 0,
            tp_degree: 1,
            max_batch_size: None,
            max_num_tokens: None,
        }
    }
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
    MockDeviceIdentity,
}

impl HardwareQuoteKind {
    pub fn attestation_tier(&self) -> u8 {
        match self {
            Self::AppleAppAttestJwt | Self::NvidiaGb10DeviceJwt | Self::MockDeviceIdentity => {
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
            1 => migrated.schema_version = 2,
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
            kind: HardwareQuoteKind::MockDeviceIdentity,
            evidence: "mock".to_owned(),
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

    #[test]
    fn receipt_schema_migration_accepts_v1_for_v2_nodes() {
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
            ReceiptSchemaMigrationError::Unsupported { from: 99, to: 2 }
        );

        assert_eq!(
            migrate_receipt_body_to_schema(&migrated, SESSION_RECEIPT_SCHEMA_VERSION).unwrap_err(),
            ReceiptSchemaMigrationError::Unsupported { from: 2, to: 1 }
        );
    }

    #[test]
    fn signing_payloads_are_versioned_and_keep_legacy_payloads_supported() {
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
}
