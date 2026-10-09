//! Provider's pre-signing checks against owned runtime, request and capacity.
//! No constructor accepts an upstream-generated approval or arbitrary sign bytes.
use super::*;
use crate::{
    attempts::{AcceptanceSnapshot, Identity as ProviderIdentity},
    capacity,
    connector::http::HttpConnection,
    endpoint::Adapter,
};
use std::sync::Arc;

/// Construct from trusted operator configuration and the actual running objects,
/// never from a buyer request. Global policy enablement is not operator consent.
pub struct Runtime {
    pub adapter: Arc<Adapter>,
    pub connection: Arc<HttpConnection>,
    pub capacity: Arc<capacity::Authority>,
    pub route: Digest,
    pub approved_policy: ProxySettlementPolicy,
}

/// Private, non-deserializable and non-debuggable. Immutable owned request;
/// canonical freshness and current capacity are rechecked before saving a sig.
pub struct Approval {
    runtime: Arc<Runtime>,
    observation: offer::Observation,
    lease: capacity::Lease,
    buyer: negotiation::BuyerOffer,
    request: Vec<u8>,
    snapshot: AcceptanceSnapshot,
    invocation: Digest,
    binding: Binding,
}
impl Runtime {
    pub fn approve(
        self: &Arc<Self>,
        buyer: negotiation::BuyerOffer,
        observation: offer::Observation,
        reservation: &capacity::Reservation,
        request: Vec<u8>,
    ) -> Result<Approval> {
        buyer.verify()?;
        observation.check_terms(&buyer.terms, &self.approved_policy)?;
        require(
            request.len() <= self.adapter.limits().request_bytes,
            "provider request exceeds bound",
        )?;
        let value: Value = serde_json::from_slice(&request)?;
        let prepared = if value.get("stream") == Some(&Value::Bool(true)) {
            self.adapter.prepare_stream(&request)
        } else {
            self.adapter.prepare_json(&request)
        }
        .map_err(|_| invalid("invalid provider request"))?;
        let binding = terms_binding(&buyer.terms)?;
        let snapshot = AcceptanceSnapshot {
            adapter: self.adapter.snapshot(),
            offer: buyer.terms.offer.clone(),
        };
        snapshot
            .validate_for(&binding)
            .map_err(|_| invalid("provider runtime snapshot differs"))?;
        require(
            prepared.matches_binding(&binding)
                && prepared
                    .maximum_usage(buyer.terms.max_usage.get("output_token").copied())
                    .map_err(|_| invalid("invalid provider usage allowance"))?
                    == buyer.terms.max_usage,
            "provider request or metering allowance differs",
        )?;
        let invocation = crate::exchange::invocation_for_terms(&buyer.terms)
            .map_err(|_| invalid("invalid provider invocation"))?;
        let approval = Approval {
            runtime: self.clone(),
            observation,
            lease: reservation.lease().clone(),
            buyer,
            request,
            snapshot,
            invocation,
            binding,
        };
        approval.recheck()?;
        Ok(approval)
    }
}
impl Approval {
    fn begin_signing(&mut self) -> Result<()> {
        self.recheck()?;
        self.lease = self
            .runtime
            .capacity
            .begin_signing(&self.lease)
            .map_err(|_| invalid("provider signing capacity fence failed"))?;
        self.signing_fenced()
    }
    pub(crate) fn signing_fenced(&self) -> Result<()> {
        require(
            self.lease.phase == capacity::Phase::Reserved,
            "provider signing capacity fence is required",
        )?;
        self.recheck()
    }
    pub fn invocation(&self) -> &Digest {
        &self.invocation
    }
    pub fn terms(&self) -> &mayhem_proto::proxy::finance::ProxySpendTerms {
        &self.buyer.terms
    }
    pub(crate) fn buyer(&self) -> &negotiation::BuyerOffer {
        &self.buyer
    }
    pub(crate) fn policy(&self) -> &ProxySettlementPolicy {
        &self.runtime.approved_policy
    }
    pub(crate) fn request(&self) -> &[u8] {
        &self.request
    }
    pub(crate) fn snapshot(&self) -> &AcceptanceSnapshot {
        &self.snapshot
    }
    pub(crate) fn binding(&self) -> &Binding {
        &self.binding
    }
    pub(crate) fn identity(&self) -> &ProviderIdentity {
        self.runtime.capacity.identity()
    }
    pub(crate) fn response_bytes(&self) -> usize {
        self.runtime.adapter.limits().response_bytes
    }
    pub(crate) fn recheck(&self) -> Result<()> {
        let t = &self.buyer.terms;
        let runtime = &self.runtime;
        self.observation.check_terms(t, &runtime.approved_policy)?;
        let id = self.identity();
        require(
            id.network_id == t.network_id
                && id.msb_bootstrap.as_str() == t.msb_bootstrap
                && id.subnet_bootstrap.as_str() == t.subnet_bootstrap
                && id.controller_pubkey.as_str() == t.offer.provider_pubkey
                && runtime.connection.fingerprint().as_str() == t.connection_digest
                && runtime.connection.revision() == t.connection_revision,
            "provider identity or live connection differs",
        )?;
        let route = runtime
            .capacity
            .check_reserved(&self.lease)
            .map_err(|_| invalid("provider capacity reservation is not ready"))?;
        require(
            route.id == runtime.route
                && route.lane == capacity::Lane::Proxy
                && route.group.as_str() == self.observation.membership()?.capacity_group
                && self.lease.id.as_str() == t.capacity_lease
                && self.lease.work.invocation == self.invocation
                && self.lease.work.request_hash.as_str() == t.request_hash,
            "provider reservation differs from request, route or shared group",
        )
    }
}

/// One bounded storage executor for this provider's negotiation operations.
/// Caller cancellation cannot drop the permit before the actual commit finishes.
pub struct ProviderNegotiation {
    journal: Arc<crate::attempts::Journal>,
    signer: Arc<crate::signing::Authority>,
    slots: Arc<tokio::sync::Semaphore>,
}
impl ProviderNegotiation {
    pub(crate) async fn has_intent(&self, invocation: Digest) -> Result<bool> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| invalid("provider negotiation storage busy"))?;
        let journal = self.journal.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            journal
                .get(&invocation)
                .map(|v| v.is_some())
                .map_err(|_| invalid("provider journal unavailable"))
        })
        .await
        .map_err(|_| Error::Task)?
    }
    pub(crate) fn identity(&self) -> Result<ProviderIdentity> {
        self.journal
            .identity()
            .map_err(|_| invalid("provider journal unavailable"))
    }
    pub fn new(
        journal: Arc<crate::attempts::Journal>,
        signer: Arc<crate::signing::Authority>,
        max_storage: usize,
    ) -> Result<Self> {
        require(
            (1..=64).contains(&max_storage)
                && journal
                    .identity()
                    .map_err(|_| invalid("provider journal unavailable"))?
                    == *signer.identity(),
            "invalid provider negotiation configuration",
        )?;
        Ok(Self {
            journal,
            signer,
            slots: Arc::new(tokio::sync::Semaphore::new(max_storage)),
        })
    }
    pub async fn accept(
        &self,
        mut approval: Approval,
        now_ms: u64,
    ) -> Result<crate::attempts::SignedProviderAcceptance> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| invalid("provider negotiation storage busy"))?;
        let journal = self.journal.clone();
        let signer = self.signer.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            approval.begin_signing()?;
            journal
                .sign_provider_acceptance(&approval, &signer, now_ms)
                .map_err(|_| {
                    invalid("provider acceptance was not committed; recover before retrying")
                })
        })
        .await
        .map_err(|_| Error::Task)?
    }
    pub async fn recover(
        &self,
        invocation: Digest,
    ) -> Result<Option<crate::attempts::SignedProviderAcceptance>> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| invalid("provider negotiation storage busy"))?;
        let journal = self.journal.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            match journal
                .get(&invocation)
                .map_err(|_| invalid("provider journal unavailable"))?
            {
                None => Ok(None),
                Some(record) => journal
                    .provider_acceptance(&invocation, record.attempt)
                    .map_err(|_| invalid("provider acceptance recovery failed")),
            }
        })
        .await
        .map_err(|_| Error::Task)?
    }
}
