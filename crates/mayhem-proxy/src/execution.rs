//! Parent-owned, one-attempt JSON and stream execution. This connects durable intent, the
//! protected HTTP broker and a supervised decoder without retrying a POST.
//! Canonical offer/reservation/lease acceptance precedes this API. The returned
//! reply passes protocol and supported schema checks and is retained privately,
//! with independently counted quantities, but not financial authority. This module cannot
//! close holds or sign receipts. The paid wrapper separately reconciles local
//! capacity from retained terminal output or proven pre-send cancellation.

mod paid;
pub mod probes;
mod timed_reader;
mod transport;
pub use paid::PaidExecutor;

use crate::{
    attempts::{self, Digest, Event, FailureSnapshot, Journal, Phase, Record},
    capacity,
    connector::{
        failure::{Code, Execution, Failure, Scope, Stage},
        http::{HttpConnection, WireFormat},
    },
    endpoint::{Adapter, ProtocolReply, Request},
    financial, health,
    worker::{self, host::Pool, DecodeLimits, Decoded, Init},
};
use std::{
    fmt,
    future::Future,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{watch, Semaphore};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("proxy financial admission is unavailable or invalid")]
    Financial(#[source] crate::Error),
    #[error("proxy shared capacity: {0}")]
    Capacity(#[from] capacity::Error),
    #[error("proxy execution configuration is invalid")]
    Configuration,
    #[error("proxy execution storage is at capacity")]
    StorageCapacity,
    #[error("proxy execution storage worker failed")]
    StorageWorker,
    #[error("proxy local stream reader failed")]
    TransportWorker,
    #[error("proxy execution journal: {0}")]
    Journal(#[from] attempts::Error),
    #[error("proxy execution endpoint: {0}")]
    Endpoint(#[from] crate::endpoint::Error),
    #[error("proxy execution decoder: {0}")]
    Decoder(#[from] worker::Error),
    #[error("{0}")]
    Upstream(Failure),
    #[error("this attempt is already dispatched; recover it without submitting again")]
    RecoveryRequired,
    #[error("this attempt has a retained outcome; use its existing result")]
    ExistingResult,
    #[error("proxy execution was cancelled; upstream execution may continue")]
    Cancelled,
    #[error("proxy execution is bound to a different accepted request or connection")]
    Binding,
}
pub type Result<T> = std::result::Result<T, Error>;

/// Blocking storage calls use an explicit bounded pool. A cancelled async waiter
/// cannot release the slot while the actual fsync is still running.
pub struct Storage {
    journal: Arc<Journal>,
    slots: Arc<Semaphore>,
}
impl Storage {
    pub fn new(journal: Arc<Journal>, workers: usize) -> Result<Self> {
        if workers == 0 || workers > 64 {
            return Err(Error::Configuration);
        }
        Ok(Self {
            journal,
            slots: Arc::new(Semaphore::new(workers)),
        })
    }
    async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&Journal) -> attempts::Result<T> + Send + 'static,
    ) -> Result<T> {
        self.run_checked(move |journal| operation(journal).map_err(Error::Journal))
            .await
    }
    async fn run_checked<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&Journal) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::StorageCapacity)?;
        let journal = self.journal.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            operation(&journal)
        })
        .await
        .map_err(|_| Error::StorageWorker)?
    }
    async fn current(&self, invocation: &Digest) -> Result<Record> {
        let key = invocation.clone();
        self.run(move |j| j.get(&key)?.ok_or(attempts::Error::NotFound))
            .await
    }
    /// Local exact-attempt recovery only. No upstream request or financial action.
    /// The caller must authorize ownership before returning payloads to a buyer.
    pub async fn recover(&self, invocation: &Digest, attempt: u64) -> Result<attempts::Recovery> {
        let key = invocation.clone();
        self.run(move |j| j.recover(&key, attempt)).await
    }
    async fn event(&self, invocation: &Digest, attempt: u64, event: Event) -> Result<Record> {
        let key = invocation.clone();
        self.run(move |j| {
            let r = j.get(&key)?.ok_or(attempts::Error::NotFound)?;
            if r.attempt != attempt {
                return Err(attempts::Error::Stale);
            }
            j.advance(&key, r.generation, event, now_ms())
        })
        .await
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// Cancellation is invocation-local. It does not call upstream global stop and
/// cannot establish that a remote job ended or that a reservation is releasable.
#[derive(Clone)]
pub struct Cancellation {
    sender: watch::Sender<bool>,
}
impl Default for Cancellation {
    fn default() -> Self {
        Self {
            sender: watch::channel(false).0,
        }
    }
}
impl Cancellation {
    pub fn cancel(&self) {
        self.sender.send_replace(true);
    }
    pub fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }
    async fn cancelled(&self) {
        let mut receiver = self.sender.subscribe();
        loop {
            if *receiver.borrow() {
                return;
            }
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
}

pub struct Executor {
    connection: Arc<HttpConnection>,
    adapter: Arc<Adapter>,
    pool: Arc<Pool>,
    storage: Arc<Storage>,
    capacity: Option<(Arc<capacity::Authority>, Digest)>,
    financial: Option<Arc<financial::Client>>,
    observations: Option<(health::Monitor, Digest)>,
    tokenizer: Option<Arc<health::native::Source>>,
}
impl Executor {
    pub fn new(
        connection: Arc<HttpConnection>,
        adapter: Arc<Adapter>,
        pool: Arc<Pool>,
        storage: Arc<Storage>,
    ) -> Result<Self> {
        let limits = adapter.limits();
        if limits.request_bytes > connection.limits().max_request_bytes
            || limits.response_bytes > connection.limits().max_response_bytes
        {
            return Err(Error::Configuration);
        }
        Ok(Self {
            connection,
            adapter,
            pool,
            storage,
            capacity: None,
            financial: None,
            observations: None,
            tokenizer: None,
        })
    }

    /// Configure shared local admission for this exact route. Both JSON and stream
    /// paths then require the accepted lease immediately before durable dispatch.
    /// This does not itself acquire capacity, authorize money, or reconcile outcomes.
    /// The unrestricted constructor remains for lower-level protocol verification;
    /// the public paid controller must use this configured authority.
    pub fn with_capacity(mut self, authority: Arc<capacity::Authority>, route: Digest) -> Self {
        self.capacity = Some((authority, route));
        self
    }

    /// Passive evidence for this registered route. This neither admits a request
    /// nor authorizes a recovery probe; canonical finance and durable capacity
    /// remain the paid controller's responsibility. No additional POST is made.
    pub fn with_observations(mut self, monitor: health::Monitor, route: Digest) -> Result<Self> {
        monitor.snapshot(&route).map_err(|_| Error::Configuration)?;
        self.observations = Some((monitor, route));
        Ok(self)
    }
    pub fn with_tokenizer(mut self, source: Arc<health::native::Source>) -> Result<Self> {
        if self.adapter.endpoint() == mayhem_proto::proxy::ProxyEndpoint::Decisions
            || !source.matches(self.connection.fingerprint(), self.adapter.recipe_hash())
        {
            return Err(Error::Configuration);
        }
        self.tokenizer = Some(source);
        Ok(self)
    }

    /// `invocation` must already name a durably accepted Core reservation/offer
    /// and capacity lease. This checks immutable bindings, not canonical authority.
    /// No user-provided arbitrary Binding is accepted/created by this method.
    pub async fn execute_json(
        &self,
        invocation: &Digest,
        bytes: &[u8],
        cancel: &Cancellation,
    ) -> Result<UnsettledReply> {
        self.execute(invocation, bytes, cancel, false, |_| async { Ok(()) })
            .await
    }
    /// Provisional deltas only. The caller owns final public framing after result
    /// retention and financial reconciliation; no success marker is emitted here.
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
        self.execute(invocation, bytes, cancel, true, emit).await
    }
    async fn execute<F, Fut>(
        &self,
        invocation: &Digest,
        bytes: &[u8],
        cancel: &Cancellation,
        streaming: bool,
        mut emit: F,
    ) -> Result<UnsettledReply>
    where
        F: FnMut(serde_json::Value) -> Fut,
        Fut: Future<Output = std::result::Result<(), ()>>,
    {
        if let (Some((_, observed)), Some((_, admitted))) = (&self.observations, &self.capacity) {
            if observed != admitted {
                return Err(Error::Configuration);
            }
        }
        let request = if streaming {
            self.adapter.prepare_stream(bytes)?
        } else {
            self.adapter.prepare_json(bytes)?
        };
        let record = self.storage.current(invocation).await?;
        if !request.matches_binding(&record.binding)
            || request.metering_policy_hash() != record.binding.metering_policy
            || &record.binding.connection_digest != self.connection.fingerprint()
            || record.binding.connection_revision != self.connection.revision()
        {
            return Err(Error::Binding);
        }
        match record.phase {
            Phase::Dispatched => return Err(Error::RecoveryRequired),
            Phase::Resolved | Phase::Closed => return Err(Error::ExistingResult),
            Phase::Prepared => (),
        }
        let key = invocation.clone();
        let attempt = record.attempt;
        // The exact original recipe/contract/offer must survive reconfiguration.
        // This is one bounded key read, never a current-catalog or receipt scan.
        let accepted = self
            .storage
            .run(move |j| j.accepted_snapshot(&key, attempt))
            .await?;
        if accepted.is_none() {
            return Err(Error::Binding);
        }
        if record.cancellation_requested || cancel.is_cancelled() {
            self.storage
                .event(invocation, record.attempt, Event::CancelRequested)
                .await?;
            return Err(Error::Cancelled);
        }
        let limits = self.adapter.limits();
        let init = Init::new(
            &record,
            if streaming {
                WireFormat::Sse
            } else {
                WireFormat::Json
            },
            self.connection.error_profile(),
            DecodeLimits {
                max_total_bytes: limits.response_bytes,
                max_event_bytes: limits.response_bytes,
            },
        )?
        .with_semantics(request.semantic_policy())?;
        // Reserve decoder resources and verify the exact worker release BEFORE
        // the commit-before-send fence. Startup failure does not dispatch anything.
        let ready = tokio::select! {
            result=async {
                self.pool.start(init).await?.configure_semantics(request.semantic_policy()).await
            } => result?,
            _=cancel.cancelled() => {
                self.storage.event(invocation,record.attempt,Event::CancelRequested).await?;
                return Err(Error::Cancelled);
            }
        };
        if cancel.is_cancelled() {
            drop(ready);
            self.storage
                .event(invocation, record.attempt, Event::CancelRequested)
                .await?;
            return Err(Error::Cancelled);
        }
        let key = invocation.clone();
        let generation = record.generation;
        let attempt = record.attempt;
        let owned_body = bytes.to_vec();
        let payload_key = key.clone();
        self.storage
            .run(move |j| {
                j.retain_request(&payload_key, attempt, &owned_body, limits.response_bytes)
            })
            .await?;
        // Freshness is checked after decoder startup and input retention, immediately
        // before dispatch. Stored accepted terms are never themselves a fresh proof.
        let observation = if let Some(client) = &self.financial {
            let k = key.clone();
            let retained = self
                .storage
                .run(move |j| j.financial_acceptance(&k, attempt))
                .await?
                .ok_or(Error::Binding)?;
            let observed = tokio::select! {
                result=client.observe(&retained.accepted().authorization) => result.map_err(Error::Financial)?,
                _=cancel.cancelled() => {
                    self.storage.event(invocation, record.attempt, Event::CancelRequested).await?;
                    return Err(Error::Cancelled);
                },
            };
            if observed.initial_binding().map_err(Error::Financial)? != record.binding {
                return Err(Error::Binding);
            }
            Some(observed)
        } else {
            None
        };
        let capacity = self.capacity.clone();
        let ticket = self
            .storage
            .run_checked(move |j| {
                // Re-read one exact record after local decoder startup. Never mark
                // capacity sent for a duplicate/stale/replaced attempt.
                let current = j.get(&key)?.ok_or(attempts::Error::NotFound)?;
                if current.generation != generation || current.attempt != attempt {
                    return Err(attempts::Error::Stale.into());
                }
                if current.phase != Phase::Prepared {
                    return Err(Error::RecoveryRequired);
                }
                if current.cancellation_requested {
                    return Err(Error::Cancelled);
                }
                if let Some(observation) = &observation {
                    if observation.initial_binding().map_err(Error::Financial)? != current.binding {
                        return Err(Error::Binding);
                    }
                    j.retain_financial_acceptance(&key, attempt, observation)?;
                    j.reserve_outcome(&key, attempt)?;
                }
                if let Some((authority, route)) = capacity {
                    authority.dispatch_accepted(
                        &current.binding.capacity_lease,
                        &capacity::Work {
                            invocation: key.clone(),
                            request_hash: current.binding.request_hash.clone(),
                        },
                        &route,
                    )?;
                }
                // If this write fails after capacity committed, the lease remains
                // occupied/uncertain. No rollback, expiry or destructor frees it.
                // Capacity fsync may have taken time. Do not turn an expired
                // financial observation into permission merely because a slot exists.
                if let Some(observation) = &observation {
                    if observation.initial_binding().map_err(Error::Financial)? != current.binding {
                        return Err(Error::Binding);
                    }
                }
                Ok(j.begin_dispatch(&key, generation, now_ms())?)
            })
            .await?;
        let active = ready.attach(ticket)?;
        let public_id = format!("proxy_{}", record.invocation.as_str());
        // A broken observer must not turn already admitted work into a failure.
        // Its admission view remains fail-closed independently. No per-token
        // locks/storage reads or upstream-reported token-rate certification.
        let mut sample = self
            .observations
            .as_ref()
            .and_then(|(monitor, route)| monitor.observe_request(route, request.health_class).ok());
        if let (Some(sample), Some(source)) = (&mut sample, &self.tokenizer) {
            sample.use_native(source)
        }
        let operation = async {
            if streaming {
                self.transport()
                    .perform_stream(
                        &request,
                        active,
                        &public_id,
                        &transport::Delivery::Customer {
                            storage: &self.storage,
                            record: &record,
                        },
                        &mut emit,
                        &mut sample,
                    )
                    .await
            } else {
                self.transport()
                    .perform(
                        &request,
                        active,
                        &public_id,
                        record.created_at_ms / 1000,
                        &mut sample,
                    )
                    .await
            }
        };
        let outcome = tokio::select! {
            biased;
            _=cancel.cancelled() => Err(Error::Cancelled),
            result=operation => result,
        };
        if let Some(sample) = sample {
            match &outcome {
                Ok(reply) => sample.success(reply.reported_usage.as_ref().map(|u| u.output_tokens)),
                Err(Error::Upstream(f) | Error::Decoder(worker::Error::Upstream(f))) => {
                    sample.failure(f.clone())
                }
                Err(Error::Endpoint(crate::endpoint::Error::Protocol)) => {
                    sample.failure(Failure::new(
                        Code::UpstreamProtocol,
                        Scope::Model,
                        Stage::ResponseBody,
                        Execution::Unknown,
                    ))
                }
                // Cancellation, consumer backpressure, decoder containment and
                // local storage failures do not prove an upstream health fault.
                _ => drop(sample),
            }
        }
        // This is a handful of state writes per attempt, never per-token work or
        // a scan of receipts. On future drop the durable Dispatched record remains.
        match outcome {
            Ok(reply) => {
                let key = invocation.clone();
                let attempt = record.attempt;
                // Retain the complete validated result before exposing a final
                // success. Even a late cancellation needs this recovery evidence.
                let owned = self
                    .storage
                    .run(move |j| j.retain_result(&key, attempt, &reply, now_ms()))
                    .await?;
                // Check cancellation written by another controller while in flight.
                let current = self.storage.current(invocation).await?;
                if current.attempt != record.attempt || current.phase != Phase::Dispatched {
                    return Err(Error::Binding);
                }
                if cancel.is_cancelled() || current.cancellation_requested {
                    self.storage
                        .event(invocation, record.attempt, Event::CancelRequested)
                        .await?;
                    return Err(Error::Cancelled);
                }
                Ok(UnsettledReply {
                    attempt: current,
                    reply: owned.reply,
                    result_digest: owned.digest,
                })
            }
            Err(Error::Cancelled) => {
                self.storage
                    .event(invocation, record.attempt, Event::CancelRequested)
                    .await?;
                Err(Error::Cancelled)
            }
            Err(error) => {
                // Parent storage failure is not evidence against the upstream.
                // Retain the uncertain attempt; a failing journal cannot safely
                // manufacture a provider-fault observation or release anything.
                if matches!(
                    &error,
                    Error::Journal(_)
                        | Error::StorageCapacity
                        | Error::StorageWorker
                        | Error::TransportWorker
                ) {
                    return Err(error);
                }
                let failure = match &error {
                    Error::Upstream(f) => f.clone(),
                    Error::Decoder(worker::Error::Upstream(f)) => f.clone(),
                    _ => Failure::new(
                        Code::UpstreamProtocol,
                        Scope::Model,
                        Stage::ResponseBody,
                        Execution::Unknown,
                    ),
                };
                self.storage
                    .event(
                        invocation,
                        record.attempt,
                        Event::Failure(FailureSnapshot::from(&failure)),
                    )
                    .await?;
                Err(error)
            }
        }
    }
    fn transport(&self) -> transport::Transport<'_> {
        transport::Transport {
            connection: &self.connection,
            adapter: &self.adapter,
        }
    }
}

// This callback has a normalized upstream frame, not a worker IPC envelope.
// Preserve that provenance so malformed model output is distinguishable from
// a local worker crash/containment failure without inspecting error text.
fn stream_frame_error(error: crate::endpoint::Error) -> worker::Error {
    match error {
        crate::endpoint::Error::Protocol => worker::Error::Upstream(Failure::new(
            Code::UpstreamProtocol,
            Scope::Model,
            Stage::ResponseBody,
            Execution::Unknown,
        )),
        crate::endpoint::Error::Request(failure) => worker::Error::Upstream(failure),
        _ => worker::Error::Protocol,
    }
}

/// No automatic settlement or buyer delivery. The integration must validate
/// remaining endpoint capabilities, verify accepted rates and reconcile accepted
/// financial/capacity state before closing the journal. The result is retained.
pub struct UnsettledReply {
    pub attempt: Record,
    pub reply: ProtocolReply,
    /// Private retained result commitment; not a settlement receipt.
    pub result_digest: Digest,
}
impl fmt::Debug for UnsettledReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnsettledReply")
            .field("attempt", &self.attempt.attempt)
            .finish_non_exhaustive()
    }
}
