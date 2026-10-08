//! Parent-owned, one-attempt JSON and stream execution. This connects durable intent, the
//! protected HTTP broker and a supervised decoder without retrying a POST.
//! Canonical offer/reservation/lease acceptance precedes this API. The returned
//! reply passes protocol and supported schema checks and is retained privately,
//! with independently counted quantities, but not financial authority. This module cannot
//! close holds, sign receipts or release model slots.

use crate::{
    attempts::{self, Digest, Event, FailureSnapshot, Journal, Phase, Record},
    capacity,
    connector::{
        failure::{Code, Execution, Failure, Scope, Stage},
        http::{HttpConnection, WireFormat},
    },
    endpoint::{Adapter, ProtocolReply, Request},
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
    #[error("proxy shared capacity: {0}")]
    Capacity(#[from] capacity::Error),
    #[error("proxy execution configuration is invalid")]
    Configuration,
    #[error("proxy execution storage is at capacity")]
    StorageCapacity,
    #[error("proxy execution storage worker failed")]
    StorageWorker,
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
                Ok(j.begin_dispatch(&key, generation, now_ms())?)
            })
            .await?;
        let active = ready.attach(ticket)?;
        let public_id = format!("proxy_{}", record.invocation.as_str());
        let operation = async {
            if streaming {
                self.perform_stream(&request, active, &public_id, &record, &mut emit)
                    .await
            } else {
                self.perform(&request, active, &public_id, record.created_at_ms / 1000)
                    .await
            }
        };
        let outcome = tokio::select! {
            biased;
            _=cancel.cancelled() => Err(Error::Cancelled),
            result=operation => result,
        };
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
                    Error::Journal(_) | Error::StorageCapacity | Error::StorageWorker
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
    async fn perform_stream<F, Fut>(
        &self,
        request: &Request,
        mut decoder: worker::host::Active,
        public_id: &str,
        record: &Record,
        emit: &mut F,
    ) -> Result<ProtocolReply>
    where
        F: FnMut(serde_json::Value) -> Fut,
        Fut: Future<Output = std::result::Result<(), ()>>,
    {
        let mut response = self
            .connection
            .send(self.adapter.operation(), Some(request.body().to_vec()))
            .await
            .map_err(Error::Upstream)?;
        if response.status != 200 || response.format != WireFormat::Sse {
            return Err(Error::Upstream(Failure::new(
                Code::UpstreamProtocol,
                Scope::Model,
                Stage::ResponseHeaders,
                Execution::Unknown,
            )));
        }
        // Persist delivery intent ONCE before calling any consumer. This is
        // conservative even when the upstream fails before its first text byte.
        self.storage
            .event(&record.invocation, record.attempt, Event::FirstOutput)
            .await?;
        let mut stream =
            crate::endpoint::stream::Stream::new(request, public_id, record.created_at_ms / 1000)?;
        let mut saved_upstream_id = false;
        while let Some(chunk) = response.next_chunk().await.map_err(Error::Upstream)? {
            let mut receive = |frame| {
                let piece = stream.push(frame).map_err(|_| worker::Error::Protocol);
                let future = match piece {
                    Ok(Some(value)) => Ok(Some(emit(value))),
                    Ok(None) => Ok(None),
                    Err(e) => Err(e),
                };
                async move {
                    if let Some(f) = future? {
                        f.await.map_err(|_| worker::Error::Cancelled)?;
                    }
                    Ok(())
                }
            };
            let pushed = decoder.push(&chunk, &mut receive).await;
            drop(receive);
            if !saved_upstream_id {
                if let Some(id) = stream.upstream_id() {
                    self.storage
                        .event(
                            &record.invocation,
                            record.attempt,
                            Event::Accepted(attempts::RemoteId::new(id)?),
                        )
                        .await?;
                    saved_upstream_id = true;
                }
            }
            pushed.map_err(|e| {
                if matches!(e, worker::Error::Cancelled) {
                    Error::Cancelled
                } else {
                    Error::Decoder(e)
                }
            })?;
            if stream.is_done() {
                break;
            }
        }
        // [DONE] is terminal in this profile. A conforming long-lived SSE HTTP
        // connection need not close before result verification can finish.
        drop(response);
        let mut receive = |frame| {
            let piece = stream.push(frame).map_err(|_| worker::Error::Protocol);
            let future = match piece {
                Ok(Some(value)) => Ok(Some(emit(value))),
                Ok(None) => Ok(None),
                Err(e) => Err(e),
            };
            async move {
                if let Some(f) = future? {
                    f.await.map_err(|_| worker::Error::Cancelled)?;
                }
                Ok(())
            }
        };
        decoder
            .finish_stream_frames(&mut receive)
            .await
            .map_err(|e| {
                if matches!(e, worker::Error::Cancelled) {
                    Error::Cancelled
                } else {
                    Error::Decoder(e)
                }
            })?;
        drop(receive);
        let result = stream.finish()?;
        decoder.verify_stream_result(&result).await?;
        request
            .decode_json(result, public_id, record.created_at_ms / 1000)
            .map_err(Error::Endpoint)
    }
    async fn perform(
        &self,
        request: &Request,
        mut decoder: worker::host::Active,
        public_id: &str,
        created: u64,
    ) -> Result<ProtocolReply> {
        let mut response = self
            .connection
            .send(self.adapter.operation(), Some(request.body().to_vec()))
            .await
            .map_err(Error::Upstream)?;
        if response.status != 200 || response.format != WireFormat::Json {
            let mut failure = Failure::new(
                Code::UpstreamProtocol,
                Scope::Model,
                Stage::ResponseHeaders,
                Execution::Unknown,
            );
            failure.upstream_status = Some(response.status);
            return Err(Error::Upstream(failure));
        }
        while let Some(chunk) = response.next_chunk().await.map_err(Error::Upstream)? {
            decoder
                .push(&chunk, |_| async { Err(worker::Error::Protocol) })
                .await?;
        }
        let mut value = None;
        decoder
            .finish(|decoded| {
                let accepted = match decoded {
                    Decoded::Json { value: v } if value.is_none() => {
                        value = Some(v);
                        true
                    }
                    _ => false,
                };
                async move {
                    if accepted {
                        Ok(())
                    } else {
                        Err(worker::Error::Protocol)
                    }
                }
            })
            .await?;
        let value = value.ok_or(Error::Decoder(worker::Error::Protocol))?;
        request
            .decode_json(value, public_id, created)
            .map_err(Error::Endpoint)
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
