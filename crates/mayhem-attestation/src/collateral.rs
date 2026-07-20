use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use mayhem_proto::AttestationTrustDataRef;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;

use crate::ValidatedAttestationPolicy;

pub const MAX_COLLATERAL_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Error, Eq, PartialEq)]
pub enum CollateralError {
    #[error("attestation policy does not define trust data {0}")]
    UnknownTrustData(String),
    #[error("trust data {id} is empty")]
    Empty { id: String },
    #[error("trust data {id} is {actual} bytes; policy maximum is {maximum}")]
    TooLarge {
        id: String,
        actual: usize,
        maximum: u64,
    },
    #[error("trust data {id} SHA-256 mismatch: expected {expected}, actual {actual}")]
    DigestMismatch {
        id: String,
        expected: String,
        actual: String,
    },
    #[error("invalid content-addressed SHA-256 digest")]
    InvalidDigest,
}

#[derive(Clone, Debug)]
pub struct ValidatedCollateral {
    sha256: String,
    bytes: Arc<[u8]>,
    observed_epoch: u64,
}

impl ValidatedCollateral {
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn observed_epoch(&self) -> u64 {
        self.observed_epoch
    }
}

#[derive(Clone, Debug, Default)]
pub struct CollateralInventory {
    by_digest: BTreeMap<String, ValidatedCollateral>,
}

impl CollateralInventory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        policy: &ValidatedAttestationPolicy,
        trust_data_id: &str,
        bytes: impl AsRef<[u8]>,
        observed_epoch: u64,
    ) -> Result<&ValidatedCollateral, CollateralError> {
        let reference = policy
            .trust_data(trust_data_id)
            .ok_or_else(|| CollateralError::UnknownTrustData(trust_data_id.to_owned()))?;
        self.insert_reference(reference, bytes, observed_epoch)
    }

    pub fn insert_reference(
        &mut self,
        reference: &AttestationTrustDataRef,
        bytes: impl AsRef<[u8]>,
        observed_epoch: u64,
    ) -> Result<&ValidatedCollateral, CollateralError> {
        let bytes = bytes.as_ref();
        validate_collateral_bytes(reference, bytes)?;
        let digest = reference.sha256.clone();
        self.by_digest
            .entry(digest.clone())
            .and_modify(|existing| {
                existing.observed_epoch = existing.observed_epoch.min(observed_epoch);
            })
            .or_insert_with(|| ValidatedCollateral {
                sha256: digest.clone(),
                bytes: Arc::from(bytes),
                observed_epoch,
            });
        Ok(self
            .by_digest
            .get(&digest)
            .expect("inserted collateral digest"))
    }

    pub fn get(&self, sha256: &str) -> Option<&ValidatedCollateral> {
        self.by_digest.get(sha256)
    }

    pub fn contains_reference(&self, reference: &AttestationTrustDataRef) -> bool {
        self.by_digest.contains_key(&reference.sha256)
    }

    pub fn contains_reference_at(
        &self,
        reference: &AttestationTrustDataRef,
        ledger_epoch: u64,
    ) -> bool {
        self.by_digest
            .get(&reference.sha256)
            .is_some_and(|collateral| collateral.observed_epoch <= ledger_epoch)
    }

    pub fn len(&self) -> usize {
        self.by_digest.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_digest.is_empty()
    }
}

pub fn collateral_cache_relative_path(sha256: &str) -> Result<PathBuf, CollateralError> {
    if !valid_sha256(sha256) {
        return Err(CollateralError::InvalidDigest);
    }
    Ok(PathBuf::from("sha256").join(&sha256[..2]).join(sha256))
}

fn validate_collateral_bytes(
    reference: &AttestationTrustDataRef,
    bytes: &[u8],
) -> Result<(), CollateralError> {
    if bytes.is_empty() {
        return Err(CollateralError::Empty {
            id: reference.id.clone(),
        });
    }
    if bytes.len() as u64 > reference.max_bytes {
        return Err(CollateralError::TooLarge {
            id: reference.id.clone(),
            actual: bytes.len(),
            maximum: reference.max_bytes,
        });
    }
    let actual_bytes = Sha256::digest(bytes);
    let expected_bytes =
        hex::decode(&reference.sha256).map_err(|_| CollateralError::InvalidDigest)?;
    if expected_bytes.len() != actual_bytes.len()
        || !bool::from(actual_bytes.as_slice().ct_eq(&expected_bytes))
    {
        return Err(CollateralError::DigestMismatch {
            id: reference.id.clone(),
            expected: reference.sha256.clone(),
            actual: hex::encode(actual_bytes),
        });
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
