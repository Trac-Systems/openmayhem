//! Owned, bounded JSON and provisional streaming buyer sessions. Selection, authenticated HTTP ownership and
//! account/key budgets belong to the caller. This controller never selects another
//! offer, changes a rail, retries Execute, or calls a native inference backend.
pub use crate::financial::negotiation::NonAdmission;
mod streaming;
use crate::{
    attempts::{Digest, Identity},
    buyer::Evidence,
    endpoint::{self, PublicAdapter},
    exchange::{self, Message, PublicState, Role},
    financial::{
        self,
        negotiation::{BuyerNegotiation, SavedPurchase},
        quote::{Lifetimes, PreparedPurchase, PriceLimits, PurchaseRequest},
        recovery::{BuyerRecovery, FinancialOutcome, SignedAcknowledgment},
    },
    negotiation,
    signing::Authority,
    worker::host::Pool,
};
use mayhem_bridge::ScBridgeConfig;
use mayhem_proto::proxy::{
    finance::{ProxySettlementPolicy, ProxySpendAuthorization},
    ProxyEndpoint,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
pub use streaming::{stream_channel, StreamEvent, StreamLimits, StreamReceiver, StreamSender};
use tokio::{
    sync::{oneshot, watch, OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
};

/// Construct before admission and retain alongside the caller's idempotency key.
/// All retries/recovery must use the original identity, never a replacement ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestIdentity {
    pub billing_id: Digest,
    pub billing_attempt: u64,
    pub session_id: Digest,
    pub request_hash: Digest,
}
impl From<&negotiation::Context> for RequestIdentity {
    fn from(context: &negotiation::Context) -> Self {
        Self {
            billing_id: context.billing_id.clone(),
            billing_attempt: context.billing_attempt,
            session_id: context.session_id.clone(),
            request_hash: context.request_hash.clone(),
        }
    }
}

/// Explicit, already resolved buyer authorization. A balance/catalog row is not
/// consent. No defaults enlarge unit prices, total exposure or epoch lifetimes.
pub struct Authorization {
    pub prices: PriceLimits,
    pub output_units: Option<u64>,
    pub lifetimes: Lifetimes,
    pub settlement_policy: ProxySettlementPolicy,
    pub endpoint_contract: Digest,
    pub recipe_hash: Digest,
}
pub struct Request {
    pub context: negotiation::Context,
    pub body: Vec<u8>,
    pub authorization: Authorization,
    pub gate: Arc<dyn AuthorizationGate>,
}
impl Request {
    pub fn identity(&self) -> RequestIdentity {
        (&self.context).into()
    }
}

#[derive(Clone, Copy, Debug)]
pub enum GateError {
    Rejected,
    Unavailable,
}
/// Trusted owner callback, never a public/plugin-supplied signing capability.
/// Idempotently bind the original job and reserve the exact terms.max_spend_au
/// durably before returning success. This owner record must also cover failure
/// or cancellation before the buyer negotiation journal exists; pending
/// negotiation rows are not a complete index of owner budget exposure.
/// Implementations must bound their own I/O/work. Cancellation joins an ambiguous
/// commit instead of dropping it. No default allow implementation exists.
pub trait AuthorizationGate: Send + Sync {
    /// Durably close the exact owner intent/budget using the controller's
    /// permanent signing fence. Missing journal rows are never this proof.
    fn retain_non_admission<'a>(
        &'a self,
        proof: &'a NonAdmission,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<(), GateError>> + Send + 'a>>;
    fn authorize<'a>(
        &'a self,
        purchase: &'a PreparedPurchase,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<(), GateError>> + Send + 'a>>;
    /// Idempotently persist the verified response under the original job before
    /// its payment acknowledgment can escape. Returning an error retains the
    /// purchase for recovery; the owner must not report an answer as durable.
    fn retain_verified_output<'a>(
        &'a self,
        output: VerifiedOutput<'a>,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<(), GateError>> + Send + 'a>>;
}

/// Constructed only after independent verification of the delivered result and
/// exact signed provider receipt. References cannot escape the owner's callback.
pub struct VerifiedOutput<'a> {
    identity: &'a RequestIdentity,
    authorization: &'a ProxySpendAuthorization,
    response: &'a Value,
    receipt: &'a crate::signing::ProviderReceipt,
}
impl<'a> VerifiedOutput<'a> {
    pub fn identity(&self) -> &RequestIdentity {
        self.identity
    }
    pub fn authorization(&self) -> &ProxySpendAuthorization {
        self.authorization
    }
    pub fn response(&self) -> &Value {
        self.response
    }
    pub fn receipt(&self) -> &crate::signing::ProviderReceipt {
        self.receipt
    }
}

#[derive(Clone, Copy)]
pub struct Limits {
    pub sessions: usize,
    /// Admission accounting for worst-case encoded buffers/retained records;
    /// this is not a bound on allocator overhead or total process RSS.
    pub buffer_bytes: usize,
    pub protocol: endpoint::Limits,
    pub message_bytes: usize,
    /// Negotiation/recovery/closure control only, never a generation timeout.
    pub control_wait: Duration,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Admission,
    Negotiating,
    Authorizing,
    Signing,
    Accepted,
    Funded,
    Executing,
    Verifying,
    Acknowledging,
    Recovery,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    Invalid,
    Busy,
    Stopped,
    Storage,
    Transport,
    Refused,
    RecoveryRequired,
    Verification,
    Unconfirmed,
}
#[derive(Debug, thiserror::Error)]
#[error("proxy buyer operation failed: {code:?} at {stage:?}")]
pub struct Error {
    pub identity: RequestIdentity,
    pub code: Code,
    pub stage: Stage,
    /// False only describes this local attempt before signing; it is never a
    /// permission for cross-provider, cross-rail or native fallback.
    pub recovery_required: bool,
}
pub type Result<T> = std::result::Result<T, Error>;

pub enum Outcome {
    /// The exact unsigned intent is durably fenced against all future signing,
    /// and its owner has retained that proof. This is not an inference result.
    NotAdmitted {
        identity: RequestIdentity,
        proof: NonAdmission,
    },
    /// Independently verified original output plus canonical signed settlement.
    Completed {
        identity: RequestIdentity,
        response: Value,
        authorization: ProxySpendAuthorization,
        settlement: FinancialOutcome,
    },
    /// Canonical financial closure; may be Paid, Waived or ExpiredUnknown.
    /// Absence of a response does not establish non-execution or retry safety.
    Closed {
        identity: RequestIdentity,
        authorization: ProxySpendAuthorization,
        settlement: FinancialOutcome,
    },
    /// Original accepted purchase remains open. Recovery never resends Execute.
    Pending {
        identity: RequestIdentity,
        state: PublicState,
    },
}
impl std::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (name, identity) = match self {
            Self::NotAdmitted { identity, .. } => ("NotAdmitted", identity),
            Self::Completed { identity, .. } => ("Completed", identity),
            Self::Closed { identity, .. } => ("Closed", identity),
            Self::Pending { identity, .. } => ("Pending", identity),
        };
        f.debug_struct(name)
            .field("identity", identity)
            .finish_non_exhaustive()
    }
}

struct Shared {
    negotiation: Arc<BuyerNegotiation>,
    recovery: Arc<BuyerRecovery>,
    client: Arc<financial::Client>,
    signer: Arc<Authority>,
    verifier: Arc<Pool>,
    bridge: ScBridgeConfig,
    limits: Limits,
}
type Slot = (Digest, u64);
struct Guard {
    _session: OwnedSemaphorePermit,
    _buffers: OwnedSemaphorePermit,
    active: Arc<Mutex<BTreeSet<Slot>>>,
    key: Slot,
}
impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(&self.key);
        }
    }
}
/// The owner must call shutdown().await before dropping the runtime. Dropping an
/// HTTP await signals cancellation; the owned task keeps permits until pending
/// durable commits finish. Finished handles are reaped on the next admission.
pub struct Controller {
    shared: Arc<Shared>,
    sessions: Arc<Semaphore>,
    buffers: Arc<Semaphore>,
    reservation: u32,
    active: Arc<Mutex<BTreeSet<Slot>>>,
    tasks: Mutex<JoinSet<()>>,
    stopped: AtomicBool,
    shutdown: watch::Sender<bool>,
    joining: tokio::sync::Mutex<()>,
}
enum Operation {
    Execute(Request, Option<StreamSender>),
    Recover(RequestIdentity, Arc<dyn AuthorizationGate>),
}
const BUFFER_UNIT: usize = 64 * 1024;
impl Controller {
    pub fn new(
        negotiation: Arc<BuyerNegotiation>,
        recovery: Arc<BuyerRecovery>,
        client: Arc<financial::Client>,
        signer: Arc<Authority>,
        verifier: Arc<Pool>,
        bridge: ScBridgeConfig,
        limits: Limits,
    ) -> crate::Result<Self> {
        let identity = signer.identity();
        let network = client.identity();
        crate::require(
            identity.network_id == network.network_id
                && identity.msb_bootstrap.as_str() == network.msb_bootstrap
                && identity.subnet_bootstrap.as_str() == network.subnet_bootstrap
                && identity.controller_pubkey.as_str() == client.requester(),
            "buyer client/wallet differs",
        )?;
        crate::require(
            (1..=64).contains(&limits.sessions)
                && !limits.control_wait.is_zero()
                && limits.control_wait <= Duration::from_secs(60)
                && (1..=256 * 1024 * 1024).contains(&limits.message_bytes)
                && bridge.operation_deadline.is_some_and(|v| !v.is_zero())
                && bridge.max_queued_bytes > 0
                && bridge.max_queued_events > 0,
            "invalid buyer session limits",
        )?;
        // Validate all protocol limits through the existing public adapter.
        PublicAdapter::new(
            ProxyEndpoint::Chat,
            mayhem_proto::endpoint_family_contract_template(
                mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
            )
            .ok_or_else(|| crate::invalid("chat contract missing"))?,
            Digest::hash("buyer-limits", &[]),
            limits.protocol,
        )
        .map_err(|_| crate::invalid("invalid buyer protocol bounds"))?;
        // Existing records can exceed newly lowered admission limits. The
        // metadata accessor includes retained allocations without scanning rows.
        let record = negotiation.max_record_bytes()?;
        let reserved = record
            .checked_mul(4)
            .and_then(|v| v.checked_add(limits.protocol.request_bytes.checked_mul(4)?))
            .and_then(|v| v.checked_add(limits.protocol.response_bytes.checked_mul(4)?))
            .and_then(|v| v.checked_add(limits.message_bytes.checked_mul(2)?))
            .and_then(|v| v.checked_add(bridge.max_queued_bytes))
            .and_then(|v| v.checked_add(512 * 1024))
            .ok_or_else(|| crate::invalid("buyer buffer accounting overflow"))?;
        let units = |bytes: usize| {
            u32::try_from(bytes.div_ceil(BUFFER_UNIT))
                .map_err(|_| crate::invalid("buyer buffer bound too large"))
        };
        let reservation = units(reserved)?;
        let budget = limits.buffer_bytes / BUFFER_UNIT;
        crate::require(
            budget >= reservation as usize && budget <= u32::MAX as usize,
            "buyer buffer budget is too small",
        )?;
        let (shutdown, _) = watch::channel(false);
        Ok(Self {
            shared: Arc::new(Shared {
                negotiation,
                recovery,
                client,
                signer,
                verifier,
                bridge,
                limits,
            }),
            sessions: Arc::new(Semaphore::new(limits.sessions)),
            buffers: Arc::new(Semaphore::new(budget)),
            reservation,
            active: Arc::new(Mutex::new(BTreeSet::new())),
            tasks: Mutex::new(JoinSet::new()),
            stopped: AtomicBool::new(false),
            shutdown,
            joining: tokio::sync::Mutex::new(()),
        })
    }
    pub fn identity(&self) -> &Identity {
        self.shared.signer.identity()
    }
    pub fn active_sessions(&self) -> usize {
        self.shared.limits.sessions - self.sessions.available_permits()
    }

    /// Read the original durable purchase from this controller's own journal.
    /// Absence is not proof that signing or a storage commit never occurred.
    pub async fn retained_purchase(
        &self,
        identity: &RequestIdentity,
    ) -> crate::Result<Option<SavedPurchase>> {
        let saved = self
            .shared
            .negotiation
            .lookup(identity.billing_id.clone(), identity.billing_attempt)
            .await?;
        if let Some(saved) = &saved {
            let terms = saved.offer().terms;
            crate::require(
                terms.session_id == identity.session_id.as_str()
                    && terms.request_hash == identity.request_hash.as_str()
                    && terms.buyer_pubkey == self.identity().controller_pubkey.as_str(),
                "retained buyer purchase identity differs",
            )?;
        }
        Ok(saved)
    }

    /// Obtain fresh canonical evidence through the same identity-checked client
    /// used for purchase. The gateway must persist it before closing exposure.
    pub async fn observe_purchase(
        &self,
        saved: &SavedPurchase,
    ) -> crate::Result<financial::Observation> {
        let authorization = saved
            .authorization()
            .ok_or_else(|| crate::invalid("buyer acceptance is not retained"))?;
        crate::require(
            authorization.terms.buyer_pubkey == self.identity().controller_pubkey.as_str(),
            "buyer purchase owner differs",
        )?;
        self.shared.client.observe(&authorization).await
    }

    pub async fn execute(&self, request: Request, stop: watch::Receiver<bool>) -> Result<Outcome> {
        let identity = request.identity();
        self.launch(Operation::Execute(request, None), identity, stop)
            .await
    }
    /// Execute a request containing stream:true. Events are bounded provisional
    /// normalized JSON; only the returned outcome can establish verified closure.
    /// Dropping the receiver cancels delivery, never erases the original purchase.
    pub async fn execute_stream(
        &self,
        request: Request,
        observer: StreamSender,
        stop: watch::Receiver<bool>,
    ) -> Result<Outcome> {
        let identity = request.identity();
        self.launch(Operation::Execute(request, Some(observer)), identity, stop)
            .await
    }
    /// Reconcile the exact original purchase. No new quote, prices, signatures
    /// for a different purchase, reservation identity or Execute are generated.
    pub async fn recover(
        &self,
        identity: RequestIdentity,
        gate: Arc<dyn AuthorizationGate>,
        stop: watch::Receiver<bool>,
    ) -> Result<Outcome> {
        self.launch(Operation::Recover(identity.clone(), gate), identity, stop)
            .await
    }
    async fn launch(
        &self,
        mut operation: Operation,
        identity: RequestIdentity,
        mut external: watch::Receiver<bool>,
    ) -> Result<Outcome> {
        let fail = |code| Error {
            identity: identity.clone(),
            code,
            stage: Stage::Admission,
            recovery_required: true,
        };
        if self.stopped.load(Ordering::Acquire) || *external.borrow() {
            return Err(fail(Code::Stopped));
        }
        if matches!(&operation, Operation::Execute(request, _) if request.body.len() > self.shared.limits.protocol.request_bytes)
        {
            return Err(fail(Code::Invalid));
        }
        let session = self
            .sessions
            .clone()
            .try_acquire_owned()
            .map_err(|_| fail(Code::Busy))?;
        let extra = match &operation {
            Operation::Execute(_, Some(observer)) => {
                if observer.limits.total_bytes > self.shared.limits.protocol.response_bytes {
                    return Err(fail(Code::Invalid));
                }
                // Assembled/normalized content is additional to ordinary JSON
                // verification/owner buffers. Queue permits outlive this task.
                u32::try_from(
                    (self.shared.limits.protocol.response_bytes * 2).div_ceil(BUFFER_UNIT),
                )
                .map_err(|_| fail(Code::Invalid))?
            }
            _ => 0,
        };
        let reservation = self
            .reservation
            .checked_add(extra)
            .ok_or_else(|| fail(Code::Invalid))?;
        let disconnected: Pin<Box<dyn Future<Output = ()> + Send>> = match &operation {
            Operation::Execute(_, Some(observer)) => Box::pin(observer.disconnected()),
            _ => Box::pin(std::future::pending()),
        };
        let buffers = self
            .buffers
            .clone()
            .try_acquire_many_owned(reservation)
            .map_err(|_| fail(Code::Busy))?;
        if let Operation::Execute(_, Some(observer)) = &mut operation {
            let units = u32::try_from(observer.limits.queued_bytes.div_ceil(BUFFER_UNIT))
                .map_err(|_| fail(Code::Invalid))?;
            observer.charge(
                self.buffers
                    .clone()
                    .try_acquire_many_owned(units)
                    .map_err(|_| fail(Code::Busy))?,
            );
        }
        let key = (identity.billing_id.clone(), identity.billing_attempt);
        if !self
            .active
            .lock()
            .map_err(|_| fail(Code::Storage))?
            .insert(key.clone())
        {
            return Err(fail(Code::Busy));
        }
        let guard = Guard {
            _session: session,
            _buffers: buffers,
            active: self.active.clone(),
            key,
        };
        let receiver = {
            let mut tasks = self.tasks.lock().map_err(|_| fail(Code::Storage))?;
            while tasks.try_join_next().is_some() {}
            if self.stopped.load(Ordering::Acquire) {
                return Err(fail(Code::Stopped));
            }
            let shared = self.shared.clone();
            let mut shutdown = self.shutdown.subscribe();
            let (mut result, receiver) = oneshot::channel();
            let task_identity = identity.clone();
            tasks.spawn(async move {
                let _guard = guard;
                let (cancel, cancelled) = watch::channel(false);
                let work = shared.run(operation, task_identity, cancelled);
                tokio::pin!(work);
                let outcome = tokio::select! {
                    value = &mut work => value,
                    _ = stopped(&mut external) => { cancel.send_replace(true); work.await },
                    _ = stopped(&mut shutdown) => { cancel.send_replace(true); work.await },
                    _ = result.closed() => { cancel.send_replace(true); work.await },
                    _ = disconnected => { cancel.send_replace(true); work.await },
                };
                let _ = result.send(outcome);
            });
            receiver
        };
        receiver.await.map_err(|_| fail(Code::Storage))?
    }
    pub async fn shutdown(&self) -> crate::Result<()> {
        let _joining = self.joining.lock().await;
        self.stopped.store(true, Ordering::Release);
        self.shutdown.send_replace(true);
        let mut failed = false;
        loop {
            // Keep ownership in Controller while awaiting. Cancelling a shutdown
            // waiter must not drop a local JoinSet and abort durable owner work.
            let result = std::future::poll_fn(|cx| match self.tasks.lock() {
                Ok(mut tasks) => tasks.poll_join_next(cx).map(Ok),
                Err(_) => std::task::Poll::Ready(Err(crate::invalid("buyer task owner poisoned"))),
            })
            .await?;
            match result {
                Some(result) => failed |= result.is_err(),
                None => break,
            }
        }
        crate::require(!failed, "buyer session task failed")
    }
}

async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow() || stop.changed().await.is_err() {
            return;
        }
    }
}
struct Progress {
    identity: RequestIdentity,
    stage: Stage,
    retained: bool,
}
impl Progress {
    fn error(&self, code: Code) -> Error {
        Error {
            identity: self.identity.clone(),
            code,
            stage: self.stage,
            recovery_required: self.retained,
        }
    }
    fn check(&self, stop: &watch::Receiver<bool>) -> Result<()> {
        if *stop.borrow() {
            Err(self.error(Code::Stopped))
        } else {
            Ok(())
        }
    }
}
impl Shared {
    async fn close_unsigned(
        &self,
        p: &mut Progress,
        gate: &Arc<dyn AuthorizationGate>,
    ) -> Result<Outcome> {
        p.stage = Stage::Recovery;
        let proof = self
            .negotiation
            .fence_unsigned(p.identity.clone())
            .await
            .map_err(|_| p.error(Code::Storage))?
            .ok_or_else(|| p.error(Code::RecoveryRequired))?;
        gate.retain_non_admission(&proof)
            .await
            .map_err(|_| p.error(Code::Storage))?;
        Ok(Outcome::NotAdmitted {
            identity: p.identity.clone(),
            proof,
        })
    }
    async fn run(
        &self,
        operation: Operation,
        identity: RequestIdentity,
        mut stop: watch::Receiver<bool>,
    ) -> Result<Outcome> {
        let mut p = Progress {
            identity,
            stage: Stage::Admission,
            retained: true,
        };
        p.check(&stop)?;
        // This lookup precedes any quote or new signature, including after an
        // earlier caller lost the durable signing acknowledgment.
        let existing = self
            .negotiation
            .lookup(p.identity.billing_id.clone(), p.identity.billing_attempt)
            .await
            .map_err(|_| p.error(Code::Storage))?;
        p.check(&stop)?;
        match operation {
            Operation::Execute(request, observer) => {
                if existing.is_some()
                    || self
                        .negotiation
                        .has_signing_fence(
                            p.identity.billing_id.clone(),
                            p.identity.billing_attempt,
                        )
                        .await
                        .map_err(|_| p.error(Code::Storage))?
                {
                    return Err(p.error(Code::RecoveryRequired));
                }
                p.retained = false;
                self.execute(request, observer, &mut p, &mut stop).await
            }
            Operation::Recover(identity, gate) => {
                if identity != p.identity {
                    return Err(p.error(Code::Invalid));
                }
                if let Some(existing) = existing {
                    self.recover(existing, &mut p, &mut stop, gate).await
                } else {
                    self.close_unsigned(&mut p, &gate).await
                }
            }
        }
    }
    async fn execute(
        &self,
        request: Request,
        observer: Option<StreamSender>,
        p: &mut Progress,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<Outcome> {
        let Request {
            context,
            body,
            authorization,
            gate,
        } = request;
        if body.len() > self.limits.protocol.request_bytes
            || !matches!(
                context.offer.endpoint,
                ProxyEndpoint::Chat
                    | ProxyEndpoint::Completions
                    | ProxyEndpoint::Responses
                    | ProxyEndpoint::Decisions
            )
            || context.contract_version != mayhem_proto::CONTRACT_VERSION
            || context.validate().is_err()
            || authorization.prices.permits(&context.offer).is_err()
            || authorization.settlement_policy.digest().ok().as_deref()
                != Some(context.settlement_policy_hash.as_str())
        {
            return Err(p.error(Code::Invalid));
        }
        let value: Value = serde_json::from_slice(&body).map_err(|_| p.error(Code::Invalid))?;
        let streaming = observer.is_some();
        if (streaming
            && (context.offer.endpoint == ProxyEndpoint::Decisions
                || value.get("stream") != Some(&Value::Bool(true))))
            || (!streaming
                && value
                    .get("stream")
                    .is_some_and(|v| v != &Value::Bool(false)))
            || mayhem_proto::endpoint_request_fingerprint(&value) != context.request_hash.as_str()
        {
            return Err(p.error(Code::Invalid));
        }
        p.stage = Stage::Negotiating;
        let mut channel = tokio::select! {
            result = negotiation::Channel::dial(self.bridge.clone(), context.clone(), self.signer.identity(), exchange::Limits { max_message_bytes: self.limits.message_bytes }) => result.map_err(|_| p.error(Code::Transport))?,
            _ = stopped(stop) => return Err(p.error(Code::Stopped)),
        };
        channel
            .send(&negotiation::Message::Request {
                request: value.clone(),
            })
            .await
            .map_err(|_| p.error(Code::Transport))?;
        let response = tokio::select! {
            result = channel.receive(self.limits.control_wait) => result.map_err(|_| p.error(Code::Transport))?,
            _ = stopped(stop) => return Err(p.error(Code::Stopped)),
        }.into_message(&context, Role::Buyer).map_err(|_| p.error(Code::Verification))?;
        let negotiation::Message::Proposal { mut proposal } = response else {
            return Err(p.error(Code::Refused));
        };
        // Public limits are buyer-local and do not change the opaque recipe hash.
        // Never adopt the provider's memory/choice/schema resource allowances.
        proposal.adapter.limits = self.limits.protocol;
        let adapter =
            PublicAdapter::restore(proposal.adapter.clone()).map_err(|_| p.error(Code::Invalid))?;
        if adapter.contract_hash() != &authorization.endpoint_contract
            || adapter.recipe_hash() != &authorization.recipe_hash
        {
            return Err(p.error(Code::Verification));
        }
        let purchase_request = PurchaseRequest::new(
            proposal.adapter.clone(),
            body,
            authorization.prices,
            authorization.output_units,
            authorization.lifetimes,
        )
        .map_err(|_| p.error(Code::Invalid))?;
        let query = financial::quote::Query {
            offer: context.offer.clone(),
            rail: context.rail,
            settlement_policy_hash: context.settlement_policy_hash.as_str().into(),
            billing_id: context.billing_id.as_str().into(),
        };
        let quote = self
            .client
            .quote(&query)
            .await
            .map_err(|_| p.error(Code::Verification))?;
        if quote.policy().map_err(|_| p.error(Code::Verification))?
            != &authorization.settlement_policy
        {
            return Err(p.error(Code::Verification));
        }
        let purchase = quote
            .prepare_purchase(
                &purchase_request,
                &proposal
                    .session_binding(&context)
                    .map_err(|_| p.error(Code::Verification))?,
            )
            .map_err(|_| p.error(Code::Verification))?;
        if purchase.terms().billing_attempt != context.billing_attempt {
            return Err(p.error(Code::RecoveryRequired));
        }
        p.check(stop)?;
        p.stage = Stage::Authorizing;
        // Commit the unsigned signing fence before the owner can retain budget.
        // A crash after this point can explicitly fence this exact intent closed.
        p.retained = true; // Any lost durable acknowledgment requires recovery.
        self.negotiation
            .retain_unsigned(&purchase)
            .await
            .map_err(|_| p.error(Code::Storage))?;
        let authorized = gate.authorize(&purchase).await;
        if authorized.is_err() || *stop.borrow() {
            return self.close_unsigned(p, &gate).await;
        }
        p.stage = Stage::Signing;
        p.retained = true; // Failed/lost fsync acknowledgment is deliberately uncertain.
        let saved = self
            .negotiation
            .sign(
                purchase,
                quote,
                self.signer.clone(),
                crate::supervisor::unix_ms(),
            )
            .await;
        let saved = match saved {
            Ok(saved) => saved,
            Err(_) => return self.close_unsigned(p, &gate).await,
        };
        p.check(stop)?;
        channel
            .send(&negotiation::Message::Offer {
                offer: saved.offer(),
            })
            .await
            .map_err(|_| p.error(Code::Transport))?;
        let response = tokio::select! {
            result = channel.receive(self.limits.control_wait) => result.map_err(|_| p.error(Code::Transport))?,
            _ = stopped(stop) => return Err(p.error(Code::Stopped)),
        }.into_message(&context, Role::Buyer).map_err(|_| p.error(Code::Verification))?;
        let negotiation::Message::Accepted { value: accepted } = response else {
            return Err(p.error(Code::Refused));
        };
        self.negotiation
            .retain_provider_acceptance(accepted.authorization)
            .await
            .map_err(|_| p.error(Code::Storage))?;
        p.stage = Stage::Accepted;
        p.check(stop)?;
        let saved = self
            .negotiation
            .publish(
                saved.key().clone(),
                &self.recovery,
                crate::supervisor::unix_ms(),
            )
            .await
            .map_err(|_| p.error(Code::Unconfirmed))?;
        p.stage = Stage::Funded;
        p.check(stop)?;
        if !saved.confirmed() {
            return Err(p.error(Code::Unconfirmed));
        }
        let mut paid = channel
            .into_paid(self.signer.identity())
            .map_err(|_| p.error(Code::Verification))?;
        p.stage = Stage::Executing;
        paid.send(&Message::Execute {
            request: value,
            streaming,
        })
        .await
        .map_err(|_| p.error(Code::Transport))?;
        self.finish(paid, saved, p, stop, false, &gate, observer)
            .await
    }

    async fn recover(
        &self,
        mut saved: SavedPurchase,
        p: &mut Progress,
        stop: &mut watch::Receiver<bool>,
        gate: Arc<dyn AuthorizationGate>,
    ) -> Result<Outcome> {
        p.stage = Stage::Recovery;
        let terms = saved.offer().terms;
        let context = negotiation::Context {
            schema_version: 1,
            network_id: terms.network_id.clone(),
            msb_bootstrap: Digest::new(&terms.msb_bootstrap)
                .map_err(|_| p.error(Code::Verification))?,
            subnet_bootstrap: Digest::new(&terms.subnet_bootstrap)
                .map_err(|_| p.error(Code::Verification))?,
            contract_version: terms.contract_version,
            session_id: Digest::new(&terms.session_id).map_err(|_| p.error(Code::Verification))?,
            buyer: Digest::new(&terms.buyer_pubkey).map_err(|_| p.error(Code::Verification))?,
            billing_id: Digest::new(&terms.billing_id).map_err(|_| p.error(Code::Verification))?,
            billing_attempt: terms.billing_attempt,
            request_hash: Digest::new(&terms.request_hash)
                .map_err(|_| p.error(Code::Verification))?,
            offer: terms.offer,
            rail: terms.rail,
            settlement_policy_hash: Digest::new(&terms.settlement_policy_hash)
                .map_err(|_| p.error(Code::Verification))?,
        };
        if RequestIdentity::from(&context) != p.identity
            || saved.request().len() > self.limits.protocol.request_bytes
        {
            return Err(p.error(Code::Invalid));
        }
        let snapshot = saved
            .snapshot()
            .public_snapshot()
            .map_err(|_| p.error(Code::Verification))?;
        if snapshot.adapter.limits.response_bytes > self.limits.protocol.response_bytes {
            return Err(p.error(Code::Invalid));
        }
        saved = self
            .negotiation
            .refresh(saved.key().clone(), crate::supervisor::unix_ms())
            .await
            .map_err(|_| p.error(Code::Unconfirmed))?;
        // A countersignature is not canonical admission. Only observe the
        // recovery ledger here after admission; otherwise recover the original
        // acceptance and publish the same reservation below.
        if saved.confirmed() {
            let auth = saved
                .authorization()
                .ok_or_else(|| p.error(Code::Verification))?;
            if let Some(settlement) = self
                .recovery
                .refresh(&auth, crate::supervisor::unix_ms())
                .await
                .map_err(|_| p.error(Code::Unconfirmed))?
            {
                return Ok(Outcome::Closed {
                    identity: p.identity.clone(),
                    authorization: auth,
                    settlement,
                });
            }
        }
        p.check(stop)?;
        let mut channel = tokio::select! {
            result = negotiation::Channel::dial(self.bridge.clone(), context.clone(), self.signer.identity(), exchange::Limits { max_message_bytes: self.limits.message_bytes }) => result.map_err(|_| p.error(Code::Transport))?,
            _ = stopped(stop) => return Err(p.error(Code::Stopped)),
        };
        channel
            .send(&negotiation::Message::Recover)
            .await
            .map_err(|_| p.error(Code::Transport))?;
        let response = tokio::select! {
            result = channel.receive(self.limits.control_wait) => result.map_err(|_| p.error(Code::Transport))?,
            _ = stopped(stop) => return Err(p.error(Code::Stopped)),
        }.into_message(&context, Role::Buyer).map_err(|_| p.error(Code::Verification))?;
        let negotiation::Message::Accepted { value } = response else {
            return Err(p.error(Code::RecoveryRequired));
        };
        self.negotiation
            .retain_provider_acceptance(value.authorization)
            .await
            .map_err(|_| p.error(Code::Storage))?;
        p.check(stop)?;
        saved = self
            .negotiation
            .publish(
                saved.key().clone(),
                &self.recovery,
                crate::supervisor::unix_ms(),
            )
            .await
            .map_err(|_| p.error(Code::Unconfirmed))?;
        let mut paid = channel
            .into_paid(self.signer.identity())
            .map_err(|_| p.error(Code::Verification))?;
        paid.send(&Message::Status)
            .await
            .map_err(|_| p.error(Code::Transport))?;
        self.finish(paid, saved, p, stop, true, &gate, None).await
    }

    async fn finish(
        &self,
        paid: exchange::Channel,
        saved: SavedPurchase,
        p: &mut Progress,
        stop: &mut watch::Receiver<bool>,
        recovery: bool,
        gate: &Arc<dyn AuthorizationGate>,
        mut observer: Option<StreamSender>,
    ) -> Result<Outcome> {
        let authorization = saved
            .authorization()
            .ok_or_else(|| p.error(Code::Verification))?;
        let terms_key = Digest::new(
            authorization
                .terms
                .digest()
                .map_err(|_| p.error(Code::Verification))?,
        )
        .map_err(|_| p.error(Code::Verification))?;
        let (mut sender, mut receiver) =
            paid.into_duplex().map_err(|_| p.error(Code::Transport))?;
        let public_id = format!("proxy_{}", receiver.session().invocation().as_str());
        let prepared = if observer.is_some() {
            let binding = financial::terms_binding(&authorization.terms)
                .map_err(|_| p.error(Code::Verification))?;
            Some(
                saved
                    .snapshot()
                    .verify_request(&binding, saved.request())
                    .map_err(|_| p.error(Code::Verification))?,
            )
        } else {
            None
        };
        let mut stream = None;
        let mut streamed = false;
        let mut observed = None;
        let mut failed = false;
        // Control steps and provisional events have separate bounds; no history
        // is retained. Generation itself has no total timer.
        let mut controls = 0;
        loop {
            if controls == 8 {
                return Err(p.error(Code::Verification));
            }
            let wait =
                (recovery || observed.is_some() || failed).then_some(self.limits.control_wait);
            let received = tokio::select! {
                result = receiver.receive(wait) => result.map_err(|_| p.error(Code::Transport))?,
                _ = stopped(stop) => {
                    // Only a request to cancel. Losing this send or disconnecting
                    // is not evidence of stopped execution or released funds.
                    let _ = sender.send(&Message::Cancel).await;
                    return Err(p.error(Code::Stopped));
                }
            };
            if !matches!(received.message(), Message::Stream { .. }) {
                controls += 1;
            }
            match received.message() {
                Message::Stream { event }
                    if observer.is_some() && observed.is_none() && !failed =>
                {
                    let observer = observer.as_mut().unwrap();
                    let bytes = observer
                        .encode(event)
                        .map_err(|_| p.error(Code::Verification))?;
                    if stream.is_none() {
                        let created =
                            if authorization.terms.offer.endpoint == ProxyEndpoint::Responses {
                                event.pointer("/response/created_at")
                            } else {
                                event.get("created")
                            }
                            .and_then(Value::as_u64)
                            .ok_or_else(|| p.error(Code::Verification))?;
                        stream = Some(
                            prepared
                                .as_ref()
                                .unwrap()
                                .stream(&public_id, created)
                                .map_err(|_| p.error(Code::Verification))?,
                        );
                    }
                    stream
                        .as_mut()
                        .unwrap()
                        .push(event)
                        .map_err(|_| p.error(Code::Verification))?;
                    streamed = true;
                    tokio::select! {
                        delivered = observer.send(bytes) => { if delivered.is_err() {
                            let _ = sender.send(&Message::Cancel).await;
                            return Err(p.error(Code::Stopped));
                        } },
                        _ = stopped(stop) => {
                            let _ = sender.send(&Message::Cancel).await;
                            return Err(p.error(Code::Stopped));
                        }
                    }
                }
                Message::Result { .. } if observed.is_none() && !failed => {
                    p.stage = Stage::Verifying;
                    let reply = receiver
                        .session()
                        .decode_result(received, saved.snapshot(), saved.request())
                        .map_err(|_| p.error(Code::Verification))?;
                    if let Some(prepared) = &prepared {
                        let created = reply.body[if authorization.terms.offer.endpoint
                            == ProxyEndpoint::Responses
                        {
                            "created_at"
                        } else {
                            "created"
                        }]
                        .as_u64()
                        .ok_or_else(|| p.error(Code::Verification))?;
                        let stream = match stream.take() {
                            Some(stream) => stream,
                            None => prepared
                                .stream(&public_id, created)
                                .map_err(|_| p.error(Code::Verification))?,
                        };
                        stream
                            .verify_final(&reply.body)
                            .map_err(|_| p.error(Code::Verification))?;
                    }
                    observed = Some(reply);
                }
                Message::Failure { .. } if observed.is_none() && !failed => {
                    failed = true;
                }
                Message::State {
                    state:
                        PublicState::Prepared
                        | PublicState::Running
                        | PublicState::OutcomeUnknown
                        | PublicState::CancelRequested,
                } if recovery => {
                    let Message::State { state } = received.message() else {
                        unreachable!()
                    };
                    return Ok(Outcome::Pending {
                        identity: p.identity.clone(),
                        state: *state,
                    });
                }
                Message::State {
                    state: PublicState::AwaitingReceipt,
                } if recovery => {}
                Message::State {
                    state: PublicState::Settled,
                } if recovery => {
                    let settlement = self
                        .recovery
                        .refresh(&authorization, crate::supervisor::unix_ms())
                        .await
                        .map_err(|_| p.error(Code::Unconfirmed))?
                        .ok_or_else(|| p.error(Code::Unconfirmed))?;
                    return Ok(Outcome::Closed {
                        identity: p.identity.clone(),
                        authorization,
                        settlement,
                    });
                }
                Message::Receipt { value } if observed.is_some() => {
                    let reply = observed.take().unwrap();
                    let response = reply.body.clone();
                    p.stage = Stage::Verifying;
                    let verified = self
                        .recovery
                        .verify_receipt(
                            &self.verifier,
                            value.clone(),
                            saved.snapshot().clone(),
                            saved.request().to_vec(),
                            reply,
                            false,
                        )
                        .await
                        .map_err(|_| p.error(Code::Verification))?;
                    gate.retain_verified_output(VerifiedOutput {
                        identity: &p.identity,
                        authorization: &authorization,
                        response: &response,
                        receipt: value,
                    })
                    .await
                    .map_err(|_| p.error(Code::Storage))?;
                    self.recovery
                        .retain_receipt(verified, crate::supervisor::unix_ms())
                        .await
                        .map_err(|_| p.error(Code::Storage))?;
                    let settlement = self
                        .acknowledge(
                            &mut sender,
                            &mut receiver,
                            &authorization,
                            terms_key,
                            p,
                            stop,
                        )
                        .await?;
                    if !matches!(settlement, FinancialOutcome::Paid { .. }) {
                        return Err(p.error(Code::Verification));
                    }
                    return Ok(Outcome::Completed {
                        identity: p.identity.clone(),
                        response,
                        authorization,
                        settlement,
                    });
                }
                Message::Waiver { value } if observed.is_none() && !streamed => {
                    p.stage = Stage::Verifying;
                    self.recovery
                        .approve_waiver(
                            value.clone(),
                            saved.request().to_vec(),
                            None,
                            false,
                            crate::supervisor::unix_ms(),
                        )
                        .await
                        .map_err(|_| p.error(Code::Verification))?;
                    let settlement = self
                        .acknowledge(
                            &mut sender,
                            &mut receiver,
                            &authorization,
                            terms_key,
                            p,
                            stop,
                        )
                        .await?;
                    if !matches!(settlement, FinancialOutcome::Waived { .. }) {
                        return Err(p.error(Code::Verification));
                    }
                    return Ok(Outcome::Closed {
                        identity: p.identity.clone(),
                        authorization,
                        settlement,
                    });
                }
                _ => return Err(p.error(Code::Verification)),
            }
        }
    }
    async fn acknowledge(
        &self,
        sender: &mut exchange::Sender,
        receiver: &mut exchange::Receiver,
        authorization: &ProxySpendAuthorization,
        key: Digest,
        p: &mut Progress,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<FinancialOutcome> {
        p.stage = Stage::Acknowledging;
        let acknowledgment = self
            .recovery
            .sign_approved(&self.signer, key)
            .await
            .map_err(|_| p.error(Code::Storage))?;
        p.check(stop)?;
        sender
            .send(&Message::Acknowledge {
                acknowledgment: acknowledgment.clone(),
            })
            .await
            .map_err(|_| p.error(Code::Transport))?;
        let response = tokio::select! {
            result = receiver.receive(Some(self.limits.control_wait)) => result.map_err(|_| p.error(Code::Transport))?,
            _ = stopped(stop) => return Err(p.error(Code::Stopped)),
        };
        if !matches!(
            response.message(),
            Message::State {
                state: PublicState::Settled
            }
        ) {
            return Err(p.error(Code::Unconfirmed));
        }
        let settlement = self
            .recovery
            .refresh(authorization, crate::supervisor::unix_ms())
            .await
            .map_err(|_| p.error(Code::Unconfirmed))?
            .ok_or_else(|| p.error(Code::Unconfirmed))?;
        let exact = match (&acknowledgment, &settlement) {
            (
                SignedAcknowledgment::Receipt { receipt: expected },
                FinancialOutcome::Paid { receipt },
            ) => expected == receipt,
            (
                SignedAcknowledgment::Waiver { closure: expected },
                FinancialOutcome::Waived { closure },
            ) => expected == closure,
            _ => false,
        };
        if !exact {
            return Err(p.error(Code::Verification));
        }
        Ok(settlement)
    }
}
