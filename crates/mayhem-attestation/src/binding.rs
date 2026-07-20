use mayhem_proto::{
    HardwareQuoteKind, HardwareQuoteRoutePolicyBinding, ATTESTATION_EVIDENCE_BINDING_VERSION,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

const EVIDENCE_BINDING_DOMAIN: &[u8] = b"mayhem-attestation-evidence-binding-v1";

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum EvidenceBindingError {
    #[error("{0} must be a lowercase 32-byte digest")]
    InvalidDigest(&'static str),
    #[error("{0} must be between 1 and 512 visible ASCII bytes")]
    InvalidIdentity(&'static str),
    #[error("attestation evidence schema version must be positive")]
    InvalidEvidenceSchemaVersion,
    #[error("attestation policy sequence must be positive")]
    InvalidPolicySequence,
    #[error("{0} does not match the immutable route policy binding")]
    PolicyIdentityMismatch(&'static str),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceBinding {
    pub kind: HardwareQuoteKind,
    pub evidence_schema_version: u32,
    pub policy_sequence: u64,
    pub policy_digest: String,
    pub platform: Option<String>,
    pub nonce: String,
    pub enclave_id: String,
    pub device_id: String,
    pub quote_binding: String,
}

impl EvidenceBinding {
    pub fn new(
        policy: &HardwareQuoteRoutePolicyBinding,
        nonce: impl Into<String>,
        enclave_id: impl Into<String>,
        device_id: impl Into<String>,
        quote_binding: impl Into<String>,
    ) -> Result<Self, EvidenceBindingError> {
        let enclave_id = enclave_id.into();
        let device_id = device_id.into();
        if enclave_id != policy.enclave_id {
            return Err(EvidenceBindingError::PolicyIdentityMismatch("enclave id"));
        }
        if device_id != policy.device_id {
            return Err(EvidenceBindingError::PolicyIdentityMismatch("device id"));
        }
        let binding = Self {
            kind: policy.kind,
            evidence_schema_version: policy.evidence_schema_version,
            policy_sequence: policy.policy_sequence,
            policy_digest: policy.policy_digest.clone(),
            platform: policy.platform.clone(),
            nonce: nonce.into(),
            enclave_id,
            device_id,
            quote_binding: quote_binding.into(),
        };
        binding.validate()?;
        Ok(binding)
    }

    pub fn digest(&self) -> Result<[u8; 32], EvidenceBindingError> {
        self.validate()?;
        let mut hash = Sha256::new();
        update_field(&mut hash, EVIDENCE_BINDING_DOMAIN);
        update_field(
            &mut hash,
            &ATTESTATION_EVIDENCE_BINDING_VERSION.to_be_bytes(),
        );
        update_field(&mut hash, self.kind.as_str().as_bytes());
        update_field(&mut hash, &self.evidence_schema_version.to_be_bytes());
        update_field(&mut hash, &self.policy_sequence.to_be_bytes());
        update_field(&mut hash, self.policy_digest.as_bytes());
        update_field(
            &mut hash,
            self.platform.as_deref().unwrap_or_default().as_bytes(),
        );
        update_field(&mut hash, self.nonce.as_bytes());
        update_field(&mut hash, self.enclave_id.as_bytes());
        update_field(&mut hash, self.device_id.as_bytes());
        update_field(&mut hash, self.quote_binding.as_bytes());
        Ok(hash.finalize().into())
    }

    fn validate(&self) -> Result<(), EvidenceBindingError> {
        if self.evidence_schema_version == 0 {
            return Err(EvidenceBindingError::InvalidEvidenceSchemaVersion);
        }
        if self.policy_sequence == 0 {
            return Err(EvidenceBindingError::InvalidPolicySequence);
        }
        for (name, digest) in [
            ("policy digest", self.policy_digest.as_str()),
            ("nonce", self.nonce.as_str()),
            ("device id", self.device_id.as_str()),
            ("quote binding", self.quote_binding.as_str()),
        ] {
            if !valid_sha256(digest) {
                return Err(EvidenceBindingError::InvalidDigest(name));
            }
        }
        for (name, identity) in [
            ("enclave id", self.enclave_id.as_str()),
            ("platform", self.platform.as_deref().unwrap_or("none")),
        ] {
            if identity.is_empty()
                || identity.len() > 512
                || !identity.bytes().all(|byte| byte.is_ascii_graphic())
            {
                return Err(EvidenceBindingError::InvalidIdentity(name));
            }
        }
        Ok(())
    }
}

fn update_field(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_be_bytes());
    hash.update(value);
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use mayhem_proto::{
        hardware_quote_binding, AttestationBody, AttestationRuntimeConfig, HardwareQuoteKind,
        HardwareQuoteRoutePolicyBinding, ATTESTATION_ALG, ATTESTATION_SCHEMA_VERSION,
    };

    use super::{EvidenceBinding, EvidenceBindingError};

    #[test]
    fn every_quote_kind_binds_the_measured_runtime_hash() {
        for kind in HardwareQuoteKind::ALL {
            let first_quote_binding = quote_binding(kind, &"aa".repeat(32));
            let second_quote_binding = quote_binding(kind, &"bb".repeat(32));
            assert_ne!(
                first_quote_binding, second_quote_binding,
                "{kind:?} must bind the measured runtime hash"
            );

            let policy = policy_binding(kind, 1);
            let first = EvidenceBinding::new(
                &policy,
                "11".repeat(32),
                "55".repeat(32),
                "22".repeat(32),
                first_quote_binding,
            )
            .unwrap();
            let second = EvidenceBinding::new(
                &policy,
                "11".repeat(32),
                "55".repeat(32),
                "22".repeat(32),
                second_quote_binding,
            )
            .unwrap();
            assert_ne!(
                first.digest().unwrap(),
                second.digest().unwrap(),
                "{kind:?} policy-bound evidence must retain the runtime measurement"
            );
        }
    }

    #[test]
    fn zero_policy_sequence_is_rejected_at_the_evidence_boundary() {
        let error = EvidenceBinding::new(
            &policy_binding(HardwareQuoteKind::Tpm2QuoteEk, 0),
            "11".repeat(32),
            "55".repeat(32),
            "22".repeat(32),
            "33".repeat(32),
        )
        .unwrap_err();

        assert_eq!(error, EvidenceBindingError::InvalidPolicySequence);
    }

    #[test]
    fn evidence_identity_must_match_the_immutable_route_binding() {
        let policy = policy_binding(HardwareQuoteKind::Tpm2QuoteEk, 1);
        assert_eq!(
            EvidenceBinding::new(
                &policy,
                "11".repeat(32),
                "66".repeat(32),
                "22".repeat(32),
                "33".repeat(32),
            )
            .unwrap_err(),
            EvidenceBindingError::PolicyIdentityMismatch("enclave id")
        );
        assert_eq!(
            EvidenceBinding::new(
                &policy,
                "11".repeat(32),
                "55".repeat(32),
                "66".repeat(32),
                "33".repeat(32),
            )
            .unwrap_err(),
            EvidenceBindingError::PolicyIdentityMismatch("device id")
        );
    }

    fn policy_binding(
        kind: HardwareQuoteKind,
        policy_sequence: u64,
    ) -> HardwareQuoteRoutePolicyBinding {
        HardwareQuoteRoutePolicyBinding {
            enclave_id: "55".repeat(32),
            device_id: "22".repeat(32),
            kind,
            evidence_schema_version: 1,
            policy_sequence,
            policy_digest: "44".repeat(32),
            platform: None,
        }
    }

    fn quote_binding(kind: HardwareQuoteKind, binary_hash: &str) -> String {
        hardware_quote_binding(&AttestationBody {
            schema_version: ATTESTATION_SCHEMA_VERSION,
            alg: ATTESTATION_ALG.to_owned(),
            enclave_id: "enclave-id".to_owned(),
            enclave_pubkey: "55".repeat(32),
            provider_pubkey: "66".repeat(32),
            manifest_hash: "77".repeat(32),
            binary_hash: binary_hash.to_owned(),
            att_tier: kind.attestation_tier(),
            hw_quote: None,
            boot_epoch: 1,
            report_ts: 2,
            nonce_u: "11".repeat(32),
            runtime_config: AttestationRuntimeConfig::default(),
        })
        .unwrap()
    }
}
