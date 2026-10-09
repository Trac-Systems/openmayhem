//! One immutable final receipt per owned attempt. Space is reserved before paid
//! dispatch; drafts and signatures share the journal's durability and pruning.
use super::*;
use mayhem_proto::proxy::finance::{
    ProxyClosureBody, ProxyClosureOutcome, ProxyReceiptBody, ProxyReceiptOutcome,
    ProxyReservationClosure, ProxyUsageReceipt,
};
const OUTCOMES: TableDefinition<&str, &[u8]> = TableDefinition::new("proxy_attempt_outcomes_v1");
const ALLOCATION: u64 = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalDraft {
    pub invocation: Digest,
    pub attempt: u64,
    pub body: ProxyReceiptBody,
    pub previous: Option<ProxyReceiptBody>,
    #[serde(default)]
    pub result_commitment: ResultCommitment,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaiverDraft {
    pub invocation: Digest,
    pub attempt: u64,
    pub body: ProxyClosureBody,
    #[serde(default)]
    pub result_commitment: ResultCommitment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub non_execution: Option<NonExecutionEvidence>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Slot {
    draft: Option<TerminalDraft>,
    receipt: Option<ProxyUsageReceipt>,
    #[serde(default)]
    waiver: Option<WaiverDraft>,
    #[serde(default)]
    closure: Option<ProxyReservationClosure>,
}
fn read_slot(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
    key: &str,
) -> Result<Option<Slot>> {
    storage(table.get(key))?
        .map(|v| {
            require(v.value().len() as u64 <= ALLOCATION)?;
            let slot: Slot = serde_json::from_slice(v.value()).map_err(|_| Error::Invalid)?;
            require(
                !(slot.draft.is_some() && slot.waiver.is_some())
                    && (slot.receipt.is_none() || slot.draft.is_some())
                    && (slot.closure.is_none() || slot.waiver.is_some()),
            )?;
            Ok(slot)
        })
        .transpose()
}
pub(super) fn has_intent(tx: &redb::WriteTransaction, key: &str) -> Result<bool> {
    let table = storage(tx.open_table(OUTCOMES))?;
    Ok(read_slot(&table, key)?.is_some_and(|s| s.draft.is_some() || s.waiver.is_some()))
}
fn known_waiver(
    recovery: &Recovery,
    at_ms: u64,
    commitment: ResultCommitment,
) -> Result<WaiverDraft> {
    let r = &recovery.record;
    let accepted = recovery.financial.as_ref().ok_or(Error::Invalid)?;
    accepted
        .validate_for(&r.binding)
        .map_err(|_| Error::Conflict)?;
    let non_execution = r.failure_non_execution();
    if non_execution.is_some() {
        require(
            recovery.result.is_none()
                && recovery.request.is_some()
                && recovery.acceptance.is_some(),
        )?;
    }
    let (outcome, evidence) = if let Some(evidence) = r.non_execution_evidence() {
        (ProxyClosureOutcome::NotExecuted, evidence)
    } else {
        let result = recovery.result.as_ref().ok_or(Error::Transition)?;
        require(recovery.request.is_some() && recovery.acceptance.is_some())?;
        let outcome = if r.cancellation_requested {
            ProxyClosureOutcome::Cancelled
        } else if result
            .reply
            .observed_usage
            .as_ref()
            .is_some_and(|v| v.disposition == crate::metering::Disposition::Incomplete)
        {
            ProxyClosureOutcome::Failed
        } else {
            ProxyClosureOutcome::CompletedUnbilled
        };
        (
            outcome,
            commitment.compute(&r.invocation, r.attempt, &r.binding, &result.reply)?,
        )
    };
    let body = ProxyClosureBody {
        schema_version: 1,
        lane: mayhem_proto::proxy::ProxyLane::Proxy,
        accepted_terms: r.binding.accepted_terms.as_str().into(),
        outcome,
        evidence_hash: evidence.as_str().into(),
        at_ms,
    };
    body.validate().map_err(|_| Error::Invalid)?;
    Ok(WaiverDraft {
        invocation: r.invocation.clone(),
        attempt: r.attempt,
        body,
        result_commitment: commitment,
        non_execution,
    })
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
    /// Explicit mutual waiver, not an automatic fallback when charging fails.
    /// Only this journal's unsent cancellation fence, trusted nonexecution
    /// evidence or validated terminal result can establish the outcome.
    /// A timeout or a failed HTTP connection cannot.
    pub fn prepare_waiver(
        &self,
        invocation: &Digest,
        attempt: u64,
        at_ms: u64,
    ) -> Result<WaiverDraft> {
        if let Some(saved) = self.waiver_draft(invocation, attempt)? {
            return Ok(saved);
        }
        let recovery = self.recover(invocation, attempt)?;
        let r = &recovery.record;
        require(matches!(r.phase, Phase::Dispatched | Phase::Resolved))?;
        let draft = known_waiver(&recovery, at_ms, ResultCommitment::PublicV1)?;
        let tx = self.transaction()?;
        let mut current = current(&tx, invocation)?.ok_or(Error::NotFound)?;
        let mut table = storage(tx.open_table(OUTCOMES))?;
        let mut slot = read_slot(&table, &r.key())?.ok_or(Error::NotFound)?;
        require(slot.draft.is_none() && slot.receipt.is_none())?;
        if let Some(saved) = slot.waiver {
            return Ok(saved);
        }
        if &current != r {
            return Err(Error::Stale);
        }
        if current.phase == Phase::Dispatched {
            let evidence = recovery
                .result
                .as_ref()
                .ok_or(Error::Transition)?
                .digest
                .clone();
            let resolution = match draft.body.outcome {
                ProxyClosureOutcome::NotExecuted => return Err(Error::Transition),
                ProxyClosureOutcome::Cancelled => Resolution::Cancelled {
                    evidence,
                    partial: None,
                },
                ProxyClosureOutcome::Failed => Resolution::Failed {
                    evidence,
                    partial: None,
                },
                ProxyClosureOutcome::CompletedUnbilled => Resolution::Completed {
                    verified: VerifiedResult {
                        result: evidence,
                        usage_evidence: Digest::new(
                            draft.body.digest().map_err(|_| Error::Invalid)?,
                        )?,
                    },
                },
            };
            payloads::verify_resolution(&tx, &r.key(), &resolution)?;
            current.phase = Phase::Resolved;
            current.resolution = Some(resolution);
            bump(&tx, &mut current, at_ms)?;
            save(&tx, &current)?;
        }
        slot.waiver = Some(draft.clone());
        write_slot(&mut table, &r.key(), &slot)?;
        drop(table);
        self.commit(tx)?;
        Ok(draft)
    }
    pub fn waiver_draft(&self, invocation: &Digest, attempt: u64) -> Result<Option<WaiverDraft>> {
        let tx = storage(self.database.begin_read())?;
        let r = read_record(&storage(tx.open_table(RECORDS))?, invocation, attempt)?;
        let table = storage(tx.open_table(OUTCOMES))?;
        let draft = read_slot(&table, &r.key())?.and_then(|s| s.waiver);
        if let Some(d) = &draft {
            require(
                d.invocation == *invocation
                    && d.attempt == attempt
                    && d.body.accepted_terms == r.binding.accepted_terms.as_str(),
            )?;
            d.body.validate().map_err(|_| Error::Invalid)?;
        }
        Ok(draft)
    }
    pub fn retain_waiver(
        &self,
        invocation: &Digest,
        attempt: u64,
        closure: &ProxyReservationClosure,
    ) -> Result<()> {
        let draft = self
            .waiver_draft(invocation, attempt)?
            .ok_or(Error::NotFound)?;
        let recovery = self.recover(invocation, attempt)?;
        require(
            draft == known_waiver(&recovery, draft.body.at_ms, draft.result_commitment)?
                && closure.body == draft.body,
        )?;
        let accepted = recovery
            .financial
            .as_ref()
            .ok_or(Error::Invalid)?
            .accepted();
        closure
            .verify(
                &accepted.authorization.terms,
                crate::receipts::verify_signature,
            )
            .map_err(|_| Error::Conflict)?;
        let tx = self.transaction()?;
        let r = current(&tx, invocation)?.ok_or(Error::NotFound)?;
        require(r.attempt == attempt && matches!(r.phase, Phase::Resolved | Phase::Closed))?;
        let mut table = storage(tx.open_table(OUTCOMES))?;
        let mut slot = read_slot(&table, &r.key())?.ok_or(Error::NotFound)?;
        require(slot.waiver.as_ref() == Some(&draft) && slot.draft.is_none())?;
        if let Some(saved) = slot.closure {
            return if &saved == closure {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        require(r.phase == Phase::Resolved)?;
        slot.closure = Some(closure.clone());
        write_slot(&mut table, &r.key(), &slot)?;
        drop(table);
        self.commit(tx)
    }
    pub fn waiver(
        &self,
        invocation: &Digest,
        attempt: u64,
    ) -> Result<Option<ProxyReservationClosure>> {
        let tx = storage(self.database.begin_read())?;
        let r = read_record(&storage(tx.open_table(RECORDS))?, invocation, attempt)?;
        let table = storage(tx.open_table(OUTCOMES))?;
        let Some(slot) = read_slot(&table, &r.key())? else {
            return Ok(None);
        };
        if let Some(closure) = &slot.closure {
            let finance = super::finance::read(&tx, &r)?.ok_or(Error::Invalid)?;
            require(slot.waiver.as_ref().is_some_and(|d| {
                d.invocation == *invocation && d.attempt == attempt && closure.body == d.body
            }))?;
            closure
                .verify(
                    &finance.accepted().authorization.terms,
                    crate::receipts::verify_signature,
                )
                .map_err(|_| Error::Invalid)?;
        }
        Ok(slot.closure)
    }
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
                waiver: None,
                closure: None,
            },
        )?;
        save_meta(&tx, &meta)?;
        drop(table);
        self.commit(tx)
    }
    pub fn terminal_draft(
        &self,
        invocation: &Digest,
        attempt: u64,
    ) -> Result<Option<TerminalDraft>> {
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
    pub fn prepare_terminal_draft(
        &self,
        invocation: &Digest,
        attempt: u64,
        at_ms: u64,
        previous: Option<ProxyReceiptBody>,
    ) -> Result<TerminalDraft> {
        if let Some(saved) = self.terminal_draft(invocation, attempt)? {
            return Ok(saved);
        }
        let recovery = self.recover(invocation, attempt)?;
        let r = &recovery.record;
        require(matches!(r.phase, Phase::Dispatched | Phase::Resolved))?;
        let accepted = recovery
            .financial
            .as_ref()
            .ok_or(Error::Invalid)?
            .accepted();
        let seq = previous
            .as_ref()
            .map_or(Some(1), |v| v.seq.checked_add(1))
            .ok_or(Error::Invalid)?;
        let body = crate::metering::draft_terminal_receipt(
            &recovery,
            &accepted.authorization.terms,
            &accepted.settlement_policy,
            seq,
            at_ms,
            previous.as_ref(),
        )
        .map_err(|_| Error::Conflict)?;
        let draft = TerminalDraft {
            invocation: invocation.clone(),
            attempt,
            body,
            previous,
            result_commitment: ResultCommitment::PublicV1,
        };
        let tx = self.transaction()?;
        let mut current = current(&tx, invocation)?.ok_or(Error::NotFound)?;
        let mut table = storage(tx.open_table(OUTCOMES))?;
        let mut slot = read_slot(&table, &r.key())?.ok_or(Error::NotFound)?;
        require(slot.waiver.is_none())?;
        if let Some(saved) = slot.draft {
            return Ok(saved);
        }
        if &current != r {
            return Err(Error::Stale);
        }
        let verified = VerifiedResult {
            result: recovery
                .result
                .as_ref()
                .ok_or(Error::Transition)?
                .digest
                .clone(),
            usage_evidence: Digest::new(&draft.body.observation_hash)?,
        };
        let resolution = match draft.body.outcome {
            ProxyReceiptOutcome::Cancelled => Resolution::Cancelled {
                evidence: verified.result.clone(),
                partial: Some(verified),
            },
            ProxyReceiptOutcome::Partial => Resolution::Failed {
                evidence: verified.result.clone(),
                partial: Some(verified),
            },
            ProxyReceiptOutcome::Complete | ProxyReceiptOutcome::Refused => {
                Resolution::Completed { verified }
            }
            ProxyReceiptOutcome::Running => return Err(Error::Invalid),
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
    pub fn retain_terminal_receipt(
        &self,
        invocation: &Digest,
        attempt: u64,
        receipt: &ProxyUsageReceipt,
    ) -> Result<()> {
        let draft = self
            .terminal_draft(invocation, attempt)?
            .ok_or(Error::NotFound)?;
        let saved = self.recover(invocation, attempt)?;
        let accepted = saved.financial.as_ref().ok_or(Error::Invalid)?.accepted();
        crate::receipts::verify_terminal(
            receipt,
            &draft,
            &accepted.authorization,
            &accepted.settlement_policy,
        )
        .map_err(|_| Error::Conflict)?;
        let expected = crate::metering::draft_terminal_receipt_for(
            &saved,
            &accepted.authorization.terms,
            &accepted.settlement_policy,
            draft.body.seq,
            draft.body.at_ms,
            draft.previous.as_ref(),
            draft.result_commitment,
        )
        .map_err(|_| Error::Conflict)?;
        require(expected == receipt.body)?;
        let tx = self.transaction()?;
        let r = current(&tx, invocation)?.ok_or(Error::NotFound)?;
        if r.attempt != attempt || !matches!(r.phase, Phase::Resolved | Phase::Closed) {
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
    pub fn terminal_receipt(
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
            crate::receipts::verify_terminal(
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
