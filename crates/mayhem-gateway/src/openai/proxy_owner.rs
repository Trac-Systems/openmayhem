//! Durable gateway ownership for the proxy buyer controller. No method in this
//! module dispatches inference or releases an uncertain purchase on drop.
use super::{now_secs, GatewayAccessControl, GatewayTokenAttribution};
use crate::job_store::{
    proxy::{ProxyNonAdmission, ProxyVerifiedOutput},
    GatewayJobStore, StoredGatewayJob,
};
use mayhem_proxy::{
    buyer_controller::{
        AuthorizationGate, GateError, NonAdmission, RequestIdentity, VerifiedOutput,
    },
    financial::{
        negotiation::SavedPurchase, quote::PreparedPurchase, recovery::FinancialOutcome,
        Observation,
    },
};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::sync::Semaphore;

// Includes queued and running blocking storage work. Dropping an HTTP caller or
// this await never returns a permit while its durable write is still in flight.
static STORAGE: Semaphore = Semaphore::const_new(8);

#[derive(Clone)]
pub(crate) struct Binding {
    pub(crate) job_id: String,
    pub(crate) token: GatewayTokenAttribution,
    pub(crate) model: String,
    pub(crate) fingerprint: String,
    pub(crate) identity: RequestIdentity,
}

/// Only the authenticated gateway owner constructs this adapter. The controller
/// receives it as a gate, not as signing or budget-management authority.
pub(crate) struct Owner {
    jobs: Arc<Mutex<GatewayJobStore>>,
    access: Arc<GatewayAccessControl>,
    binding: Binding,
}

impl Owner {
    pub(crate) fn new(
        jobs: Arc<Mutex<GatewayJobStore>>,
        access: Arc<GatewayAccessControl>,
        binding: Binding,
    ) -> Self {
        Self {
            jobs,
            access,
            binding,
        }
    }

    fn check(job: &StoredGatewayJob, binding: &Binding) -> Result<(), GateError> {
        if job.id != binding.job_id
            || job.owner_token_id.as_deref() != Some(binding.token.token_id.as_str())
            || job.model != binding.model
            || job.request_fingerprint != binding.fingerprint
            || job.proxy.as_ref().map(|p| p.identity()) != Some(&binding.identity)
        {
            return Err(GateError::Rejected);
        }
        Ok(())
    }

    async fn storage<T: Send + 'static>(
        operation: impl FnOnce() -> Result<T, GateError> + Send + 'static,
    ) -> Result<T, GateError> {
        let permit = STORAGE.try_acquire().map_err(|_| GateError::Unavailable)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            operation()
        })
        .await
        .map_err(|_| GateError::Unavailable)?
    }

    /// Persist observation of the original buyer journal. Missing/unreadable
    /// records deliberately have no opposite "release" operation here.
    pub(crate) async fn note_purchase(&self, purchase: SavedPurchase) -> Result<(), GateError> {
        let jobs = self.jobs.clone();
        let binding = self.binding.clone();
        Self::storage(move || {
            let mut jobs = jobs.lock().map_err(|_| GateError::Unavailable)?;
            let existing = jobs
                .get(&binding.job_id, now_secs())
                .map_err(|_| GateError::Unavailable)?
                .ok_or(GateError::Rejected)?;
            Self::check(&existing, &binding)?;
            jobs.note_proxy_purchase(&binding.job_id, &purchase, now_secs())
                .map_err(|_| GateError::Unavailable)?;
            Ok(())
        })
        .await
    }

    /// A fresh canonical observation is required before accounting. Persist the
    /// outcome first, then settle its exact commitment in the common budget
    /// journal, then finish the job. Repeating after a crash is idempotent at
    /// each boundary; local cancellation cannot manufacture financial closure.
    pub(crate) async fn close(
        &self,
        observation: Observation,
    ) -> Result<StoredGatewayJob, GateError> {
        let jobs = self.jobs.clone();
        let access = self.access.clone();
        let binding = self.binding.clone();
        Self::storage(move || {
            let closed = {
                let mut jobs = jobs.lock().map_err(|_| GateError::Unavailable)?;
                let existing = jobs
                    .get(&binding.job_id, now_secs())
                    .map_err(|_| GateError::Unavailable)?
                    .ok_or(GateError::Rejected)?;
                Self::check(&existing, &binding)?;
                jobs.close_proxy(&binding.job_id, &observation, now_secs())
                    .map_err(|_| GateError::Unavailable)?
            };
            let proxy = closed.proxy.as_ref().ok_or(GateError::Rejected)?;
            let terms = proxy.terms().ok_or(GateError::Rejected)?;
            let terms_digest = terms.digest().map_err(|_| GateError::Rejected)?;
            let closure = proxy.closure().ok_or(GateError::Rejected)?;
            let marker = closure.commitment().map_err(|_| GateError::Rejected)?;
            let spent = match &closure.outcome {
                FinancialOutcome::Paid { receipt } => receipt.body.au_owed_cum,
                // Observation verifies full reservation release and unchanged
                // prior billing spend for expiry; a timeout alone cannot enter
                // this branch or establish a zero-cost outcome.
                FinancialOutcome::Waived { .. } | FinancialOutcome::ExpiredUnknown { .. } => 0,
            };
            access
                .settle_proxy_budget(
                    &binding.token,
                    &binding.job_id,
                    &binding.fingerprint,
                    &terms_digest,
                    spent,
                    true,
                    marker.as_str(),
                )
                .map_err(|_| GateError::Unavailable)?;
            jobs.lock()
                .map_err(|_| GateError::Unavailable)?
                .finish_proxy(&binding.job_id, marker, now_secs())
                .map_err(|_| GateError::Unavailable)
        })
        .await
    }
}

impl AuthorizationGate for Owner {
    fn retain_non_admission<'a>(
        &'a self,
        proof: &'a NonAdmission,
    ) -> Pin<Box<dyn Future<Output = Result<(), GateError>> + Send + 'a>> {
        let fence = ProxyNonAdmission::capture(proof);
        let jobs = self.jobs.clone();
        let access = self.access.clone();
        let binding = self.binding.clone();
        Box::pin(async move {
            let fence = fence.map_err(|_| GateError::Rejected)?;
            Self::storage(move || {
                {
                    let mut jobs = jobs.lock().map_err(|_| GateError::Unavailable)?;
                    let existing = jobs
                        .get(&binding.job_id, now_secs())
                        .map_err(|_| GateError::Unavailable)?
                        .ok_or(GateError::Rejected)?;
                    Self::check(&existing, &binding)?;
                    jobs.retain_proxy_non_admission(&binding.job_id, fence.clone(), now_secs())
                        .map_err(|_| GateError::Unavailable)?;
                }
                access
                    .fence_proxy_budget(
                        &binding.token,
                        &binding.job_id,
                        &binding.fingerprint,
                        fence.terms.as_str(),
                        fence.maximum,
                        fence.marker.as_str(),
                    )
                    .map_err(|_| GateError::Unavailable)?;
                jobs.lock()
                    .map_err(|_| GateError::Unavailable)?
                    .finish_proxy_non_admission(&binding.job_id, &fence.marker, now_secs())
                    .map_err(|_| GateError::Unavailable)?;
                Ok(())
            })
            .await
        })
    }

    fn authorize<'a>(
        &'a self,
        purchase: &'a PreparedPurchase,
    ) -> Pin<Box<dyn Future<Output = Result<(), GateError>> + Send + 'a>> {
        let terms = purchase.terms().clone();
        let policy = purchase.policy().clone();
        let jobs = self.jobs.clone();
        let access = self.access.clone();
        let binding = self.binding.clone();
        Box::pin(async move {
            Self::storage(move || {
                // Retain the owner intent before budget mutation. If a later
                // boundary fails, it remains pending; no signature is permitted
                // until both this exact intent and its reservation are durable.
                {
                    let mut jobs = jobs.lock().map_err(|_| GateError::Unavailable)?;
                    let existing = jobs
                        .get(&binding.job_id, now_secs())
                        .map_err(|_| GateError::Unavailable)?
                        .ok_or(GateError::Rejected)?;
                    Self::check(&existing, &binding)?;
                    jobs.authorize_proxy_terms(&binding.job_id, terms.clone(), policy, now_secs())
                        .map_err(|_| GateError::Unavailable)?;
                }
                access
                    .reserve_proxy_budget(
                        &binding.token,
                        &binding.job_id,
                        &binding.fingerprint,
                        &terms,
                        &binding.model,
                    )
                    .map_err(|_| GateError::Rejected)
            })
            .await
        })
    }

    fn retain_verified_output<'a>(
        &'a self,
        output: VerifiedOutput<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<(), GateError>> + Send + 'a>> {
        let output = ProxyVerifiedOutput::capture(&output);
        let jobs = self.jobs.clone();
        let binding = self.binding.clone();
        Box::pin(async move {
            Self::storage(move || {
                let mut jobs = jobs.lock().map_err(|_| GateError::Unavailable)?;
                let existing = jobs
                    .get(&binding.job_id, now_secs())
                    .map_err(|_| GateError::Unavailable)?
                    .ok_or(GateError::Rejected)?;
                Self::check(&existing, &binding)?;
                jobs.retain_proxy_output(&binding.job_id, output, now_secs())
                    .map_err(|_| GateError::Unavailable)?;
                Ok(())
            })
            .await
        })
    }
}

#[cfg(all(test, unix))]
pub(crate) mod tests;
