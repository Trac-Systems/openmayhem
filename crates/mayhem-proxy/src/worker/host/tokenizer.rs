//! Source-owned, bounded contained tokenizer actors. Parsed artifacts stay in a
//! child; source drop, timeout and cancellation kill/reap it before permit reuse.
use super::*;
use crate::health::native::{engine, Field, Limits};
use std::sync::OnceLock;
use tokio::{
    io::AsyncReadExt,
    sync::{mpsc, oneshot, Mutex},
    task::JoinSet,
};

#[derive(Clone)]
pub(crate) struct Tokenizer {
    sender: mpsc::Sender<Request>,
    program: PathBuf,
    workdir: PathBuf,
    stopped: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}
struct Request {
    fields: Vec<Field>,
    retained: Option<OwnedSemaphorePermit>,
    answer: Answer,
}
enum Answer {
    Async(oneshot::Sender<Result<u64>>),
    Sync(std::sync::mpsc::SyncSender<Result<u64>>),
}
impl Answer {
    async fn closed(&mut self) {
        match self {
            Self::Async(s) => s.closed().await,
            Self::Sync(_) => std::future::pending().await,
        }
    }
    fn send(self, result: Result<u64>) {
        match self {
            Self::Async(s) => {
                let _ = s.send(result);
            }
            Self::Sync(s) => {
                let _ = s.send(result);
            }
        }
    }
}
struct State {
    pool: Pool,
    bytes: Arc<[u8]>,
    digest: Digest,
    limits: Limits,
    idle: Mutex<Vec<Process>>,
}
impl Pool {
    pub(crate) fn tokenizer(
        &self,
        bytes: Arc<[u8]>,
        digest: Digest,
        limits: Limits,
    ) -> Result<Tokenizer> {
        crate::health::native::check_limits(limits).map_err(|_| Error::Configuration)?;
        static ACTORS: OnceLock<Arc<Semaphore>> = OnceLock::new();
        let actor = ACTORS
            .get_or_init(|| Arc::new(Semaphore::new(128)))
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Capacity)?;
        let (sender, mut receiver) = mpsc::channel::<Request>(limits.workers);
        // Preserve the host's exact staged executable authority (including any
        // platform launch capability); only tokenizer-local budgets are replaced.
        let mut pool = self.clone();
        pool.limits = PoolLimits {
            max_children: limits.workers,
            max_buffer_bytes: (limits.artifact_bytes + limits.output_bytes + 8192) * limits.workers,
            startup_timeout: self
                .limits
                .startup_timeout
                .min(Duration::from_secs(engine::WALL_SECONDS)),
            processing_timeout: self
                .limits
                .processing_timeout
                .min(Duration::from_secs(engine::WALL_SECONDS)),
        };
        pool.children = Arc::new(Semaphore::new(limits.workers));
        pool.buffers = Arc::new(Semaphore::new(limits.workers));
        let state = Arc::new(State {
            pool,
            bytes,
            digest,
            limits,
            idle: Mutex::new(Vec::new()),
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| Error::Start)?;
        let stopped = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let completed = stopped.clone();
        std::thread::Builder::new().name("proxy-tokenizer".into()).spawn(move || {
            let _actor=actor;
            runtime.block_on(async move {
                let mut tasks=JoinSet::new();
                loop {
                    tokio::select! {
                        request=receiver.recv()=> match request {
                            Some(request)=> {
                                while tasks.try_join_next().is_some() {}
                                if tasks.len() >= limits.workers {request.answer.send(Err(Error::Capacity));continue;}
                                let state=state.clone();
                                tasks.spawn(async move {state.request(request).await});
                            },
                            None=>break,
                        },
                        _=tasks.join_next(), if !tasks.is_empty()=>(),
                    }
                }
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                state.idle.lock().await.clear();
                // Supervisors retain process permits until wait/reap finishes.
                // Do not tear down their runtime prematurely after source drop.
                while state.pool.children.available_permits()!=limits.workers {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            });
            // Release all launcher/image authority before reporting completion.
            drop(runtime);
            if let Ok(mut value) = completed.0.lock() {
                *value = true;
                completed.1.notify_all();
            }
        }).map_err(|_|Error::Start)?;
        Ok(Tokenizer {
            sender,
            program: self.program.clone(),
            workdir: self.workdir.clone(),
            stopped,
        })
    }
}
impl Tokenizer {
    /// Installation only: consume the source's last sender and await actual
    /// reaping before moving its staging directory. Never used per inference.
    pub(crate) fn finish_installation(self) -> Result<()> {
        let stopped = self.stopped.clone();
        drop(self);
        let value = stopped.0.lock().map_err(|_| Error::Stopped)?;
        let (value, _) = stopped
            .1
            .wait_timeout_while(value, Duration::from_secs(engine::WALL_SECONDS * 2), |v| {
                !*v
            })
            .map_err(|_| Error::Stopped)?;
        if *value {
            Ok(())
        } else {
            Err(Error::ProcessingTimeout)
        }
    }
    pub(crate) fn same_launcher(&self, pool: &Pool) -> bool {
        self.program == pool.program && self.workdir == pool.workdir
    }
    pub(crate) fn validate(&self) -> Result<()> {
        let (answer, receiver) = std::sync::mpsc::sync_channel(1);
        self.sender
            .try_send(Request {
                fields: Vec::new(),
                retained: None,
                answer: Answer::Sync(answer),
            })
            .map_err(|_| Error::Capacity)?;
        // Runs only inside the owner's bounded startup/provisioning scope. The
        // actor enforces processing deadlines and replies after failure reaping.
        receiver.recv().map_err(|_| Error::Stopped)?.map(|_| ())
    }
    pub(crate) async fn count(
        &self,
        fields: Vec<Field>,
        retained: OwnedSemaphorePermit,
    ) -> Result<u64> {
        let (answer, receiver) = oneshot::channel();
        self.sender
            .try_send(Request {
                fields,
                retained: Some(retained),
                answer: Answer::Async(answer),
            })
            .map_err(|_| Error::Capacity)?;
        receiver.await.map_err(|_| Error::Stopped)?
    }
}
impl State {
    async fn request(self: Arc<Self>, request: Request) {
        let Request {
            fields,
            mut retained,
            mut answer,
        } = request;
        let operation = async {
            let cached = { self.idle.lock().await.pop() };
            let mut process = match cached {
                Some(p) => {
                    *p.retained.lock().map_err(|_| Error::Stopped)? = retained.take();
                    p
                }
                None => self.start(&mut retained).await?,
            };
            let nonce = nonce()?;
            let job = engine::Job {
                nonce: nonce.clone(),
                fields: fields.len(),
            };
            let result = tokio::time::timeout(self.pool.limits.processing_timeout, async {
                send(
                    &mut process.input,
                    &serde_json::to_vec(&job).map_err(|_| Error::Protocol)?,
                )
                .await?;
                for field in &fields {
                    process
                        .input
                        .write_all(&(field.first_bytes as u32).to_le_bytes())
                        .await
                        .map_err(|_| Error::Stopped)?;
                    send(&mut process.input, field.text.as_bytes()).await?;
                }
                receive(
                    &mut process.output,
                    &nonce,
                    &self.digest,
                    2,
                    self.limits.output_bytes,
                )
                .await
            })
            .await
            .map_err(|_| Error::ProcessingTimeout)
            .and_then(|v| v);
            match result {
                Ok(tokens) => Ok((tokens, process)),
                Err(error) => {
                    process.stop().await;
                    Err(error)
                }
            }
        };
        let result =
            tokio::select! {value=operation=>value,_=answer.closed()=>Err(Error::Cancelled)};
        // On cancellation the process guard requests kill. Its supervisor owns
        // the capture permit through reaping, even if this request is dropped.
        drop(fields);
        drop(retained);
        let result = match result {
            Ok((tokens, process)) => {
                if let Ok(mut slot) = process.retained.lock() {
                    slot.take();
                }
                self.idle.lock().await.push(process);
                Ok(tokens)
            }
            Err(error) => Err(error),
        };
        answer.send(result);
    }
    async fn start(&self, retained: &mut Option<OwnedSemaphorePermit>) -> Result<Process> {
        let permit = self
            .pool
            .children
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Capacity)?;
        let mut child = self.pool.spawn_bundled(BundledMode::Tokenizer)?;
        let input = child.stdin.take().ok_or(Error::Start)?;
        let output = child.stdout.take().ok_or(Error::Start)?;
        let retained = Arc::new(std::sync::Mutex::new(retained.take()));
        let supervised_retained = retained.clone();
        let (stop, mut stopped) = watch::channel(false);
        let (exit, exited) = watch::channel(None);
        tokio::spawn(async move {
            let status = tokio::select! {result=child.wait()=>result,_=stopped.changed()=>{let _=child.start_kill();child.wait().await}};
            if let Ok(mut slot) = supervised_retained.lock() {
                slot.take();
            }
            drop(permit);
            exit.send_replace(Some(status.is_ok_and(|v| v.success())));
        });
        let mut process = Process {
            input,
            output,
            stop,
            exited,
            retained,
        };
        let nonce = nonce()?;
        let init = engine::Init {
            abi: 2,
            release: RELEASE.into(),
            nonce: nonce.clone(),
            digest: self.digest.clone(),
            limits: self.limits,
            fields: 0,
        };
        let result = tokio::time::timeout(self.pool.limits.startup_timeout, async {
            send(
                &mut process.input,
                &serde_json::to_vec(&init).map_err(|_| Error::Protocol)?,
            )
            .await?;
            send(&mut process.input, &self.bytes).await?;
            let n = receive(&mut process.output, &nonce, &self.digest, 2, 0).await?;
            if n != 0 {
                return Err(Error::Protocol);
            }
            Ok(())
        })
        .await
        .map_err(|_| Error::ProcessingTimeout)
        .and_then(|v| v);
        if let Err(error) = result {
            process.stop().await;
            return Err(error);
        }
        Ok(process)
    }
}
struct Process {
    input: ChildStdin,
    output: ChildStdout,
    stop: watch::Sender<bool>,
    exited: watch::Receiver<Option<bool>>,
    retained: Arc<std::sync::Mutex<Option<OwnedSemaphorePermit>>>,
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}
impl Process {
    async fn stop(&mut self) {
        self.stop.send_replace(true);
        while self.exited.borrow().is_none() {
            if self.exited.changed().await.is_err() {
                break;
            }
        }
    }
}
fn nonce() -> Result<Digest> {
    let mut b = [0; 32];
    getrandom::fill(&mut b).map_err(|_| Error::Start)?;
    Ok(Digest::hash("mayhem/proxy/tokenizer-ipc/v2", &[&b]))
}
async fn send(output: &mut ChildStdin, bytes: &[u8]) -> Result<()> {
    let size = u32::try_from(bytes.len()).map_err(|_| Error::Protocol)?;
    output
        .write_all(&size.to_le_bytes())
        .await
        .map_err(|_| Error::Stopped)?;
    output.write_all(bytes).await.map_err(|_| Error::Stopped)?;
    output.flush().await.map_err(|_| Error::Stopped)
}
async fn receive(
    input: &mut ChildStdout,
    nonce: &Digest,
    digest: &Digest,
    abi: u32,
    output_bytes: usize,
) -> Result<u64> {
    let length = input.read_u32_le().await.map_err(|_| Error::Stopped)? as usize;
    if length > engine::CONTROL {
        return Err(Error::Protocol);
    }
    let mut body = vec![0; length];
    input
        .read_exact(&mut body)
        .await
        .map_err(|_| Error::Stopped)?;
    let reply: engine::Reply = serde_json::from_slice(&body).map_err(|_| Error::Protocol)?;
    if reply.abi != abi
        || reply.release != RELEASE
        || &reply.nonce != nonce
        || &reply.digest != digest
        || reply.tokens > output_bytes as u64 * 16
    {
        return Err(Error::Identity);
    }
    Ok(reply.tokens)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn limits() -> Limits {
        Limits {
            artifact_bytes: 1024 * 1024,
            output_bytes: 1024 * 1024,
            channels: 8,
            workers: 1,
            minimum_tokens: 2,
        }
    }
    fn fixture(program: &Path, work: &Path, deadline: Duration) -> Pool {
        std::fs::set_permissions(work, std::fs::Permissions::from_mode(0o700)).unwrap();
        Pool::new(
            program,
            work,
            PoolLimits {
                max_children: 1,
                max_buffer_bytes: 4 * 1024 * 1024,
                startup_timeout: deadline,
                processing_timeout: deadline,
            },
        )
        .unwrap()
    }
    fn data() -> (Arc<[u8]>, Digest) {
        let b=serde_json::to_vec(&serde_json::json!({"version":"1.0","truncation":null,"padding":null,"added_tokens":[],
            "normalizer":null,"pre_tokenizer":{"type":"WhitespaceSplit"},"post_processor":null,"decoder":null,
            "model":{"type":"WordLevel","vocab":{"[UNK]":0,"one":1,"two":2,"三":3},"unk_token":"[UNK]"}})).unwrap();
        let d = Digest::new(blake3::hash(&b).to_hex().as_str()).unwrap();
        (Arc::from(b), d)
    }
    #[tokio::test]
    async fn tokenizer_deadline_and_cancel_retain_capture_until_reap() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("fixture");
        std::fs::write(&program, "#!/bin/sh\nexec /bin/sleep 30\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir(&work).unwrap();
        let pool = fixture(&program, &work, Duration::from_millis(50));
        let (b, d) = data();
        let actor = pool.tokenizer(b, d, limits()).unwrap();
        let captures = Arc::new(Semaphore::new(1));
        assert!(matches!(
            actor
                .count(vec![], captures.clone().try_acquire_owned().unwrap())
                .await,
            Err(Error::ProcessingTimeout)
        ));
        assert_eq!(captures.available_permits(), 1);
        let next = actor.clone();
        let cap = captures.clone();
        let task =
            tokio::spawn(async move { next.count(vec![], cap.try_acquire_owned().unwrap()).await });
        while captures.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
        task.abort();
        let _ = task.await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while captures.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn tokenizer_actor_retains_pin_reuses_child_and_recovers_after_rejected_job() {
        let dir = tempfile::tempdir().unwrap();
        let program = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("mayhem-proxy-worker");
        let pool = fixture(&program, dir.path(), Duration::from_secs(5));
        let (b, d) = data();
        let actor = pool.tokenizer(b, d, limits()).unwrap();
        actor.validate().unwrap();
        let cap = Arc::new(Semaphore::new(1));
        for first in [4, 4, 999, 4] {
            let result = actor
                .count(
                    vec![Field {
                        text: "one two 三".into(),
                        first_bytes: first,
                    }],
                    cap.clone().try_acquire_owned().unwrap(),
                )
                .await;
            if first == 999 {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap(), 2);
            }
            assert_eq!(cap.available_permits(), 1);
        }
    }
    #[tokio::test]
    async fn tokenizer_warm_count_deadline_kills_reaps_and_recreates_the_original_pin() {
        let dir = tempfile::tempdir().unwrap();
        let program = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("mayhem-proxy-worker");
        let mut pool = fixture(&program, dir.path(), Duration::from_secs(5));
        pool.limits.processing_timeout = Duration::from_millis(50);
        let mut limits = limits();
        limits.output_bytes = 4 * 1024 * 1024;
        let (b, d) = data();
        let actor = pool.tokenizer(b, d, limits).unwrap();
        actor.validate().unwrap();
        let captures = Arc::new(Semaphore::new(1));
        let result = actor
            .count(
                vec![Field {
                    text: "one ".repeat(1_000_000),
                    first_bytes: 0,
                }],
                captures.clone().try_acquire_owned().unwrap(),
            )
            .await;
        assert!(
            matches!(result, Err(Error::ProcessingTimeout)),
            "{result:?}"
        );
        assert_eq!(captures.available_permits(), 1);
        assert_eq!(
            actor
                .count(
                    vec![Field {
                        text: "one two 三".into(),
                        first_bytes: 4
                    }],
                    captures.clone().try_acquire_owned().unwrap()
                )
                .await
                .unwrap(),
            2
        );
    }
}
