//! Paid admission interface: canonical finance, owned acceptance and shared capacity
//! are mandatory. The caller already obtained buyer/provider signed acceptance through
//! canonical publication; this component does not invent signatures or create funds.
//! Public authentication, publication and receipt settlement remain separate concerns.
use super::*;
use crate::attempts::AcceptanceSnapshot;
use mayhem_proto::proxy::finance::ProxySpendAuthorization;

pub struct PaidExecutor {
    executor: Executor,
    financial: Arc<financial::Client>,
    authority: Arc<capacity::Authority>,
    route: Digest,
}
impl PaidExecutor {
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
}
