//! Parent supervision. Process and worst-case IPC byte permits remain with the
//! supervisor until the child is reaped, including cancellation/drop/start failure.
//! These are local decoder resources, NOT model capacity or financial closure.

use super::*;
use crate::attempts::{DispatchTicket, Phase};
use std::{
    future::Future,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    process::Command,
    sync::{watch, OwnedSemaphorePermit, Semaphore},
};
mod tokenizer;
pub(crate) use tokenizer::Tokenizer;

#[cfg(windows)]
mod windows;
#[cfg(not(windows))]
use tokio::process::{Child as PlatformChild, ChildStdin, ChildStdout};
#[cfg(windows)]
use windows::{Child as PlatformChild, ChildStdin, ChildStdout};

#[derive(Clone, Copy)]
enum BundledMode {
    Decoder,
    Tokenizer,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolLimits {
    pub max_children: usize,
    /// Bounds reserved raw/IPC buffers, not a claim to bound total process RSS.
    pub max_buffer_bytes: usize,
    pub startup_timeout: Duration,
    /// Local parser/IPC response wait only. Never a total generation deadline.
    /// Consumer backpressure is excluded from this clock.
    pub processing_timeout: Duration,
}
impl PoolLimits {
    fn validate(self) -> Result<()> {
        config(
            (1..=128).contains(&self.max_children)
                && self.max_buffer_bytes >= CHUNK_BYTES
                && !self.startup_timeout.is_zero()
                && !self.processing_timeout.is_zero(),
        )?;
        units(self.max_buffer_bytes)?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct Pool {
    program: PathBuf,
    workdir: PathBuf,
    limits: PoolLimits,
    children: Arc<Semaphore>,
    buffers: Arc<Semaphore>,
    #[cfg(windows)]
    windows: windows::Launcher,
}

fn config(ok: bool) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Configuration)
    }
}
fn units(bytes: usize) -> Result<u32> {
    u32::try_from(bytes.div_ceil(CHUNK_BYTES)).map_err(|_| Error::Configuration)
}

fn decoder_buffer_bytes(limits: DecodeLimits, semantic_policy: bool) -> Result<usize> {
    limits.validate()?;
    limits
        .max_total_bytes
        .checked_add(
            limits
                .ipc_bytes()
                .checked_mul(2)
                .ok_or(Error::Configuration)?,
        )
        .and_then(|n| n.checked_add(CHUNK_BYTES * 2))
        .and_then(|n| {
            n.checked_add(if semantic_policy {
                crate::semantics::MAX_POLICY_BYTES * 2
            } else {
                0
            })
        })
        .ok_or(Error::Configuration)
}

impl Pool {
    /// Admission must never promise a response size that cannot fit even one
    /// isolated verifier. Concurrent use may wait for recovery; an impossible
    /// per-result allocation must instead fail at configuration time.
    pub(crate) fn require_received_limit(&self, max_bytes: usize) -> Result<()> {
        let required = decoder_buffer_bytes(
            DecodeLimits {
                max_total_bytes: max_bytes,
                max_event_bytes: max_bytes,
            },
            true,
        )?;
        config(units(required)? <= units(self.limits.max_buffer_bytes)?)
    }

    /// Independently check a buyer-received terminal result. This is local-only
    /// verification, not dispatch authority: no journal ticket, upstream I/O,
    /// wallet or financial operation is created. Keep untrusted schema/regex
    /// execution in the same bounded/reaped worker used for provider decoding.
    pub(crate) async fn verify_received(
        &self,
        invocation: &Digest,
        attempt: u64,
        binding: &Binding,
        policy: &crate::semantics::Policy,
        body: &serde_json::Value,
        max_bytes: usize,
    ) -> Result<()> {
        if policy.request_hash != binding.request_hash || policy.endpoint != binding.endpoint {
            return Err(Error::Identity);
        }
        // This is an already received terminal body, not an open upstream
        // stream. Enforce the caller's response ceiling, then reserve decoder
        // buffers for the exact encoded input. Reserving the ceiling here can
        // reject even a tiny answer when its worst-case IPC expansion exceeds
        // the pool, permanently preventing receipt recovery.
        let bytes = json(body, max_bytes)?;
        let received_bytes = bytes.len();
        let init = Init {
            abi: ABI,
            release: RELEASE.into(),
            session: Session {
                invocation: invocation.clone(),
                attempt,
                binding_hash: binding_hash(binding)?,
            },
            format: WireFormat::Json,
            error_profile: ErrorProfile::HttpStatus,
            limits: DecodeLimits {
                max_total_bytes: received_bytes,
                max_event_bytes: received_bytes,
            },
            semantic_policy: Some(policy.digest().map_err(Error::Upstream)?),
        };
        let mut prepared = self.start(init).await?.configure_semantics(policy).await?;
        for chunk in bytes.chunks(CHUNK_BYTES) {
            prepared
                .io
                .exchange(CHUNK, chunk, |_| async { Err(Error::Protocol) })
                .await?;
        }
        let mut seen = false;
        prepared
            .io
            .exchange(FINISH, &[], |decoded| {
                let valid =
                    matches!(decoded, Decoded::Json { ref value } if value == body) && !seen;
                seen = true;
                async move {
                    if valid {
                        Ok(())
                    } else {
                        Err(Error::Protocol)
                    }
                }
            })
            .await?;
        if !seen {
            return Err(Error::Protocol);
        }
        prepared.stop().await
    }

    /// Only the trusted local launcher chooses this bundled executable and private
    /// working directory (empty on Unix; a bounded image journal on Windows).
    /// Neither is accepted in a public request/recipe.
    /// Package signature verification belongs to the Core installer; this is not
    /// a distribution/install API or an arbitrary executable plugin mechanism.
    pub fn new(
        program: impl AsRef<Path>,
        workdir: impl AsRef<Path>,
        limits: PoolLimits,
    ) -> Result<Self> {
        limits.validate()?;
        let program = program.as_ref();
        let workdir = workdir.as_ref();
        config(program.is_absolute() && workdir.is_absolute())?;
        let file = std::fs::symlink_metadata(program).map_err(|_| Error::Configuration)?;
        let dir = std::fs::symlink_metadata(workdir).map_err(|_| Error::Configuration)?;
        config(file.is_file() && dir.is_dir())?;
        #[cfg(not(windows))]
        config(
            std::fs::read_dir(workdir)
                .map_err(|_| Error::Configuration)?
                .next()
                .is_none(),
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let uid = rustix::process::geteuid().as_raw();
            config(
                (file.uid() == uid || file.uid() == 0)
                    && file.mode() & 0o022 == 0
                    && file.mode() & 0o111 != 0
                    && dir.uid() == uid
                    && dir.mode() & 0o077 == 0,
            )?;
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::Configuration);
        #[cfg(windows)]
        let windows = windows::Launcher::new(program, workdir).map_err(|_| Error::Configuration)?;
        #[allow(unreachable_code)]
        Ok(Self {
            program: program.into(),
            workdir: workdir.into(),
            limits,
            children: Arc::new(Semaphore::new(limits.max_children)),
            buffers: Arc::new(Semaphore::new(units(limits.max_buffer_bytes)? as usize)),
            #[cfg(windows)]
            windows,
        })
    }

    /// Reuse the exact pinned executable while assigning independent process
    /// and IPC budgets. No new image staging or executable/path lookup occurs.
    pub fn with_independent_limits(&self, limits: PoolLimits) -> Result<Self> {
        limits.validate()?;
        let mut pool = self.clone();
        pool.limits = limits;
        pool.children = Arc::new(Semaphore::new(limits.max_children));
        pool.buffers = Arc::new(Semaphore::new(units(limits.max_buffer_bytes)? as usize));
        Ok(pool)
    }

    fn spawn_bundled(&self, mode: BundledMode) -> Result<PlatformChild> {
        #[cfg(windows)]
        return windows::spawn(&self.windows, mode);
        #[cfg(not(windows))]
        {
            let mut command = Command::new(&self.program);
            command
                .arg(match mode {
                    BundledMode::Decoder => "--stdio-v1",
                    BundledMode::Tokenizer => "--tokenizer-stdio-v2",
                })
                .env_clear()
                .current_dir(&self.workdir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            #[cfg(unix)]
            command.process_group(0);
            command.spawn().map_err(|_| Error::Start)
        }
    }

    /// Spawn before dispatch when practical. No upstream call occurs here. A
    /// prepared session cannot feed model bytes until a matching ticket is consumed.
    pub async fn start(&self, init: Init) -> Result<Prepared> {
        init.validate()?;
        let children = self
            .children
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Capacity)?;
        let bytes = decoder_buffer_bytes(init.limits, init.semantic_policy.is_some())?;
        let buffers = self
            .buffers
            .clone()
            .try_acquire_many_owned(units(bytes)?)
            .map_err(|_| Error::Capacity)?;
        let mut child = self.spawn_bundled(BundledMode::Decoder)?;
        let input = child.stdin.take().ok_or(Error::Start)?;
        let output = child.stdout.take().ok_or(Error::Start)?;
        let (stop, mut stop_rx) = watch::channel(false);
        let (exit, exit_rx) = watch::channel(None);
        let resources = Arc::new((children, buffers));
        let supervised_resources = resources.clone();
        // Both ends retain resources: a reaped child cannot release buffer quota
        // while the parent is still draining output or a consumer is blocked.
        // Fixed one supervisor per acquired process permit, never a task/token.
        tokio::spawn(async move {
            let status = tokio::select! {
                result = child.wait() => result,
                _ = stop_rx.changed() => {
                    let _ = child.start_kill();
                    child.wait().await
                }
            };
            drop(supervised_resources);
            exit.send_replace(Some(status.is_ok_and(|s| s.success())));
        });
        let mut io = Worker {
            input: Some(input),
            output,
            stop,
            exit: exit_rx,
            session: init.session.clone(),
            limits: init.limits,
            timeout: self.limits.processing_timeout,
            sequence: 0,
            poisoned: false,
            finished: false,
            resources: Some(resources),
            format: init.format,
            received_event_bytes: 0,
            semantic_policy: init.semantic_policy.clone(),
            semantics_ready: false,
            frames_ended: false,
        };
        let hello = json(&init, CONTROL_BYTES)?;
        let handshake = async {
            send(io.input.as_mut().ok_or(Error::Stopped)?, HELLO, &hello).await?;
            let packet = receive(&mut io.output, CONTROL_BYTES).await?;
            if packet.kind != READY {
                return Err(Error::Protocol);
            }
            let ready: Ready =
                serde_json::from_slice(&packet.bytes).map_err(|_| Error::Protocol)?;
            if ready.abi != ABI || ready.release != RELEASE || ready.session != init.session {
                return Err(Error::Identity);
            }
            Ok(())
        };
        tokio::time::timeout(self.limits.startup_timeout, handshake)
            .await
            .map_err(|_| Error::ProcessingTimeout)??;
        Ok(Prepared { io })
    }
}

pub struct Prepared {
    io: Worker,
}
impl Prepared {
    pub(crate) fn attach_job(self, lease: &crate::attempts::jobs::Lease) -> Result<Active> {
        if !self.io.semantics_ready || Session::from_record(lease.record())? != self.io.session {
            return Err(Error::Identity);
        }
        Ok(Active { io: self.io })
    }

    pub(crate) fn attach_probe(self, ticket: crate::capacity::probes::Dispatch) -> Result<Active> {
        if self.io.semantic_policy.is_some() && !self.io.semantics_ready {
            return Err(Error::Configuration);
        }
        if ticket.probe().phase != crate::capacity::probes::ProbePhase::Dispatched
            || Session::from_probe(ticket.probe())? != self.io.session
        {
            return Err(Error::Identity);
        }
        Ok(Active { io: self.io })
    }
    pub async fn configure_semantics(mut self, policy: &crate::semantics::Policy) -> Result<Self> {
        if self.io.semantics_ready
            || self.io.semantic_policy.as_ref() != Some(&policy.digest().map_err(Error::Upstream)?)
        {
            return Err(Error::Identity);
        }
        let bytes = policy.bytes().map_err(Error::Upstream)?;
        for chunk in bytes.chunks(CHUNK_BYTES) {
            self.io
                .exchange(POLICY_CHUNK, chunk, |_| async { Err(Error::Protocol) })
                .await?;
        }
        self.io
            .exchange(POLICY_END, &[], |_| async { Err(Error::Protocol) })
            .await?;
        self.io.semantics_ready = true;
        Ok(self)
    }
    /// Consumes the journal-issued ticket exactly once. Full Core admission and
    /// connection/recipe/request validation still precede transport dispatch.
    pub fn attach(self, ticket: DispatchTicket) -> Result<Active> {
        if self.io.semantic_policy.is_some() && !self.io.semantics_ready {
            return Err(Error::Configuration);
        }
        let record = ticket.record();
        if record.phase != Phase::Dispatched || Session::from_record(record)? != self.io.session {
            return Err(Error::Identity);
        }
        Ok(Active { io: self.io })
    }
    pub fn cancellation(&self) -> Cancellation {
        self.io.cancellation()
    }
    pub async fn stop(self) -> Result<()> {
        self.io.stop().await
    }
}

#[derive(Clone)]
pub struct Cancellation {
    stop: watch::Sender<bool>,
}
impl Cancellation {
    pub fn cancel(&self) {
        self.stop.send_replace(true);
    }
}

async fn cancelled(receiver: &mut watch::Receiver<bool>) {
    loop {
        if *receiver.borrow() {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

pub struct Active {
    io: Worker,
}
impl Active {
    pub fn cancellation(&self) -> Cancellation {
        self.io.cancellation()
    }
    pub async fn finish_stream_frames<F, Fut>(&mut self, emit: F) -> Result<()>
    where
        F: FnMut(Decoded) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        if !matches!(self.io.format, WireFormat::Sse | WireFormat::Ndjson)
            || !self.io.semantics_ready
            || self.io.frames_ended
        {
            return Err(Error::Configuration);
        }
        self.io.exchange(FRAME_END, &[], emit).await?;
        self.io.frames_ended = true;
        Ok(())
    }
    pub async fn verify_stream_result(&mut self, result: &serde_json::Value) -> Result<()> {
        if !self.io.frames_ended || !self.io.semantics_ready {
            return Err(Error::Configuration);
        }
        let bytes = json(result, self.io.limits.max_event_bytes)?;
        for chunk in bytes.chunks(CHUNK_BYTES) {
            self.io
                .exchange(VERIFY_CHUNK, chunk, |_| async { Err(Error::Protocol) })
                .await?;
        }
        self.io
            .exchange(VERIFY_END, &[], |_| async { Err(Error::Protocol) })
            .await
    }
    pub async fn push<F, Fut>(&mut self, bytes: &[u8], emit: F) -> Result<()>
    where
        F: FnMut(Decoded) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        if bytes.is_empty() || bytes.len() > CHUNK_BYTES {
            return Err(Error::Configuration);
        }
        self.io.exchange(CHUNK, bytes, emit).await
    }
    /// For JSON this also runs the configured semantic verifier. The parent
    /// still validates request-bound protocol fields and independently meters.
    /// Configured SSE uses finish_stream_frames + verify_stream_result instead.
    pub async fn finish<F, Fut>(&mut self, emit: F) -> Result<()>
    where
        F: FnMut(Decoded) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        self.io.exchange(FINISH, &[], emit).await
    }
    pub async fn stop(self) -> Result<()> {
        self.io.stop().await
    }
}

struct Worker {
    input: Option<ChildStdin>,
    output: ChildStdout,
    stop: watch::Sender<bool>,
    exit: watch::Receiver<Option<bool>>,
    session: Session,
    limits: DecodeLimits,
    timeout: Duration,
    sequence: u64,
    poisoned: bool,
    finished: bool,
    resources: Option<Arc<(OwnedSemaphorePermit, OwnedSemaphorePermit)>>,
    format: WireFormat,
    received_event_bytes: usize,
    semantic_policy: Option<Digest>,
    semantics_ready: bool,
    frames_ended: bool,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

/// Dropping an in-flight exchange poisons its framing and interrupts that child.
/// A later call cannot read a leftover partial packet as a fresh response.
struct ExchangeGuard {
    stop: watch::Sender<bool>,
    armed: bool,
}
impl Drop for ExchangeGuard {
    fn drop(&mut self) {
        if self.armed {
            self.stop.send_replace(true);
        }
    }
}

impl Worker {
    fn cancellation(&self) -> Cancellation {
        Cancellation {
            stop: self.stop.clone(),
        }
    }
    async fn stop(mut self) -> Result<()> {
        self.stop.send_replace(true);
        self.input.take();
        while self.exit.borrow().is_none() {
            self.exit.changed().await.map_err(|_| Error::Stopped)?;
        }
        Ok(())
    }
    async fn exchange<F, Fut>(&mut self, kind: u8, bytes: &[u8], mut emit: F) -> Result<()>
    where
        F: FnMut(Decoded) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        if self.poisoned || self.finished {
            return Err(Error::Stopped);
        }
        if *self.stop.borrow() {
            return Err(Error::Cancelled);
        }
        self.poisoned = true;
        let mut guard = ExchangeGuard {
            stop: self.stop.clone(),
            armed: true,
        };
        let mut cancel = self.stop.subscribe();
        self.sequence = self.sequence.checked_add(1).ok_or(Error::Protocol)?;
        let mut events = 0usize;
        // At most one buffered event plus events completed by this input chunk.
        // The shortest NDJSON record is one JSON byte plus its newline.
        let max_events = if matches!(kind, POLICY_CHUNK | POLICY_END | VERIFY_CHUNK | VERIFY_END) {
            0
        } else if matches!(kind, FINISH | FRAME_END) {
            1
        } else {
            bytes.len() / 2 + 1
        };
        let write = send(self.input.as_mut().ok_or(Error::Stopped)?, kind, bytes);
        tokio::select! {
            biased;
            _ = cancelled(&mut cancel) => return Err(Error::Cancelled),
            result = tokio::time::timeout(self.timeout, write) => result.map_err(|_| Error::ProcessingTimeout)??,
        }
        loop {
            let packet = tokio::select! {
                biased;
                _ = cancelled(&mut cancel) => return Err(Error::Cancelled),
                result = tokio::time::timeout(self.timeout, receive(&mut self.output, self.limits.ipc_bytes())) => result.map_err(|_| Error::ProcessingTimeout)??,
            };
            match packet.kind {
                EVENT => {
                    if self.format == WireFormat::Json && kind != FINISH {
                        return Err(Error::Protocol);
                    }
                    events += 1;
                    self.received_event_bytes = self
                        .received_event_bytes
                        .checked_add(packet.bytes.len())
                        .ok_or(Error::Protocol)?;
                    if events > max_events
                        || self.received_event_bytes
                            > self
                                .limits
                                .max_total_bytes
                                .saturating_mul(32)
                                .saturating_add(CONTROL_BYTES)
                    {
                        return Err(Error::Protocol);
                    }
                    let event: Decoded =
                        serde_json::from_slice(&packet.bytes).map_err(|_| Error::Protocol)?;
                    if !matches!(
                        (&event, self.format),
                        (Decoded::Json { .. }, WireFormat::Json)
                            | (Decoded::Sse { .. }, WireFormat::Sse)
                            | (Decoded::Ndjson { .. }, WireFormat::Ndjson)
                    ) && !(self.semantics_ready
                        && self.format == WireFormat::Ndjson
                        && matches!(event, Decoded::Sse { .. }))
                    {
                        return Err(Error::Protocol);
                    }
                    tokio::select! {
                        biased;
                        _ = cancelled(&mut cancel) => return Err(Error::Cancelled),
                        result = emit(event) => result?,
                    }
                }
                FAILURE => {
                    if packet.bytes.len() > CONTROL_BYTES {
                        return Err(Error::Protocol);
                    }
                    let failure: FailureSnapshot =
                        serde_json::from_slice(&packet.bytes).map_err(|_| Error::Protocol)?;
                    if matches!(kind, POLICY_CHUNK | POLICY_END) {
                        if failure.execution != Execution::NotDispatched
                            || failure.stage != Stage::BeforeDispatch
                            || failure.scope != Scope::Request
                            || failure.code != Code::InvalidSchema
                        {
                            return Err(Error::Protocol);
                        }
                    } else if failure.execution != Execution::Unknown
                        || failure.stage != Stage::ResponseBody
                    {
                        return Err(Error::Protocol);
                    }
                    return Err(Error::Upstream(
                        failure.to_failure().map_err(|_| Error::Protocol)?,
                    ));
                }
                ACK | END => {
                    if packet.kind
                        != if matches!(kind, FINISH | VERIFY_END) {
                            END
                        } else {
                            ACK
                        }
                        || packet.bytes.as_slice() != self.sequence.to_le_bytes()
                    {
                        return Err(Error::Protocol);
                    }
                    if kind == FINISH && self.format == WireFormat::Json && events != 1 {
                        return Err(Error::Protocol);
                    }
                    if matches!(kind, FINISH | VERIFY_END) {
                        self.finished = true;
                        if let Some(mut input) = self.input.take() {
                            let _ = input.shutdown().await;
                        }
                        let wait = async {
                            loop {
                                if let Some(ok) = *self.exit.borrow() {
                                    return if ok { Ok(()) } else { Err(Error::Stopped) };
                                }
                                self.exit.changed().await.map_err(|_| Error::Stopped)?;
                            }
                        };
                        tokio::select! {
                            biased;
                            _ = cancelled(&mut cancel) => return Err(Error::Cancelled),
                            result = tokio::time::timeout(self.timeout, wait) => result.map_err(|_| Error::ProcessingTimeout)??,
                        }
                        self.resources.take();
                    }
                    self.poisoned = false;
                    guard.armed = false;
                    return Ok(());
                }
                _ => return Err(Error::Protocol),
            }
        }
    }
}

#[cfg(all(test, unix))]
mod received_limit_tests {
    use super::*;
    #[test]
    fn impossible_received_ceiling_is_rejected_before_paid_admission() {
        use std::os::unix::fs::PermissionsExt;
        let work = tempfile::tempdir().unwrap();
        std::fs::set_permissions(work.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let limits = PoolLimits {
            max_children: 8,
            max_buffer_bytes: 128 * 1024 * 1024,
            startup_timeout: Duration::from_secs(10),
            processing_timeout: Duration::from_secs(10),
        };
        let pool = Pool::new(std::env::current_exe().unwrap(), work.path(), limits).unwrap();
        assert!(pool.require_received_limit(4 * 1024 * 1024).is_ok());
        assert!(matches!(
            pool.require_received_limit(8 * 1024 * 1024),
            Err(Error::Configuration)
        ));
        let larger = pool
            .with_independent_limits(PoolLimits {
                max_buffer_bytes: 256 * 1024 * 1024,
                ..limits
            })
            .unwrap();
        assert!(larger.require_received_limit(8 * 1024 * 1024).is_ok());
        assert!(larger.require_received_limit(0).is_err());
        assert!(larger
            .require_received_limit(256 * 1024 * 1024 + 1)
            .is_err());
    }
}
