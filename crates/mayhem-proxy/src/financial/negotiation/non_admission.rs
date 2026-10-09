//! Permanent, bounded signing fences. Missing signed rows never prove absence;
//! only an unsigned intent retained before the owner hook can be fenced closed.
use super::*;
use crate::buyer_controller::RequestIdentity;
use mayhem_proto::proxy::finance::ProxySpendTerms;

pub(super) const FENCES: TableDefinition<&str, &[u8]> =
    TableDefinition::new("proxy_negotiation_signing_fences_v2");
const MAX_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Unsigned,
    Signed,
    Fenced,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Fence {
    terms: ProxySpendTerms,
    purchase: Digest,
    state: State,
    allocated_bytes: u64,
}
impl Fence {
    fn identity(&self) -> Result<RequestIdentity> {
        Ok(RequestIdentity {
            billing_id: Digest::new(&self.terms.billing_id)
                .map_err(|_| invalid("buyer intent identity differs"))?,
            billing_attempt: self.terms.billing_attempt,
            session_id: Digest::new(&self.terms.session_id)
                .map_err(|_| invalid("buyer intent identity differs"))?,
            request_hash: Digest::new(&self.terms.request_hash)
                .map_err(|_| invalid("buyer intent identity differs"))?,
        })
    }
    fn proof(&self) -> Result<NonAdmission> {
        require(self.state == State::Fenced, "buyer intent is not fenced")?;
        let commitment = Digest::hash(
            "mayhem/proxy/buyer-non-admission/v1",
            &[
                self.terms.digest().map_err(invalid)?.as_bytes(),
                self.purchase.as_str().as_bytes(),
            ],
        );
        Ok(NonAdmission {
            identity: self.identity()?,
            terms: self.terms.clone(),
            commitment,
        })
    }
}

/// Evidence from the buyer's durable signing authority, not a claim supplied by
/// a provider or HTTP caller. It proves this exact intent cannot be signed; it
/// neither authorizes a replacement purchase nor represents canonical payment.
pub struct NonAdmission {
    identity: RequestIdentity,
    terms: ProxySpendTerms,
    commitment: Digest,
}
impl NonAdmission {
    pub fn identity(&self) -> &RequestIdentity {
        &self.identity
    }
    pub fn terms(&self) -> &ProxySpendTerms {
        &self.terms
    }
    pub fn commitment(&self) -> &Digest {
        &self.commitment
    }
}

impl Store {
    pub(super) fn has_signing_fence(&self, billing_id: &Digest, attempt: u64) -> Result<bool> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| invalid("negotiation writer poisoned"))?;
        require(
            !self.failed.load(Ordering::Acquire),
            "negotiation storage needs recovery after failed commit",
        )?;
        let slot = self.slot(billing_id, attempt)?;
        let tx = crate::db(self.database.begin_read())?;
        let table = crate::db(tx.open_table(FENCES))?;
        let present = crate::db(table.get(slot.as_str()))?.is_some();
        Ok(present)
    }
    fn decode_fence(&self, slot: &Digest, bytes: &[u8]) -> Result<Fence> {
        require(
            bytes.len() <= MAX_BYTES,
            "buyer signing fence exceeds bound",
        )?;
        let fence: Fence = serde_json::from_slice(bytes)?;
        let t = &fence.terms;
        require(
            key(t)? == *slot
                && t.network_id == self.identity.network_id
                && t.msb_bootstrap == self.identity.msb_bootstrap.as_str()
                && t.subnet_bootstrap == self.identity.subnet_bootstrap.as_str()
                && t.buyer_pubkey == self.identity.controller_pubkey.as_str()
                && bytes.len() as u64 <= fence.allocated_bytes
                && fence.allocated_bytes <= MAX_BYTES as u64,
            "buyer signing fence binding differs",
        )?;
        Ok(fence)
    }
    fn read_fence(&self, tx: &redb::WriteTransaction, slot: &Digest) -> Result<Option<Fence>> {
        let table = crate::db(tx.open_table(FENCES))?;
        let fence = crate::db(table.get(slot.as_str()))?
            .map(|v| self.decode_fence(slot, v.value()))
            .transpose();
        fence
    }
    fn replace_fence(
        &self,
        tx: &redb::WriteTransaction,
        slot: &Digest,
        fence: &Fence,
    ) -> Result<()> {
        let bytes = serde_json::to_vec(fence)?;
        self.decode_fence(slot, &bytes)?;
        let old = self.read_fence(tx, slot)?;
        if let Some(old) = &old {
            require(
                old.allocated_bytes == fence.allocated_bytes
                    && old.terms == fence.terms
                    && old.purchase == fence.purchase,
                "buyer signing fence changed intent",
            )?;
        } else {
            let mut meta = metadata(tx)?;
            meta.fences = meta
                .fences
                .checked_add(1)
                .ok_or_else(|| invalid("buyer fence quota overflow"))?;
            meta.fence_bytes = meta
                .fence_bytes
                .checked_add(fence.allocated_bytes)
                .ok_or_else(|| invalid("buyer fence quota overflow"))?;
            require(
                meta.fences <= self.limits.max_records
                    && meta
                        .bytes
                        .checked_add(meta.fence_bytes)
                        .is_some_and(|n| n <= self.limits.max_payload_bytes),
                "buyer signing fence quota exhausted",
            )?;
            crate::db(
                crate::db(tx.open_table(META))?
                    .insert("state", serde_json::to_vec(&meta)?.as_slice()),
            )?;
        }
        crate::db(crate::db(tx.open_table(FENCES))?.insert(slot.as_str(), bytes.as_slice()))?;
        Ok(())
    }
    fn new_fence(terms: ProxySpendTerms, purchase: Digest, state: State) -> Result<Fence> {
        let mut fence = Fence {
            terms,
            purchase,
            state,
            allocated_bytes: 0,
        };
        fence.allocated_bytes = (serde_json::to_vec(&fence)?.len() as u64)
            .checked_add(128)
            .ok_or_else(|| invalid("buyer fence allocation overflow"))?;
        require(
            fence.allocated_bytes <= MAX_BYTES as u64,
            "buyer signing fence exceeds bound",
        )?;
        Ok(fence)
    }
    pub(super) fn retain_unsigned(&self, terms: ProxySpendTerms, purchase: Digest) -> Result<()> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| invalid("negotiation writer poisoned"))?;
        let slot = key(&terms)?;
        let tx = self.write()?;
        require(
            self.read_fence(&tx, &slot)?.is_none()
                && crate::db(crate::db(tx.open_table(RECORDS))?.get(slot.as_str()))?.is_none(),
            "buyer purchase already retained; recover original intent",
        )?;
        let fence = Self::new_fence(terms, purchase, State::Unsigned)?;
        self.replace_fence(&tx, &slot, &fence)?;
        self.commit(tx)
    }
    /// Called inside the same transaction and writer lock as signature retention.
    pub(super) fn retain_signed_fence(
        &self,
        tx: &redb::WriteTransaction,
        slot: &Digest,
        terms: &ProxySpendTerms,
        purchase: &Digest,
        signed_record_exists: bool,
    ) -> Result<()> {
        let mut fence = match self.read_fence(tx, slot)? {
            Some(fence) => {
                require(
                    fence.terms == *terms
                        && fence.purchase == *purchase
                        && fence.state != State::Fenced
                        && (fence.state != State::Signed || signed_record_exists),
                    "buyer purchase was signed or permanently fenced; recover original intent",
                )?;
                fence
            }
            None => Self::new_fence(terms.clone(), purchase.clone(), State::Signed)?,
        };
        fence.state = State::Signed;
        self.replace_fence(tx, slot, &fence)
    }
    pub(super) fn fence_unsigned(
        &self,
        identity: &RequestIdentity,
    ) -> Result<Option<NonAdmission>> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| invalid("negotiation writer poisoned"))?;
        let slot = self.slot(&identity.billing_id, identity.billing_attempt)?;
        let tx = self.write()?;
        let Some(mut fence) = self.read_fence(&tx, &slot)? else {
            return Ok(None);
        };
        require(
            fence.identity()? == *identity,
            "buyer non-admission identity differs",
        )?;
        if fence.state == State::Signed {
            return Ok(None);
        }
        require(
            crate::db(crate::db(tx.open_table(RECORDS))?.get(slot.as_str()))?.is_none(),
            "signed buyer record conflicts with unsigned intent",
        )?;
        if fence.state == State::Fenced {
            return fence.proof().map(Some);
        }
        fence.state = State::Fenced;
        self.replace_fence(&tx, &slot, &fence)?;
        self.commit(tx)?;
        fence.proof().map(Some)
    }
    pub(super) fn slot(&self, billing_id: &Digest, attempt: u64) -> Result<Digest> {
        require(
            attempt > 0 && attempt <= PROXY_MAX_SAFE_INTEGER,
            "invalid purchase attempt",
        )?;
        let id = &self.identity;
        Ok(Digest::hash(
            "mayhem/proxy/buyer-purchase-slot/v1",
            &[
                id.network_id.as_bytes(),
                id.msb_bootstrap.as_str().as_bytes(),
                id.subnet_bootstrap.as_str().as_bytes(),
                id.controller_pubkey.as_str().as_bytes(),
                billing_id.as_str().as_bytes(),
                &attempt.to_le_bytes(),
            ],
        ))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Barrier;
    fn terms() -> ProxySpendTerms {
        let h = "1".repeat(64);
        let mut value = serde_json::json!({
            "schema_version":1,"lane":"proxy","network_id":"fence-test","msb_bootstrap":h,
            "subnet_bootstrap":h,"contract_version":1,"buyer_pubkey":h,"billing_id":h,
            "billing_attempt":1,"session_id":h,"reservation_id":h,"billing_epoch":1,
            "acceptance_expires_after_epoch":2,"reservation_expires_after_epoch":3,"reservation_receipt_grace_epochs":1,
            "payout_revision":h,"request_hash":h,"endpoint_contract":h,"recipe_hash":h,"connection_digest":h
        });
        let rest = serde_json::json!({
            "connection_revision":1,"capacity_lease":h,"rail":"fiat","served_context":1,
            "settlement_policy_hash":h,"payment_terms_hash":h,"rules_ver":1,
            "max_usage":{"tok_out":1},"max_spend_au":"1","prior_spend_au":"0","prior_reserved_au":"0","max_total_spend_au":"1"
        });
        value
            .as_object_mut()
            .unwrap()
            .extend(rest.as_object().unwrap().clone());
        value["offer"] = serde_json::json!({"schema_version":1,"lane":"proxy","market_id":h,"provider_pubkey":h,"membership_revision":1,
            "revision":1,"endpoint":"openai_chat_completions","ctx_bracket":"small","outcome_class":"","metering_policy_hash":h,
            "rates":[{"unit":"tok_out","per_unit_au":"1","granularity":1}],"per_request_au":"0","min_session_au":"0","accepted_rails":["fiat"]});
        let terms: ProxySpendTerms = serde_json::from_value(value).unwrap();
        terms.validate().unwrap();
        terms
    }
    fn setup() -> (tempfile::TempDir, Arc<Store>, ProxySpendTerms, Digest) {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let terms = terms();
        let id = attempts::Identity {
            network_id: terms.network_id.clone(),
            msb_bootstrap: Digest::new(&terms.msb_bootstrap).unwrap(),
            subnet_bootstrap: Digest::new(&terms.subnet_bootstrap).unwrap(),
            controller_pubkey: Digest::new(&terms.buyer_pubkey).unwrap(),
        };
        let store = Store::open(
            directory.path().join("buyer.redb"),
            id,
            Limits {
                max_records: 1,
                max_payload_bytes: 131072,
                max_record_bytes: 65536,
                closed_retention_ms: 1,
            },
        )
        .unwrap();
        (
            directory,
            Arc::new(store),
            terms,
            Digest::new("2".repeat(64)).unwrap(),
        )
    }
    #[test]
    fn unsigned_fence_survives_reopen_requires_retained_intent_and_preserves_quota() {
        let (directory, store, terms, purchase) = setup();
        let identity = Store::new_fence(terms.clone(), purchase.clone(), State::Unsigned)
            .unwrap()
            .identity()
            .unwrap();
        assert!(store.fence_unsigned(&identity).unwrap().is_none());
        store
            .retain_unsigned(terms.clone(), purchase.clone())
            .unwrap();
        let owner = store.identity.clone();
        let limits = store.limits;
        drop(store);
        let store =
            Store::open_existing(directory.path().join("buyer.redb"), owner, limits).unwrap();
        let proof = store.fence_unsigned(&identity).unwrap().unwrap();
        let commitment = proof.commitment().clone();
        assert_eq!(proof.terms(), &terms);
        assert_eq!(proof.identity(), &identity);
        assert_eq!(
            store
                .fence_unsigned(&identity)
                .unwrap()
                .unwrap()
                .commitment(),
            &commitment
        );
        assert_eq!(store.prune(100000, 64).unwrap(), 0);
        assert!(store
            .retain_unsigned(terms.clone(), purchase.clone())
            .is_err());
        let mut other = terms.clone();
        other.billing_id = "3".repeat(64);
        assert!(store.retain_unsigned(other, purchase.clone()).is_err());
        let tx = store.write().unwrap();
        assert!(store
            .retain_signed_fence(&tx, &key(&terms).unwrap(), &terms, &purchase, false)
            .is_err());
        drop(tx);
        store.failed.store(true, Ordering::Release);
        assert!(
            store.fence_unsigned(&identity).is_err(),
            "ambiguous storage never exposes proof"
        );
    }
    #[test]
    fn unsigned_fence_and_signing_transition_have_one_atomic_winner() {
        for _ in 0..16 {
            let (_directory, store, terms, purchase) = setup();
            let identity = Store::new_fence(terms.clone(), purchase.clone(), State::Unsigned)
                .unwrap()
                .identity()
                .unwrap();
            store
                .retain_unsigned(terms.clone(), purchase.clone())
                .unwrap();
            let barrier = Arc::new(Barrier::new(2));
            let signer_store = store.clone();
            let signer_barrier = barrier.clone();
            let signing = std::thread::spawn(move || {
                signer_barrier.wait();
                let _guard = signer_store.lock.lock().unwrap();
                let tx = signer_store.write().unwrap();
                if signer_store
                    .retain_signed_fence(&tx, &key(&terms).unwrap(), &terms, &purchase, false)
                    .is_err()
                {
                    return false;
                }
                signer_store.commit(tx).unwrap();
                true
            });
            barrier.wait();
            let fenced = store.fence_unsigned(&identity).unwrap().is_some();
            let signed = signing.join().unwrap();
            assert_ne!(signed, fenced);
            assert_eq!(store.fence_unsigned(&identity).unwrap().is_some(), fenced);
        }
    }
}
