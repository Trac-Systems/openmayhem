//! Bounded provider proposal orchestration. No listener, raw signer or model POST.
//! Unsigned proposals can expire; signing/uncertain obligations cannot expire here.
use super::{Context, Message, Proposal, Received, Role};
use crate::{
    attempts::{Digest, SignedProviderAcceptance},
    capacity,
    exchange::channel::bounded_json,
    financial::{
        self,
        provider::{Approval, ProviderNegotiation, Runtime},
    },
    invalid, require, Error, Result,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone, Copy)]
pub struct Limits {
    pub pending: usize,
    pub per_buyer: usize,
    pub request_bytes: usize,
    pub total_request_bytes: usize,
    pub storage_operations: usize,
    /// Only unsigned proposal lifetime, not inference or financial expiry.
    pub unsigned_lifetime: Duration,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Proposed,
    Signing,
    Uncertain,
}
struct Pending {
    context: Context,
    commitment: Digest,
    proposal: Proposal,
    request: Vec<u8>,
    reservation: capacity::Reservation,
    expires: Instant,
    phase: Phase,
}
#[derive(Default)]
struct State {
    pending: BTreeMap<Digest, Pending>,
    bytes: usize,
}
/// Counts contain no prompts, addresses, credentials or signatures.
#[derive(Debug)]
pub struct Status {
    pub unsigned: usize,
    pub signing_or_uncertain: usize,
    pub request_bytes: usize,
}
pub struct Reconciliation {
    pub examined: usize,
    pub released: usize,
    pub retained: usize,
    pub next_after: Option<Digest>,
}
struct Inner {
    runtime: Arc<Runtime>,
    signer: Arc<ProviderNegotiation>,
    client: Arc<financial::Client>,
    limits: Limits,
    slots: Arc<Semaphore>,
    state: Mutex<State>,
}
#[derive(Clone)]
pub struct Controller {
    inner: Arc<Inner>,
}
impl Controller {
    pub fn new(
        runtime: Arc<Runtime>,
        signer: Arc<ProviderNegotiation>,
        client: Arc<financial::Client>,
        limits: Limits,
    ) -> Result<Self> {
        require(
            (1..=4096).contains(&limits.pending)
                && limits.per_buyer > 0
                && limits.per_buyer <= limits.pending
                && limits.request_bytes > 0
                && limits.request_bytes <= runtime.adapter.limits().request_bytes
                && limits.total_request_bytes >= limits.request_bytes
                && (1..=64).contains(&limits.storage_operations)
                && !limits.unsigned_lifetime.is_zero()
                && Instant::now()
                    .checked_add(limits.unsigned_lifetime)
                    .is_some()
                && signer.identity()? == *runtime.capacity.identity(),
            "invalid provider proposal controller configuration",
        )?;
        Ok(Self {
            inner: Arc::new(Inner {
                runtime,
                signer,
                client,
                limits,
                slots: Arc::new(Semaphore::new(limits.storage_operations)),
                state: Mutex::new(State::default()),
            }),
        })
    }
    fn slot(&self) -> Result<OwnedSemaphorePermit> {
        self.inner
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| invalid("provider proposal control capacity unavailable"))
    }
    fn authenticate(&self, context: &Context) -> Result<()> {
        context
            .link(self.inner.runtime.capacity.identity(), Role::Provider)
            .map_err(|_| invalid("provider negotiation identity differs"))?;
        require(
            context.contract_version == mayhem_proto::CONTRACT_VERSION,
            "new provider negotiation requires current contract",
        )
    }
    async fn observe(&self, context: &Context) -> Result<financial::offer::Observation> {
        self.inner
            .client
            .offer_state(&financial::offer::Query {
                offer: context.offer.clone(),
                rail: context.rail,
                settlement_policy_hash: context.settlement_policy_hash.as_str().into(),
            })
            .await
    }
    /// Only an authenticated Request can reserve capacity. Repeating the same
    /// still-unsigned context returns the same proposal; it does not allocate twice.
    pub async fn propose(&self, context: Context, received: Received) -> Result<Proposal> {
        let permit = self.slot()?;
        self.authenticate(&context)?;
        let Message::Request { request } = received
            .into_message(&context, Role::Provider)
            .map_err(|_| invalid("request does not belong to this negotiation"))?
        else {
            return Err(invalid("expected authenticated provider request"));
        };
        let bytes = bounded_json(&request, self.inner.limits.request_bytes)
            .map_err(|_| invalid("provider proposal request exceeds bound"))?;
        require(
            !self
                .inner
                .signer
                .has_intent(
                    context
                        .invocation()
                        .map_err(|_| invalid("invalid invocation"))?,
                )
                .await?,
            "provider already retained this attempt; recover instead of proposing again",
        )?;
        let observation = self.observe(&context).await?;
        let inner = self.inner.clone();
        // Cancellation cannot drop the I/O permit or an allocated lease. Pending
        // state belongs to the controller, even when its caller loses this reply.
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            inner.propose(context, bytes, observation)
        })
        .await
        .map_err(|_| Error::Task)?
    }
    /// Once signing starts, cancellation/error retains the slot for recovery.
    /// Invalid offers fail before that transition and can expire as unsigned.
    pub async fn accept(
        &self,
        context: Context,
        received: Received,
        now_ms: u64,
    ) -> Result<SignedProviderAcceptance> {
        let permit = self.slot()?;
        self.authenticate(&context)?;
        let Message::Offer { offer } = received
            .into_message(&context, Role::Provider)
            .map_err(|_| invalid("offer does not belong to this negotiation"))?
        else {
            return Err(invalid("expected authenticated buyer offer"));
        };
        let observation = self.observe(&context).await?;
        let invocation = context
            .invocation()
            .map_err(|_| invalid("invalid invocation"))?;
        let commitment = context.digest().map_err(|_| invalid("invalid context"))?;
        let inner = self.inner.clone();
        let (approval, permit) = tokio::task::spawn_blocking(move || {
            inner
                .approve(&context, offer, observation)
                .map(|a| (a, permit))
        })
        .await
        .map_err(|_| Error::Task)??;
        let result = self.inner.signer.accept(approval, now_ms).await;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut state = inner
                .state
                .lock()
                .map_err(|_| invalid("proposal state unavailable"))?;
            if result.is_ok() {
                // Recovery may already have read the committed signature and
                // removed pending memory. It never released execution capacity.
                if state
                    .pending
                    .get(&invocation)
                    .is_some_and(|p| p.commitment == commitment)
                {
                    state.remove(&invocation)?;
                }
            } else if let Some(pending) = state
                .pending
                .get_mut(&invocation)
                .filter(|p| p.commitment == commitment)
            {
                pending.phase = Phase::Uncertain;
            }
            result
        })
        .await
        .map_err(|_| Error::Task)?
    }
    /// Exact durable signature recovery. A missing signature is not automatically
    /// proof that an interrupted sign/commit or published obligation cannot exist.
    pub async fn recover(&self, context: &Context) -> Result<Option<SignedProviderAcceptance>> {
        let permit = self.slot()?;
        context
            .link(self.inner.runtime.capacity.identity(), Role::Provider)
            .map_err(|_| invalid("provider recovery identity differs"))?;
        let invocation = context
            .invocation()
            .map_err(|_| invalid("invalid invocation"))?;
        let Some(value) = self.inner.signer.recover(invocation.clone()).await? else {
            return Ok(None);
        };
        context
            .bind_terms(&value.authorization.terms)
            .map_err(|_| invalid("saved acceptance belongs to another negotiation"))?;
        let commitment = context.digest().map_err(|_| invalid("invalid context"))?;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut state = inner
                .state
                .lock()
                .map_err(|_| invalid("proposal state unavailable"))?;
            if let Some(pending) = state.pending.get(&invocation) {
                require(
                    pending.commitment == commitment,
                    "saved acceptance context differs from pending negotiation",
                )?;
                state.remove(&invocation)?;
            }
            Ok(Some(value))
        })
        .await
        .map_err(|_| Error::Task)?
    }
    /// Explicit cancellation of an unsigned proposal only. No receipt/hold is
    /// released here; the capability has never entered signing or dispatch.
    pub async fn cancel_unsigned(&self, context: Context) -> Result<bool> {
        let permit = self.slot()?;
        self.authenticate(&context)?;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let invocation = context
                .invocation()
                .map_err(|_| invalid("invalid invocation"))?;
            let mut state = inner
                .state
                .lock()
                .map_err(|_| invalid("proposal state unavailable"))?;
            let Some(pending) = state.pending.get(&invocation) else {
                return Ok(false);
            };
            require(
                pending.commitment == context.digest().map_err(|_| invalid("invalid context"))?
                    && pending.phase == Phase::Proposed,
                "signed or mismatched proposal requires recovery",
            )?;
            inner.cancel(&mut state, &invocation)?;
            Ok(true)
        })
        .await
        .map_err(|_| Error::Task)?
    }
    /// Bounded by configured pending count; no journal/ledger/history scan.
    /// The supervisor must schedule this even when there is no new inference.
    pub async fn expire_unsigned(&self) -> Result<usize> {
        let permit = self.slot()?;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut state = inner
                .state
                .lock()
                .map_err(|_| invalid("proposal state unavailable"))?;
            let now = Instant::now();
            let expired: Vec<_> = state
                .pending
                .iter()
                .filter(|(_, p)| p.phase == Phase::Proposed && p.expires <= now)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &expired {
                inner.cancel(&mut state, id)?;
            }
            Ok(expired.len())
        })
        .await
        .map_err(|_| Error::Task)?
    }
    pub async fn status(&self) -> Result<Status> {
        let permit = self.slot()?;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let state = inner
                .state
                .lock()
                .map_err(|_| invalid("proposal state unavailable"))?;
            let unsigned = state
                .pending
                .values()
                .filter(|p| p.phase == Phase::Proposed)
                .count();
            Ok(Status {
                unsigned,
                signing_or_uncertain: state.pending.len() - unsigned,
                request_bytes: state.bytes,
            })
        })
        .await
        .map_err(|_| Error::Task)?
    }
    /// One bounded page of this route's prior-controller allocations. Only the
    /// durable never-signed proposal phase permits release. Protected obligations
    /// remain available to the separate financial/execution recovery controller.
    pub async fn reconcile_unsigned(
        &self,
        after: Option<Digest>,
        limit: usize,
    ) -> Result<Reconciliation> {
        let permit = self.slot()?;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let cap = &inner.runtime.capacity;
            let route = &inner.runtime.route;
            let group = cap
                .route_group(route)
                .map_err(|_| invalid("provider recovery group unavailable"))?;
            let page = cap
                .recover_group(&group, after.as_ref(), limit)
                .map_err(|_| invalid("provider recovery page unavailable"))?;
            let mut result = Reconciliation {
                examined: page.leases.len(),
                released: 0,
                retained: 0,
                next_after: page.next_after,
            };
            for lease in page.leases {
                if lease.route == *route
                    && cap
                        .reclaim_old_proposal(&lease, route)
                        .map_err(|_| invalid("provider unsigned capacity reconciliation failed"))?
                {
                    result.released += 1;
                } else {
                    result.retained += 1;
                }
            }
            Ok(result)
        })
        .await
        .map_err(|_| Error::Task)?
    }
}
impl State {
    fn remove(&mut self, invocation: &Digest) -> Result<Pending> {
        let pending = self
            .pending
            .remove(invocation)
            .ok_or_else(|| invalid("proposal missing"))?;
        self.bytes = self
            .bytes
            .checked_sub(pending.request.len())
            .ok_or_else(|| invalid("proposal byte accounting differs"))?;
        Ok(pending)
    }
}
impl Inner {
    fn propose(
        &self,
        context: Context,
        request: Vec<u8>,
        observed: financial::offer::Observation,
    ) -> Result<Proposal> {
        observed.check_proposal(&context, &self.runtime.approved_policy)?;
        let value: serde_json::Value = serde_json::from_slice(&request)?;
        let prepared = if value.get("stream") == Some(&serde_json::Value::Bool(true)) {
            self.runtime.adapter.prepare_stream(&request)
        } else {
            self.runtime.adapter.prepare_json(&request)
        }
        .map_err(|_| invalid("invalid provider proposal request"))?;
        require(
            prepared.request_hash() == &context.request_hash,
            "proposal request fingerprint differs",
        )?;
        let member = observed.membership()?;
        require(
            member.recipe_hash == self.runtime.adapter.recipe_hash().as_str()
                && member.connection_revision == self.runtime.connection.revision()
                && member.endpoints.iter().any(|e| {
                    e.endpoint == self.runtime.adapter.endpoint()
                        && e.contract_hash == self.runtime.adapter.contract_hash().as_str()
                }),
            "provider runtime differs from canonical membership",
        )?;
        let invocation = context
            .invocation()
            .map_err(|_| invalid("invalid invocation"))?;
        let commitment = context.digest().map_err(|_| invalid("invalid context"))?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| invalid("proposal state unavailable"))?;
        if let Some(old) = state.pending.get(&invocation) {
            require(
                old.commitment == commitment
                    && old.phase == Phase::Proposed
                    && Instant::now() < old.expires,
                "existing proposal must be recovered or expired",
            )?;
            self.runtime
                .capacity
                .check_reserved(old.reservation.lease())
                .map_err(|_| invalid("proposal capacity no longer ready"))?;
            return Ok(old.proposal.clone());
        }
        require(
            state.pending.len() < self.limits.pending
                && state
                    .pending
                    .values()
                    .filter(|p| p.context.buyer == context.buyer)
                    .count()
                    < self.limits.per_buyer
                && request.len() <= self.limits.total_request_bytes.saturating_sub(state.bytes),
            "provider pending proposal quota reached",
        )?;
        let reservation = self
            .runtime
            .capacity
            .reserve_proposal(
                &self.runtime.route,
                capacity::Work {
                    invocation: invocation.clone(),
                    request_hash: context.request_hash.clone(),
                },
            )
            .map_err(|_| {
                invalid("provider capacity cannot accept proposal; recover existing work")
            })?;
        let route = self.runtime.capacity.check_reserved(reservation.lease());
        if !route.is_ok_and(|r| {
            r.lane == capacity::Lane::Proxy
                && r.group.as_str() == member.capacity_group
                && r.max_concurrency <= member.max_concurrency
        }) {
            self.runtime
                .capacity
                .cancel_reserved(reservation)
                .map_err(|_| invalid("proposal capacity cleanup requires reconciliation"))?;
            return Err(invalid(
                "provider capacity configuration differs from membership",
            ));
        }
        let proposal = Proposal {
            adapter: self.runtime.adapter.public_snapshot(),
            reservation_id: Digest::hash(
                "mayhem/proxy/proposed-reservation/v1",
                &[
                    commitment.as_str().as_bytes(),
                    reservation.lease().id.as_str().as_bytes(),
                ],
            ),
            connection_digest: self.runtime.connection.fingerprint().clone(),
            connection_revision: self.runtime.connection.revision(),
            capacity_lease: reservation.lease().id.clone(),
        };
        state.bytes += request.len();
        state.pending.insert(
            invocation,
            Pending {
                context,
                commitment,
                proposal: proposal.clone(),
                request,
                reservation,
                expires: Instant::now() + self.limits.unsigned_lifetime,
                phase: Phase::Proposed,
            },
        );
        Ok(proposal)
    }
    fn approve(
        &self,
        context: &Context,
        offer: financial::negotiation::BuyerOffer,
        observation: financial::offer::Observation,
    ) -> Result<Approval> {
        let invocation = context
            .invocation()
            .map_err(|_| invalid("invalid invocation"))?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| invalid("proposal state unavailable"))?;
        let pending = state
            .pending
            .get_mut(&invocation)
            .ok_or_else(|| invalid("provider proposal missing; recover before retrying"))?;
        require(
            pending.commitment == context.digest().map_err(|_| invalid("invalid context"))?
                && pending.phase == Phase::Proposed
                && Instant::now() < pending.expires,
            "provider proposal expired or already signing; recover before retrying",
        )?;
        context
            .bind_terms(&offer.terms)
            .map_err(|_| invalid("buyer offer context differs"))?;
        pending
            .proposal
            .bind_terms(&offer.terms)
            .map_err(|_| invalid("buyer offer differs from proposal"))?;
        let approved = self.runtime.approve(
            offer,
            observation,
            &pending.reservation,
            pending.request.clone(),
        )?;
        pending.phase = Phase::Signing;
        Ok(approved)
    }
    fn cancel(&self, state: &mut State, invocation: &Digest) -> Result<()> {
        // Removal is serialized with approve. Failure leaves durable capacity
        // occupied and returns an explicit error; never pretend cleanup succeeded.
        let pending = state
            .pending
            .get(invocation)
            .ok_or_else(|| invalid("proposal missing"))?;
        self.runtime
            .capacity
            .cancel_reserved_ref(&pending.reservation)
            .map_err(|_| invalid("unsigned capacity cleanup requires reconciliation"))?;
        state.remove(invocation)?;
        Ok(())
    }
}
