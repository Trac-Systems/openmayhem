//! Owned proxy session orchestration. This is called by a trusted dispatcher with
//! an authenticated negotiation channel, never by deserializing a claimed buyer.
//! No public startup, native provider change or implicit financial waiver.
mod connection;
pub mod maintenance;
mod queue;

use crate::{
    attempts::{Digest, Identity, Journal},
    exchange,
    execution::{self, Cancellation, Executor, PaidExecutor, Storage},
    financial::{
        self,
        provider::{ProviderNegotiation, Runtime},
    },
    negotiation::{
        self,
        provider::{Controller as Proposals, Limits as ProposalLimits},
    },
    signing::Authority,
    worker::host::Pool,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{watch, OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid proxy serving configuration or session")]
    Configuration,
    #[error("proxy session control is at capacity; recover the same request")]
    Busy,
    #[error("proxy session transport failed")]
    Transport(#[from] exchange::Error),
    #[error("proxy session controller failed")]
    Control(#[from] crate::Error),
    #[error("proxy execution or recovery failed")]
    Execution(#[from] execution::Error),
    #[error("proxy session owner failed")]
    Task,
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy)]
pub struct Limits {
    pub sessions: usize,
    pub per_buyer: usize,
    pub outbound_messages: usize,
    pub outbound_bytes: usize,
    /// Negotiation and idle control waits only. Never bounds model generation.
    pub control_wait: Duration,
    pub proposals: ProposalLimits,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Settled,
    Disconnected,
    ControlIdle,
    Refused,
}
struct State {
    connections: BTreeMap<Digest, Digest>,
    jobs: BTreeMap<Digest, Cancellation>,
}
struct Inner {
    identity: Identity,
    proposals: Proposals,
    executor: Arc<PaidExecutor>,
    signer: Arc<Authority>,
    limits: Limits,
    permits: Arc<Semaphore>,
    maintenance: Arc<Semaphore>,
    state: Mutex<State>,
}
#[derive(Clone)]
pub struct Controller {
    inner: Arc<Inner>,
}
/// Dropping this handle asks the connection to close. The owner still retains
/// in-flight execution long enough to save its result/uncertainty.
pub struct Handle {
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<Result<End>>>,
}
impl Handle {
    pub fn disconnect(&self) {
        self.stop.send_replace(true);
    }
    pub async fn wait(mut self) -> Result<End> {
        self.task
            .take()
            .ok_or(Error::Task)?
            .await
            .map_err(|_| Error::Task)?
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}
struct ConnectionGuard {
    owner: Arc<Inner>,
    invocation: Digest,
    _permit: OwnedSemaphorePermit,
    registered: bool,
}
impl ConnectionGuard {
    fn unregister(&mut self) {
        if self.registered {
            if let Ok(mut state) = self.owner.state.lock() {
                state.connections.remove(&self.invocation);
            }
            self.registered = false;
        }
    }
}
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.unregister();
    }
}
struct JobGuard {
    owner: Arc<Inner>,
    invocation: Digest,
}
impl Drop for JobGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.owner.state.lock() {
            state.jobs.remove(&self.invocation);
        }
    }
}
impl Controller {
    /// All components come from the same owned runtime/journal/wallet. The raw
    /// upstream adapter never receives a signer or chooses payment authority.
    pub fn new(
        runtime: Arc<Runtime>,
        journal: Arc<Journal>,
        signer: Arc<Authority>,
        financial: Arc<financial::Client>,
        pool: Arc<Pool>,
        limits: Limits,
    ) -> Result<Self> {
        if limits.sessions == 0
            || limits.sessions > 4096
            || limits.per_buyer == 0
            || limits.per_buyer > limits.sessions
            || limits.outbound_messages == 0
            || limits.outbound_messages > 4096
            || limits.outbound_bytes == 0
            || limits.outbound_bytes > 256 * 1024 * 1024
            || limits.control_wait.is_zero()
            || limits.control_wait > Duration::from_secs(86_400)
            || signer.identity() != runtime.capacity.identity()
            || journal.identity().map_err(execution::Error::from)? != *runtime.capacity.identity()
        {
            return Err(Error::Configuration);
        }
        let negotiation = Arc::new(ProviderNegotiation::new(
            journal.clone(),
            signer.clone(),
            limits.proposals.storage_operations,
        )?);
        let proposals = Proposals::new(
            runtime.clone(),
            negotiation,
            financial.clone(),
            limits.proposals,
        )?;
        let executor = Executor::new(
            runtime.connection.clone(),
            runtime.adapter.clone(),
            pool,
            Arc::new(Storage::new(journal, limits.proposals.storage_operations)?),
        )?;
        let executor = Arc::new(PaidExecutor::new(
            executor,
            financial,
            runtime.capacity.clone(),
            runtime.route.clone(),
        )?);
        Ok(Self {
            inner: Arc::new(Inner {
                identity: runtime.capacity.identity().clone(),
                proposals,
                executor,
                signer,
                limits,
                permits: Arc::new(Semaphore::new(limits.sessions)),
                maintenance: Arc::new(Semaphore::new(1)),
                state: Mutex::new(State {
                    connections: BTreeMap::new(),
                    jobs: BTreeMap::new(),
                }),
            }),
        })
    }
    pub fn proposals(&self) -> &Proposals {
        &self.inner.proposals
    }
    pub fn start(&self, channel: negotiation::Channel) -> Result<Handle> {
        channel.provider_for(&self.inner.identity)?;
        let context = channel.context();
        let invocation = context.invocation()?;
        let permit = self
            .inner
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        {
            let mut state = self.inner.state.lock().map_err(|_| Error::Task)?;
            if state.connections.contains_key(&invocation)
                || state
                    .connections
                    .values()
                    .filter(|v| *v == &context.buyer)
                    .count()
                    >= self.inner.limits.per_buyer
            {
                return Err(Error::Busy);
            }
            state
                .connections
                .insert(invocation.clone(), context.buyer.clone());
        }
        let guard = ConnectionGuard {
            owner: self.inner.clone(),
            invocation,
            _permit: permit,
            registered: true,
        };
        let (stop, stopped) = watch::channel(false);
        let owner = self.inner.clone();
        let task =
            tokio::spawn(async move { connection::run(owner, channel, guard, stopped).await });
        Ok(Handle {
            stop,
            task: Some(task),
        })
    }
}
impl Inner {
    fn job(self: &Arc<Self>, invocation: &Digest) -> Result<(JobGuard, Cancellation)> {
        let mut state = self.state.lock().map_err(|_| Error::Task)?;
        if state.jobs.contains_key(invocation) {
            return Err(Error::Busy);
        }
        let cancellation = Cancellation::default();
        state.jobs.insert(invocation.clone(), cancellation.clone());
        Ok((
            JobGuard {
                owner: self.clone(),
                invocation: invocation.clone(),
            },
            cancellation,
        ))
    }
    fn cancellation(&self, invocation: &Digest) -> Result<Option<Cancellation>> {
        Ok(self
            .state
            .lock()
            .map_err(|_| Error::Task)?
            .jobs
            .get(invocation)
            .cloned())
    }
}
