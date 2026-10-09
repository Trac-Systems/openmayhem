//! Paid admission interface: canonical finance, owned acceptance and shared capacity
//! are mandatory. The caller already obtained buyer/provider signed acceptance through
//! canonical publication; this component does not invent signatures or create funds.
//! Public authentication, protected signing and startup integration remain separate.
use super::*;
use crate::attempts::AcceptanceSnapshot;
use mayhem_proto::proxy::finance::{
    ProxyReservationClosure, ProxySpendAuthorization, ProxyUsageReceipt,
};

enum Outcome {
    Receipt(ProxyUsageReceipt),
    Waiver(ProxyReservationClosure),
}
impl Outcome {
    fn digest(&self) -> Result<Digest> {
        Digest::new(
            match self {
                Self::Receipt(v) => v.body.digest(),
                Self::Waiver(v) => v.body.digest(),
            }
            .map_err(|_| Error::Binding)?,
        )
        .map_err(Error::Journal)
    }
    fn confirmed(&self, observation: &financial::Observation) -> Result<bool> {
        match self {
            Self::Receipt(v) => observation.confirms_receipt(v),
            Self::Waiver(v) => observation.confirms_waiver(v),
        }
        .map_err(Error::Financial)
    }
    fn retained(&self, journal: &Journal, invocation: &Digest, attempt: u64) -> Result<bool> {
        Ok(match self {
            Self::Receipt(v) => journal.terminal_receipt(invocation, attempt)?.as_ref() == Some(v),
            Self::Waiver(v) => journal.waiver(invocation, attempt)?.as_ref() == Some(v),
        })
    }
}

pub struct PaidExecutor {
    executor: Executor,
    financial: Arc<financial::Client>,
    authority: Arc<capacity::Authority>,
    route: Digest,
}
impl PaidExecutor {
    fn check_signer(&self, signer: &crate::signing::Authority) -> Result<()> {
        let actor = signer.identity();
        let network = self.financial.identity();
        if actor.network_id != network.network_id
            || actor.msb_bootstrap.as_str() != network.msb_bootstrap
            || actor.subnet_bootstrap.as_str() != network.subnet_bootstrap
            || actor.controller_pubkey.as_str() != self.financial.requester()
        {
            return Err(Error::Binding);
        }
        Ok(())
    }
    /// Signs only the exact durable draft derived from this provider's owned
    /// request/result and original canonical terms. A connector cannot supply it.
    pub async fn sign_terminal_receipt(
        &self,
        signer: &crate::signing::Authority,
        invocation: &Digest,
        attempt: u64,
    ) -> Result<crate::signing::ProviderReceipt> {
        self.check_signer(signer)?;
        let draft = self.prepare_terminal_receipt(invocation, attempt).await?;
        let saved = self.recover(invocation, attempt).await?;
        let accepted = saved.financial.as_ref().ok_or(Error::Binding)?.accepted();
        signer
            .provider_receipt(draft, accepted)
            .map_err(Error::Financial)
    }
    /// Requesting this is explicit provider consent to a known-outcome waiver.
    /// This still requires independent buyer approval before canonical closure.
    pub async fn sign_waiver(
        &self,
        signer: &crate::signing::Authority,
        invocation: &Digest,
        attempt: u64,
    ) -> Result<crate::signing::ProviderWaiver> {
        self.check_signer(signer)?;
        let draft = self.prepare_waiver(invocation, attempt).await?;
        let saved = self.recover(invocation, attempt).await?;
        let accepted = saved.financial.as_ref().ok_or(Error::Binding)?.accepted();
        signer
            .provider_waiver(draft, accepted)
            .map_err(Error::Financial)
    }
    /// Freeze the final body before requesting either signature. Repeated calls
    /// recover its original timestamp/sequence, even after an interrupted signer.
    pub async fn prepare_terminal_receipt(
        &self,
        invocation: &Digest,
        attempt: u64,
    ) -> Result<attempts::TerminalDraft> {
        let key = invocation.clone();
        if let Some(draft) = self
            .executor
            .storage
            .run(move |j| j.terminal_draft(&key, attempt))
            .await?
        {
            return Ok(draft);
        }
        let saved = self.recover(invocation, attempt).await?;
        let accepted = saved.financial.as_ref().ok_or(Error::Binding)?.accepted();
        let observed = self
            .financial
            .observe(&accepted.authorization)
            .await
            .map_err(Error::Financial)?;
        if observed.is_closed() {
            return Err(Error::RecoveryRequired);
        }
        let previous = observed.receipt_head().map_err(Error::Financial)?;
        if previous.as_ref().is_some_and(|r| r.body.final_receipt) {
            return Err(Error::RecoveryRequired);
        }
        let key = invocation.clone();
        self.executor
            .storage
            .run_checked(move |j| {
                // Check freshness again after scheduling bounded storage work.
                observed.receipt_head().map_err(Error::Financial)?;
                j.reserve_outcome(&key, attempt)?;
                Ok(j.prepare_terminal_draft(&key, attempt, now_ms(), previous.map(|v| v.body))?)
            })
            .await
    }
    pub async fn retain_terminal_receipt(
        &self,
        invocation: &Digest,
        attempt: u64,
        receipt: &mayhem_proto::proxy::finance::ProxyUsageReceipt,
    ) -> Result<()> {
        let key = invocation.clone();
        let receipt = receipt.clone();
        self.executor
            .storage
            .run(move |j| j.retain_terminal_receipt(&key, attempt, &receipt))
            .await
    }
    /// Observe -> submit the identical retained envelope if needed -> observe.
    /// No inference retry, timer-based financial release or receipt history scan.
    /// False means the exact receipt is still awaiting canonical confirmation.
    pub async fn publish_terminal_receipt(
        &self,
        invocation: &Digest,
        attempt: u64,
    ) -> Result<bool> {
        let key = invocation.clone();
        let receipt = self
            .executor
            .storage
            .run(move |j| j.terminal_receipt(&key, attempt))
            .await?
            .ok_or(Error::RecoveryRequired)?;
        self.publish_outcome(invocation, attempt, Outcome::Receipt(receipt))
            .await
    }
    pub async fn prepare_waiver(
        &self,
        invocation: &Digest,
        attempt: u64,
    ) -> Result<attempts::WaiverDraft> {
        let key = invocation.clone();
        self.executor
            .storage
            .run(move |j| {
                j.reserve_outcome(&key, attempt)?;
                j.prepare_waiver(&key, attempt, now_ms())
            })
            .await
    }
    pub async fn retain_waiver(
        &self,
        invocation: &Digest,
        attempt: u64,
        closure: &ProxyReservationClosure,
    ) -> Result<()> {
        let key = invocation.clone();
        let closure = closure.clone();
        self.executor
            .storage
            .run(move |j| j.retain_waiver(&key, attempt, &closure))
            .await
    }
    pub async fn publish_waiver(&self, invocation: &Digest, attempt: u64) -> Result<bool> {
        let key = invocation.clone();
        let closure = self
            .executor
            .storage
            .run(move |j| j.waiver(&key, attempt))
            .await?
            .ok_or(Error::RecoveryRequired)?;
        self.publish_outcome(invocation, attempt, Outcome::Waiver(closure))
            .await
    }
    async fn publish_outcome(
        &self,
        invocation: &Digest,
        attempt: u64,
        outcome: Outcome,
    ) -> Result<bool> {
        let saved = self.recover(invocation, attempt).await?;
        let digest = outcome.digest()?;
        if saved.record.phase == Phase::Closed {
            return if saved.record.closure == Some(digest) {
                Ok(true)
            } else {
                Err(Error::Binding)
            };
        }
        let accepted = saved.financial.as_ref().ok_or(Error::Binding)?.accepted();
        let mut observation = self
            .financial
            .observe(&accepted.authorization)
            .await
            .map_err(Error::Financial)?;
        if !outcome.confirmed(&observation)? {
            if observation.is_closed() && matches!(&outcome, Outcome::Receipt(_)) {
                return Err(Error::RecoveryRequired);
            }
            let submission = match &outcome {
                Outcome::Receipt(receipt) => {
                    self.financial
                        .submit_receipt(
                            &accepted.authorization,
                            &accepted.settlement_policy,
                            receipt,
                        )
                        .await
                }
                Outcome::Waiver(closure) => {
                    self.financial
                        .submit_waiver(&accepted.authorization, closure)
                        .await
                }
            };
            observation = self
                .financial
                .observe(&accepted.authorization)
                .await
                .map_err(Error::Financial)?;
            if !outcome.confirmed(&observation)? {
                submission.map_err(Error::Financial)?;
                return Ok(false);
            }
        }
        self.reconcile_capacity(invocation, attempt).await?;
        let key = invocation.clone();
        let authority = self.authority.clone();
        self.executor
            .storage
            .run_checked(move |j| {
                if !outcome.confirmed(&observation)? {
                    return Err(Error::RecoveryRequired);
                }
                let r = j.get(&key)?.ok_or(attempts::Error::NotFound)?;
                if r.attempt != attempt
                    || !outcome.retained(j, &key, attempt)?
                    || authority.lease(&r.binding.capacity_lease)?.is_some()
                {
                    return Err(Error::Binding);
                }
                if r.phase == Phase::Closed {
                    return if r.closure == Some(digest) {
                        Ok(true)
                    } else {
                        Err(Error::Binding)
                    };
                }
                if let Err(error) =
                    j.advance(&key, r.generation, Event::Close(digest.clone()), now_ms())
                {
                    // Another reconciliation can win after the read above. Accept
                    // only its exact durable closure, not an arbitrary stale update.
                    let same = matches!(error, attempts::Error::Stale)
                        && j.get(&key)?.is_some_and(|latest| {
                            latest.attempt == attempt
                                && latest.phase == Phase::Closed
                                && latest.closure == Some(digest)
                        });
                    if !same {
                        return Err(error.into());
                    }
                }
                Ok(true)
            })
            .await
    }
    pub fn new(
        mut executor: Executor,
        financial: Arc<financial::Client>,
        authority: Arc<capacity::Authority>,
        route: Digest,
    ) -> Result<Self> {
        let i = authority.identity();
        let f = financial.identity();
        if &executor.storage.journal.identity()? != i {
            return Err(Error::Binding);
        }
        if i.network_id != f.network_id
            || i.msb_bootstrap.as_str() != f.msb_bootstrap
            || i.subnet_bootstrap.as_str() != f.subnet_bootstrap
            || i.controller_pubkey.as_str() != financial.requester()
        {
            return Err(Error::Binding);
        }
        executor.capacity = Some((authority.clone(), route.clone()));
        executor.financial = Some(financial.clone());
        Ok(Self {
            executor,
            financial,
            authority,
            route,
        })
    }
    /// Prepare only this canonical accepted authorization. The invocation must be
    /// scoped to the authenticated buyer. Capacity was reserved before signing so
    /// its random ID is committed in the accepted terms. No upstream request here.
    /// Repeated preparation preserves original immutable terms; changed authorization
    /// for the same invocation is a conflict, not permission to replace history.
    pub async fn prepare_accepted(
        &self,
        invocation: Digest,
        authorization: &ProxySpendAuthorization,
        bytes: &[u8],
        streaming: bool,
    ) -> Result<Record> {
        let request = if streaming {
            self.executor.adapter.prepare_stream(bytes)?
        } else {
            self.executor.adapter.prepare_json(bytes)?
        };
        let t = &authorization.terms;
        if t.offer.provider_pubkey != self.authority.identity().controller_pubkey.as_str()
            || t.request_hash != request.request_hash().as_str()
            || t.connection_digest != self.executor.connection.fingerprint().as_str()
            || t.connection_revision != self.executor.connection.revision()
            || t.recipe_hash != self.executor.adapter.recipe_hash().as_str()
            || t.endpoint_contract != self.executor.adapter.contract_hash().as_str()
        {
            return Err(Error::Binding);
        }
        let observation = self
            .financial
            .observe(authorization)
            .await
            .map_err(Error::Financial)?;
        let binding = observation.initial_binding().map_err(Error::Financial)?;
        if !request.matches_binding(&binding) {
            return Err(Error::Binding);
        }
        let snapshot = AcceptanceSnapshot {
            adapter: self.executor.adapter.snapshot(),
            offer: t.offer.clone(),
        };
        snapshot.validate_for(&binding)?;
        let authority = self.authority.clone();
        let route = self.route.clone();
        let body = bytes.to_vec();
        let result_allowance = self.executor.adapter.limits().response_bytes;
        self.executor
            .storage
            .run_checked(move |journal| {
                if observation.initial_binding().map_err(Error::Financial)? != binding {
                    return Err(Error::Binding);
                }
                let lease = authority
                    .lease(&binding.capacity_lease)?
                    .ok_or(Error::Binding)?;
                if lease.route != route
                    || lease.phase != capacity::Phase::Reserved
                    || lease.work.invocation != invocation
                    || lease.work.request_hash != binding.request_hash
                {
                    return Err(Error::Binding);
                }
                // Journal retention also checks provider/network identity, and remains
                // immutable across a partially completed preparation/restart.
                let record = journal.prepare(invocation, binding.clone(), now_ms())?;
                if record.binding != binding {
                    return Err(Error::Binding);
                }
                if record.phase != Phase::Prepared {
                    return Err(Error::RecoveryRequired);
                }
                journal.retain_acceptance(&record.invocation, record.attempt, &snapshot)?;
                journal.retain_financial_acceptance(
                    &record.invocation,
                    record.attempt,
                    &observation,
                )?;
                journal.retain_request(
                    &record.invocation,
                    record.attempt,
                    &body,
                    result_allowance,
                )?;
                journal.reserve_outcome(&record.invocation, record.attempt)?;
                Ok(record)
            })
            .await
    }
    pub async fn execute_json(
        &self,
        invocation: &Digest,
        bytes: &[u8],
        cancel: &Cancellation,
    ) -> Result<UnsettledReply> {
        self.executor.execute_json(invocation, bytes, cancel).await
    }
    pub async fn execute_stream<F, Fut>(
        &self,
        invocation: &Digest,
        bytes: &[u8],
        cancel: &Cancellation,
        emit: F,
    ) -> Result<UnsettledReply>
    where
        F: FnMut(serde_json::Value) -> Fut,
        Fut: Future<Output = std::result::Result<(), ()>>,
    {
        self.executor
            .execute_stream(invocation, bytes, cancel, emit)
            .await
    }
    /// Explicit cancellation BEFORE the journal dispatch fence. Commit the
    /// cancellation first so a competing sender cannot run after capacity is freed.
    /// A dispatched/unknown attempt must instead use upstream recovery evidence.
    pub async fn cancel_unsent(&self, invocation: &Digest, attempt: u64) -> Result<()> {
        let key = invocation.clone();
        self.executor
            .storage
            .run_checked(move |journal| {
                let record = journal.get(&key)?.ok_or(attempts::Error::NotFound)?;
                if record.attempt != attempt {
                    return Err(Error::Binding);
                }
                if record.unsent_cancellation_evidence().is_some() {
                    return Ok(());
                }
                if record.phase != Phase::Prepared {
                    return Err(Error::RecoveryRequired);
                }
                journal.advance(&key, record.generation, Event::CancelRequested, now_ms())?;
                Ok(())
            })
            .await?;
        self.reconcile_capacity(invocation, attempt).await?;
        Ok(())
    }
    /// Release only from retained, validated terminal output or the journal's exact
    /// pre-send cancellation fence. Capacity and financial closure are independent:
    /// this NEVER releases customer funds, publishes a receipt or retries inference.
    pub async fn reconcile_capacity(&self, invocation: &Digest, attempt: u64) -> Result<bool> {
        let key = invocation.clone();
        let authority = self.authority.clone();
        let route = self.route.clone();
        self.executor
            .storage
            .run_checked(move |journal| {
                let saved = journal.recover(&key, attempt)?;
                let r = &saved.record;
                let unsent = r.unsent_cancellation_evidence();
                let evidence = if let Some(evidence) = &unsent {
                    evidence.clone()
                } else if let Some(result) = &saved.result {
                    if saved.acceptance.is_none()
                        || saved.financial.is_none()
                        || saved.request.is_none()
                    {
                        return Err(Error::Binding);
                    }
                    result.digest.clone()
                } else {
                    return Err(Error::RecoveryRequired);
                };
                let Some(lease) = authority.lease(&r.binding.capacity_lease)? else {
                    return Ok(false);
                };
                if lease.route != route
                    || lease.work.invocation != r.invocation
                    || lease.work.request_hash != r.binding.request_hash
                    || (unsent.is_none() && lease.phase == capacity::Phase::Reserved)
                {
                    return Err(Error::Binding);
                }
                Ok(authority.complete(capacity::VerifiedCompletion { lease, evidence })?)
            })
            .await
    }

    /// Exact owned recovery, no new POST or implicit release. Fresh financial and
    /// capacity reconciliation is separate from returning previously retained data.
    pub async fn recover(&self, invocation: &Digest, attempt: u64) -> Result<attempts::Recovery> {
        self.executor.storage.recover(invocation, attempt).await
    }
    pub(crate) async fn recover_current(
        &self,
        invocation: &Digest,
    ) -> Result<Option<attempts::Recovery>> {
        let key = invocation.clone();
        self.executor
            .storage
            .run(move |journal| {
                journal
                    .get(&key)?
                    .map(|r| journal.recover(&key, r.attempt))
                    .transpose()
            })
            .await
    }
    pub(crate) async fn request_cancel(
        &self,
        invocation: &Digest,
        cancel: &Cancellation,
    ) -> Result<()> {
        let key = invocation.clone();
        let record = self
            .executor
            .storage
            .run(move |journal| {
                let r = journal.get(&key)?.ok_or(attempts::Error::NotFound)?;
                if r.phase == Phase::Closed || r.cancellation_requested {
                    return Ok(r);
                }
                journal.advance(&key, r.generation, Event::CancelRequested, now_ms())
            })
            .await?;
        cancel.cancel();
        if record.unsent_cancellation_evidence().is_some() {
            self.reconcile_capacity(invocation, record.attempt).await?;
        }
        Ok(())
    }
}
