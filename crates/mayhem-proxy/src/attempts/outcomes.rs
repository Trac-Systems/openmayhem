//! One immutable final receipt per owned attempt. Space is reserved before paid
//! dispatch; drafts and signatures share the journal's durability and pruning.
use super::*;
use mayhem_proto::proxy::finance::{ProxyReceiptBody, ProxyUsageReceipt};
const OUTCOMES: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_attempt_outcomes_v1");
const ALLOCATION: u64 = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletedDraft {
    pub invocation: Digest,
    pub attempt: u64,
    pub body: ProxyReceiptBody,
    pub previous: Option<ProxyReceiptBody>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Slot {
    draft: Option<CompletedDraft>,
    receipt: Option<ProxyUsageReceipt>,
}
fn read_slot(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
    key: &str,
) -> Result<Option<Slot>> {
    storage(table.get(key))?
        .map(|v| {
            require(v.value().len() as u64 <= ALLOCATION)?;
            serde_json::from_slice(v.value()).map_err(|_| Error::Invalid)
        })
        .transpose()
}
fn write_slot(table: &mut redb::Table<&str, &[u8]>, key: &str, slot: &Slot) -> Result<()> {
    let bytes = serde_json::to_vec(slot).map_err(|_| Error::Invalid)?;
    require(bytes.len() as u64 <= ALLOCATION)?;
    storage(table.insert(key, bytes.as_slice()))?;
    Ok(())
}
pub(super) fn initialize(tx: &redb::WriteTransaction, create: bool) -> Result<()> {
    let exists = storage(tx.list_tables())?.any(|t| t.name() == OUTCOMES.name());
    require(exists != create)?;
    require(
        storage(storage(tx.open_table(OUTCOMES))?.len())?
            <= storage(storage(tx.open_table(RECORDS))?.len())?,
    )
}
pub(super) fn prune(tx: &redb::WriteTransaction, key: &str, meta: &mut Meta) -> Result<()> {
    if storage(storage(tx.open_table(OUTCOMES))?.remove(key))?.is_some() {
        meta.payload_bytes = meta
            .payload_bytes
            .checked_sub(ALLOCATION)
            .ok_or(Error::Invalid)?;
    }
    Ok(())
}
impl Journal {
    /// Mandatory before new paid dispatch. Migration/recovery of an older attempt
    /// may allocate explicitly if headroom exists; lack of space never drops work.
    pub fn reserve_outcome(&self, invocation: &Digest, attempt: u64) -> Result<()> {
        let tx = self.transaction()?;
        let r = current(&tx, invocation)?.ok_or(Error::NotFound)?;
        require(r.attempt == attempt)?;
        let mut table = storage(tx.open_table(OUTCOMES))?;
        if read_slot(&table, &r.key())?.is_some() {
            return Ok(());
        }
        require(r.phase != Phase::Closed)?;
        let mut meta = metadata(&tx)?;
        meta.payload_bytes = meta
            .payload_bytes
            .checked_add(ALLOCATION)
            .ok_or(Error::Capacity)?;
        if meta.payload_bytes > self.limits.max_payload_bytes {
            return Err(Error::Capacity);
        }
        write_slot(
            &mut table,
            &r.key(),
            &Slot {
                draft: None,
                receipt: None,
            },
        )?;
        save_meta(&tx, &meta)?;
        drop(table);
        self.commit(tx)
    }
    pub fn completed_draft(
        &self,
        invocation: &Digest,
        attempt: u64,
    ) -> Result<Option<CompletedDraft>> {
        let tx = storage(self.database.begin_read())?;
        let records = storage(tx.open_table(RECORDS))?;
        let r = read_record(&records, invocation, attempt)?;
        let table = storage(tx.open_table(OUTCOMES))?;
        let draft = read_slot(&table, &r.key())?.and_then(|s| s.draft);
        if let Some(d) = &draft {
            require(
                d.invocation == r.invocation
                    && d.attempt == r.attempt
                    && d.body.accepted_terms == r.binding.accepted_terms.as_str(),
            )?;
            d.body.validate().map_err(|_| Error::Invalid)?;
        }
        Ok(draft)
    }
    pub fn prepare_completed_draft(
        &self,
        invocation: &Digest,
        attempt: u64,
        at_ms: u64,
        previous: Option<ProxyReceiptBody>,
    ) -> Result<CompletedDraft> {
        if let Some(saved) = self.completed_draft(invocation, attempt)? {
            return Ok(saved);
        }
        let recovery = self.recover(invocation, attempt)?;
        let r = &recovery.record;
        require(
            matches!(r.phase, Phase::Dispatched | Phase::Resolved) && !r.cancellation_requested,
        )?;
        let accepted = recovery
            .financial
            .as_ref()
            .ok_or(Error::Invalid)?
            .accepted();
        let seq = previous
            .as_ref()
            .map_or(Some(1), |v| v.seq.checked_add(1))
            .ok_or(Error::Invalid)?;
        let body = crate::metering::draft_completed_receipt(
            &recovery,
            &accepted.authorization.terms,
            &accepted.settlement_policy,
            seq,
            at_ms,
            previous.as_ref(),
        )
        .map_err(|_| Error::Conflict)?;
        let draft = CompletedDraft {
            invocation: invocation.clone(),
            attempt,
            body,
            previous,
        };
        let tx = self.transaction()?;
        let mut current = current(&tx, invocation)?.ok_or(Error::NotFound)?;
        let mut table = storage(tx.open_table(OUTCOMES))?;
        let mut slot = read_slot(&table, &r.key())?.ok_or(Error::NotFound)?;
        if let Some(saved) = slot.draft {
            return Ok(saved);
        }
        if &current != r {
            return Err(Error::Stale);
        }
        let resolution = Resolution::Completed {
            verified: VerifiedResult {
                result: Digest::new(&draft.body.result_hash)?,
                usage_evidence: Digest::new(&draft.body.observation_hash)?,
            },
        };
        payloads::verify_resolution(&tx, &r.key(), &resolution)?;
        if current.phase == Phase::Dispatched {
            current.phase = Phase::Resolved;
            current.resolution = Some(resolution);
            bump(&tx, &mut current, at_ms)?;
            save(&tx, &current)?;
        } else {
            require(current.resolution.as_ref() == Some(&resolution))?;
        }
        slot.draft = Some(draft.clone());
        write_slot(&mut table, &r.key(), &slot)?;
        drop(table);
        self.commit(tx)?;
        Ok(draft)
    }
    pub fn retain_completed_receipt(
        &self,
        invocation: &Digest,
        attempt: u64,
        receipt: &ProxyUsageReceipt,
    ) -> Result<()> {
        let draft = self
            .completed_draft(invocation, attempt)?
            .ok_or(Error::NotFound)?;
        let saved = self.recover(invocation, attempt)?;
        let accepted = saved.financial.as_ref().ok_or(Error::Invalid)?.accepted();
        crate::receipts::verify_completed(
            receipt,
            &draft,
            &accepted.authorization,
            &accepted.settlement_policy,
        )
        .map_err(|_| Error::Conflict)?;
        let expected = crate::metering::draft_completed_receipt(
            &saved,
            &accepted.authorization.terms,
            &accepted.settlement_policy,
            draft.body.seq,
            draft.body.at_ms,
            draft.previous.as_ref(),
        )
        .map_err(|_| Error::Conflict)?;
        require(expected == receipt.body)?;
        let tx = self.transaction()?;
        let r = current(&tx, invocation)?.ok_or(Error::NotFound)?;
        if r.attempt != attempt
            || r.cancellation_requested
            || !matches!(r.phase, Phase::Resolved | Phase::Closed)
        {
            return Err(Error::Transition);
        }
        let mut table = storage(tx.open_table(OUTCOMES))?;
        let mut slot = read_slot(&table, &r.key())?.ok_or(Error::NotFound)?;
        require(slot.draft.as_ref() == Some(&draft))?;
        if let Some(existing) = slot.receipt {
            return if &existing == receipt {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        require(r.phase == Phase::Resolved)?;
        slot.receipt = Some(receipt.clone());
        write_slot(&mut table, &r.key(), &slot)?;
        drop(table);
        self.commit(tx)
    }
    pub fn completed_receipt(
        &self,
        invocation: &Digest,
        attempt: u64,
    ) -> Result<Option<ProxyUsageReceipt>> {
        let tx = storage(self.database.begin_read())?;
        let records = storage(tx.open_table(RECORDS))?;
        let r = read_record(&records, invocation, attempt)?;
        let table = storage(tx.open_table(OUTCOMES))?;
        let Some(slot) = read_slot(&table, &r.key())? else {
            return Ok(None);
        };
        if let Some(receipt) = &slot.receipt {
            let finance = super::finance::read(&tx, &r)?.ok_or(Error::Invalid)?;
            let accepted = finance.accepted();
            crate::receipts::verify_completed(
                receipt,
                slot.draft.as_ref().ok_or(Error::Invalid)?,
                &accepted.authorization,
                &accepted.settlement_policy,
            )
            .map_err(|_| Error::Invalid)?;
        }
        Ok(slot.receipt)
    }
}
