use std::collections::BTreeMap;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use mayhem_attestation::ValidatedAttestationPolicy;
use mayhem_proto::{AttestationTrustDataKind, HardwareQuoteKind};
use sha2::{Digest, Sha256};

use crate::{reject, Result};

const MAX_ENDORSEMENTS: usize = 16;
const MAX_ENDORSEMENT_BYTES: usize = 256 * 1024;
const MAX_TOTAL_ENDORSEMENT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct AuthenticatedItem {
    pub id: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
    pub source_url: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AuthenticatedCollateral {
    items: BTreeMap<String, AuthenticatedItem>,
}

impl AuthenticatedCollateral {
    pub fn from_endorsements(
        policy: &ValidatedAttestationPolicy,
        kind: HardwareQuoteKind,
        endorsements: &[String],
    ) -> Result<Self> {
        if endorsements.len() > MAX_ENDORSEMENTS {
            return reject("provider supplied too many managed-verifier endorsements");
        }
        let kind_policy = policy
            .quote_kind(kind)
            .ok_or_else(|| crate::VerifyError::Rejected("admin policy has no quote kind".into()))?;
        let mut items = BTreeMap::new();
        let mut total = 0usize;
        for encoded in endorsements {
            if encoded.len() > MAX_ENDORSEMENT_BYTES.saturating_mul(2) {
                return reject("managed-verifier endorsement exceeds its encoded size limit");
            }
            let bytes = BASE64
                .decode(encoded)
                .map_err(|_| crate::VerifyError::Rejected("endorsement is not base64".into()))?;
            if bytes.is_empty() || bytes.len() > MAX_ENDORSEMENT_BYTES {
                return reject("managed-verifier endorsement has an invalid size");
            }
            total = total
                .checked_add(bytes.len())
                .ok_or_else(|| crate::VerifyError::Rejected("endorsement size overflow".into()))?;
            if total > MAX_TOTAL_ENDORSEMENT_BYTES {
                return reject("managed-verifier endorsements exceed the aggregate size limit");
            }
            let digest = hex::encode(Sha256::digest(&bytes));
            let reference = policy
                .policy()
                .trust_data
                .iter()
                .find(|reference| {
                    reference.sha256 == digest
                        && kind_policy.required_trust_data.contains(&reference.id)
                        && !matches!(reference.kind, AttestationTrustDataKind::Measurement)
                })
                .ok_or_else(|| {
                    crate::VerifyError::Rejected(
                        "provider supplied a root/JWKS/collateral item not authenticated by admin policy"
                            .into(),
                    )
                })?;
            if bytes.len() as u64 > reference.max_bytes {
                return reject(format!(
                    "authenticated collateral {} exceeds its admin bound",
                    reference.id
                ));
            }
            if items.contains_key(&reference.id) {
                return reject(format!(
                    "authenticated collateral {} was duplicated",
                    reference.id
                ));
            }
            items.insert(
                reference.id.clone(),
                AuthenticatedItem {
                    id: reference.id.clone(),
                    media_type: reference.media_type.clone(),
                    bytes,
                    source_url: policy.policy_source_url(&reference.id),
                },
            );
        }
        Ok(Self { items })
    }

    pub fn for_kind<'a>(
        &'a self,
        policy: &'a ValidatedAttestationPolicy,
        kind: HardwareQuoteKind,
    ) -> impl Iterator<Item = &'a AuthenticatedItem> + 'a {
        let required = policy
            .quote_kind(kind)
            .map(|entry| &entry.required_trust_data);
        self.items
            .values()
            .filter(move |item| required.is_some_and(|ids| ids.contains(&item.id)))
    }
}
