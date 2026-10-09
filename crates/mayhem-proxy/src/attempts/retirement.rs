//! Canonically never-admitted provider obligations. Keep the execution fence and
//! proof durable before releasing capacity; close only after capacity is released.
use super::*;
use crate::{
    capacity,
    financial::{intent, negotiation::BuyerOffer, terms_binding},
};

const TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_provider_retirement_v1");
const MAX_BYTES: usize = 64 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Retirement {
    attempt: u64,
    buyer: BuyerOffer,
    lease: capacity::Lease,
    absence: intent::RetainedAbsence,
}
impl Retirement {
    pub(crate) fn attempt(&self) -> u64 {
        self.attempt
    }
    pub(crate) fn buyer(&self) -> &BuyerOffer {
        &self.buyer
    }
    pub(crate) fn lease(&self) -> &capacity::Lease {
        &self.lease
    }
    pub(crate) fn validate(&self, identity: &Identity) -> Result<()> {
        self.buyer.verify().map_err(|_| Error::Invalid)?;
        let t = &self.buyer.terms;
        self.absence
            .validate_for(t, identity.controller_pubkey.as_str())
            .map_err(|_| Error::Invalid)?;
        require(
            t.network_id == identity.network_id
                && t.msb_bootstrap == identity.msb_bootstrap.as_str()
                && t.subnet_bootstrap == identity.subnet_bootstrap.as_str()
                && t.offer.provider_pubkey == identity.controller_pubkey.as_str()
                && t.capacity_lease == self.lease.id.as_str()
                && t.request_hash == self.lease.work.request_hash.as_str()
                && crate::exchange::invocation_for_terms(t).map_err(|_| Error::Invalid)?
                    == self.lease.work.invocation,
        )
    }
    fn validate_record(&self, record: &Record, identity: &Identity) -> Result<()> {
        self.validate(identity)?;
        require(
            self.attempt > 0
                && record.attempt == self.attempt
                && record.binding
                    == terms_binding(&self.buyer.terms).map_err(|_| Error::Invalid)?
                && record.invocation == self.lease.work.invocation
                && record.unsent_cancellation_evidence().is_some(),
        )
    }
    pub(crate) fn closure(&self) -> Result<Digest> {
        Ok(Digest::hash(
            "mayhem/proxy/never-admitted-provider/v1",
            &[&serde_json::to_vec(self).map_err(|_| Error::Invalid)?],
        ))
    }
}
pub(super) fn initialize(tx: &redb::WriteTransaction, create: bool) -> Result<()> {
    let exists = storage(tx.list_tables())?.any(|t| t.name() == TABLE.name());
    require(exists != create)?;
    require(
        storage(storage(tx.open_table(TABLE))?.len())?
            <= storage(storage(tx.open_table(RECORDS))?.len())?,
    )
}
pub(super) fn has(tx: &redb::ReadTransaction, record: &Record) -> Result<bool> {
    Ok(storage(storage(tx.open_table(TABLE))?.get(record.key().as_str()))?.is_some())
}
pub(super) fn prune(tx: &redb::WriteTransaction, key: &str, meta: &mut Meta) -> Result<()> {
    if let Some(v) = storage(storage(tx.open_table(TABLE))?.remove(key))? {
        meta.payload_bytes = meta
            .payload_bytes
            .checked_sub(v.value().len() as u64)
            .ok_or(Error::Invalid)?;
    }
    Ok(())
}
fn decode_retired(bytes: &[u8], record: &Record, identity: &Identity) -> Result<Retirement> {
    require(bytes.len() <= MAX_BYTES)?;
    let saved: Retirement = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
    saved.validate_record(record, identity)?;
    Ok(saved)
}
impl Journal {
    /// A fresh canonical proof and the exact retained signing intention are both
    /// required. Neither a missing journal row nor elapsed time substitutes.
    pub(crate) fn retire_provider(
        &self,
        original: &capacity::SigningIntent,
        observation: &intent::Observation,
        now_ms: u64,
    ) -> Result<Retirement> {
        let identity = self.identity()?;
        let mut saved = Retirement {
            attempt: 0,
            buyer: original.buyer().clone(),
            lease: original.lease().clone(),
            absence: observation
                .absent(original.buyer(), identity.controller_pubkey.as_str())
                .map_err(|_| Error::Conflict)?,
        };
        saved.validate(&identity)?;
        let binding = terms_binding(&saved.buyer.terms).map_err(|_| Error::Invalid)?;
        let tx = self.transaction()?;
        let mut record = match current(&tx, &saved.lease.work.invocation)? {
            Some(r) => r,
            None => self.insert(
                &tx,
                saved.lease.work.invocation.clone(),
                binding.clone(),
                now_ms,
            )?,
        };
        saved.attempt = record.attempt;
        require(record.binding == binding && now_ms >= record.updated_at_ms)?;
        let mut table = storage(tx.open_table(TABLE))?;
        if let Some(v) = storage(table.get(record.key().as_str()))? {
            let old = decode_retired(v.value(), &record, &identity)?;
            require(old.buyer == saved.buyer && old.lease.id == saved.lease.id)?;
            return Ok(old);
        }
        // Admission or any possible dispatch belongs to execution reconciliation.
        require(
            !finance::has(&tx, &record.key())?
                && !outcomes::has_intent(&tx, &record.key())?
                && (record.phase == Phase::Prepared
                    || (record.phase == Phase::Resolved
                        && record.unsent_cancellation_evidence().is_some())),
        )?;
        record.cancellation_requested = true;
        record.phase = Phase::Resolved;
        record.resolution = Some(Resolution::NotExecuted {
            evidence: unsent_commitment(&record.invocation, record.attempt),
        });
        saved.validate_record(&record, &identity)?;
        let bytes = serde_json::to_vec(&saved).map_err(|_| Error::Invalid)?;
        require(bytes.len() <= MAX_BYTES)?;
        let mut meta = metadata(&tx)?;
        meta.payload_bytes = meta
            .payload_bytes
            .checked_add(bytes.len() as u64)
            .ok_or(Error::Capacity)?;
        if meta.payload_bytes > self.limits.max_payload_bytes {
            return Err(Error::Capacity);
        }
        storage(table.insert(record.key().as_str(), bytes.as_slice()))?;
        save_meta(&tx, &meta)?;
        bump(&tx, &mut record, now_ms)?;
        save(&tx, &record)?;
        drop(table);
        observation.fresh().map_err(|_| Error::Conflict)?;
        self.commit(tx)?;
        Ok(saved)
    }
    pub(crate) fn provider_retirement(
        &self,
        invocation: &Digest,
        attempt: u64,
    ) -> Result<Option<Retirement>> {
        let _guard = self.write_lock.lock().map_err(|_| Error::Storage)?;
        if self.commit_failed.load(Ordering::Acquire) {
            return Err(Error::Storage);
        }
        let tx = storage(self.database.begin_read())?;
        let record = read_record(&storage(tx.open_table(RECORDS))?, invocation, attempt)?;
        let table = storage(tx.open_table(TABLE))?;
        storage(table.get(record.key().as_str()))?
            .map(|v| decode_retired(v.value(), &record, &self.identity()?))
            .transpose()
    }
}
