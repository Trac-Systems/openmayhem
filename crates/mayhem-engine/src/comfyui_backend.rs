use super::{
    validate_load_config, verify_artifact, ArtifactChunk, ArtifactSink, CancellationToken,
    EngineBackend, EngineError, GenerateOutput, GenerateRequest, LoadConfig, LoadedModelInfo,
    Result, TokenSink, Tokenization, WorkflowGenerationOutput, WorkflowGenerationRequest,
    WorkflowProgressEvent,
};
use base64::Engine as _;
use mayhem_enclave::{
    SandboxConfig, SandboxedChild, SandboxedChildStderr, SandboxedChildStdin, SandboxedChildStdout,
    SandboxedCommand, SandboxedStderr,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Digest as _;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const WORKER: &str = include_str!("comfyui_worker.py");
const WORKER_STDIN_BOOTSTRAP: &str = concat!(
    "import base64,sys;",
    "exec(compile(base64.b64decode(sys.stdin.buffer.readline()),",
    "'<mayhem-comfyui-worker>','exec'))"
);
const PYTHON_ENV: &str = "MAYHEM_COMFYUI_PYTHON";
const DEVICE_ENV: &str = "MAYHEM_COMFYUI_DEVICE";
const ARTIFACT_CHUNK_BYTES: usize = 256 * 1024;
const WORKER_STDERR_TAIL_BYTES: usize = 64 * 1024;
const MAX_WORKER_REQUEST_LINE_BYTES: usize = 64 * 1024 * 1024;
const MAX_WORKER_RESPONSE_LINE_BYTES: usize = 192 * 1024 * 1024;
const LOAD_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct ComfyUiBackend {
    worker: Option<ComfyUiWorker>,
    loaded: Option<LoadedComfyUi>,
}

#[derive(Clone, Debug)]
struct LoadedComfyUi {
    evidence: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerMessage {
    id: u64,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerLoadResult {
    object_info_classes: u32,
    node_classes: Vec<String>,
    socket_path: Option<String>,
    control_mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerWorkflowResult {
    prompt_id: String,
    artifacts: Vec<WorkerArtifact>,
    progress_events: Vec<WorkflowProgressEvent>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerArtifact {
    artifact_id: String,
    content_type: String,
    data_base64: String,
}

impl ComfyUiBackend {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn worker(&mut self) -> Result<&mut ComfyUiWorker> {
        self.worker.as_mut().ok_or_else(|| EngineError::NotLoaded)
    }

    fn next_request_id() -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}

impl EngineBackend for ComfyUiBackend {
    fn backend_id(&self) -> &'static str {
        "comfyui"
    }

    fn load(&mut self, config: LoadConfig) -> Result<LoadedModelInfo> {
        validate_load_config(&config)?;
        verify_artifact(&config.artifact)?;
        if config.artifact.format != super::ArtifactFormat::ComfyUiRuntime {
            return Err(EngineError::ComfyUi(
                "ComfyUI backend requires a ComfyUI runtime artifact".to_owned(),
            ));
        }
        let runtime_root = fs::canonicalize(&config.artifact.path).map_err(|error| {
            EngineError::ComfyUi(format!(
                "canonicalizing ComfyUI runtime {} failed: {error}",
                config.artifact.path.display()
            ))
        })?;
        let cache_root = config
            .backend_cache_dir
            .clone()
            .unwrap_or_else(default_cache_root);
        fs::create_dir_all(&cache_root).map_err(|error| {
            EngineError::ComfyUi(format!(
                "creating ComfyUI cache root {} failed: {error}",
                cache_root.display()
            ))
        })?;
        let python = env::var_os(PYTHON_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("python3"));
        let device = env::var(DEVICE_ENV).unwrap_or_else(|_| "cpu".to_owned());
        let socket_dir = short_socket_dir();
        let mut worker = ComfyUiWorker::spawn(
            &python,
            config.memory_limit_bytes,
            &cache_root,
            &socket_dir,
            &runtime_root,
        )?;
        let id = Self::next_request_id();
        let base_dir = cache_root.join("base");
        let socket_path = socket_dir.join("comfy.sock");
        worker.send(
            id,
            "load",
            json!({
                "runtime_root": runtime_root,
                "base_dir": base_dir,
                "socket_path": socket_path,
                "device": device,
            }),
        )?;
        let response: WorkerLoadResult =
            worker.wait_response(id, LOAD_TIMEOUT, &CancellationToken::new())?;
        let info = LoadedModelInfo {
            backend: self.backend_id().to_owned(),
            artifact: config.artifact,
            ctx_size: config.ctx_size,
            n_ctx_train: 0,
            n_vocab: 0,
        };
        let evidence = json!({
            "runtime_root": runtime_root,
            "socket_path": response.socket_path,
            "control_mode": response.control_mode,
            "object_info_classes": response.object_info_classes,
            "node_classes_hash": sha256_json(&response.node_classes)?,
            "device": device,
        });
        self.loaded = Some(LoadedComfyUi { evidence });
        self.worker = Some(worker);
        Ok(info)
    }

    fn loaded_backend_evidence(&self) -> Option<Value> {
        self.loaded.as_ref().map(|loaded| loaded.evidence.clone())
    }

    fn component_healthy(&mut self) -> bool {
        self.worker
            .as_mut()
            .is_some_and(|worker| worker.child.try_wait().is_ok_and(|status| status.is_none()))
    }

    fn process_ids(&self) -> Vec<u32> {
        self.worker
            .as_ref()
            .map(|worker| vec![worker.child.id()])
            .unwrap_or_default()
    }

    fn tokenize(&self, _text: &str) -> Result<Tokenization> {
        Err(EngineError::InvalidConfig(
            "ComfyUI backend does not tokenize text".to_owned(),
        ))
    }

    fn generate(
        &mut self,
        _request: GenerateRequest,
        _sink: &mut dyn TokenSink,
        _cancellation: &CancellationToken,
    ) -> Result<GenerateOutput> {
        Err(EngineError::InvalidConfig(
            "ComfyUI backend does not support text generation".to_owned(),
        ))
    }

    fn run_workflow(
        &mut self,
        request: WorkflowGenerationRequest,
        artifact_sink: &mut dyn ArtifactSink,
        cancellation: &CancellationToken,
    ) -> Result<WorkflowGenerationOutput> {
        request.validate()?;
        let id = Self::next_request_id();
        self.worker()?.send(
            id,
            "run_workflow",
            json!({
                "workflow": request.workflow,
                "client_id": request.client_id,
                "timeout_ms": request.timeout_ms,
            }),
        )?;
        let response: WorkerWorkflowResult = self.worker()?.wait_response(
            id,
            Duration::from_millis(request.timeout_ms),
            cancellation,
        )?;
        for artifact in &response.artifacts {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&artifact.data_base64)
                .map_err(|error| {
                    EngineError::ComfyUi(format!("decoding ComfyUI artifact failed: {error}"))
                })?;
            for (index, chunk) in bytes.chunks(ARTIFACT_CHUNK_BYTES).enumerate() {
                cancellation.check()?;
                artifact_sink.on_artifact_chunk(ArtifactChunk {
                    artifact_id: artifact.artifact_id.clone(),
                    index: u32::try_from(index).map_err(|_| {
                        EngineError::ComfyUi("ComfyUI artifact has too many chunks".to_owned())
                    })?,
                    content_type: artifact.content_type.clone(),
                    bytes: chunk.to_vec(),
                    final_chunk: (index + 1) * ARTIFACT_CHUNK_BYTES >= bytes.len(),
                })?;
            }
        }
        Ok(WorkflowGenerationOutput {
            prompt_id: response.prompt_id,
            artifact_count: response.artifacts.len() as u32,
            progress_events: response.progress_events,
        })
    }
}

impl Drop for ComfyUiBackend {
    fn drop(&mut self) {
        if let Some(worker) = &mut self.worker {
            worker.stop();
        }
    }
}

struct ComfyUiWorker {
    child: SandboxedChild,
    stdin: SandboxedChildStdin,
    stdout_rx: Option<Receiver<WorkerRead>>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    stderr_reader: Option<JoinHandle<()>>,
}

impl ComfyUiWorker {
    fn spawn(
        python: &Path,
        memory_limit_bytes: Option<u64>,
        cache_root: &Path,
        socket_dir: &Path,
        runtime_root: &Path,
    ) -> Result<Self> {
        let (python, runtime_roots) = python_runtime_roots(python)?;
        let executable_runtime_roots = runtime_roots.clone();
        let mut read_only_dirs = vec![runtime_root.to_path_buf()];
        read_only_dirs.extend(runtime_roots);
        fs::create_dir_all(socket_dir).map_err(|error| {
            EngineError::ComfyUi(format!(
                "creating ComfyUI socket directory {} failed: {error}",
                socket_dir.display()
            ))
        })?;
        let sandbox = SandboxConfig::new(
            read_only_dirs,
            vec![cache_root.to_path_buf(), socket_dir.to_path_buf()],
        );
        let mut command = SandboxedCommand::new(&python);
        if let Some(memory_limit_bytes) = memory_limit_bytes {
            command.memory_limit_bytes(memory_limit_bytes);
        }
        configure_worker_environment(&mut command, &python, cache_root)?;
        for root in executable_runtime_roots {
            command.executable_read_only_dir(root);
        }
        command.allow_code_generation();
        command
            .current_dir(cache_root)
            .stderr(SandboxedStderr::Piped)
            .arg("-I")
            .arg("-u")
            .arg("-c")
            .arg(WORKER_STDIN_BOOTSTRAP);
        let mut child = command.spawn(&sandbox).map_err(|error| {
            EngineError::ComfyUi(format!(
                "starting sandboxed ComfyUI worker with {} failed: {error}",
                python.display()
            ))
        })?;
        let mut stdin = child.take_stdin().ok_or_else(|| {
            EngineError::ComfyUi("opening ComfyUI worker stdin failed".to_owned())
        })?;
        let worker_source = base64::engine::general_purpose::STANDARD.encode(WORKER.as_bytes());
        stdin
            .write_all(worker_source.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .map_err(|error| {
                EngineError::ComfyUi(format!(
                    "sending ComfyUI worker source over the private bootstrap pipe failed: {error}"
                ))
            })?;
        let stdout = child.take_stdout().ok_or_else(|| {
            EngineError::ComfyUi("opening ComfyUI worker stdout failed".to_owned())
        })?;
        let stderr = child.take_stderr().ok_or_else(|| {
            EngineError::ComfyUi("opening ComfyUI worker stderr failed".to_owned())
        })?;
        let (stdout_tx, stdout_rx) = mpsc::sync_channel(super::WORKER_STDOUT_QUEUE_CAPACITY);
        let stdout_reader = thread::spawn(move || read_worker_stdout(stdout, stdout_tx));
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&stderr_tail);
        let stderr_reader = thread::spawn(move || read_worker_stderr(stderr, capture));
        Ok(Self {
            child,
            stdin,
            stdout_rx: Some(stdout_rx),
            stdout_reader: Some(stdout_reader),
            stderr_tail,
            stderr_reader: Some(stderr_reader),
        })
    }

    fn send(&mut self, id: u64, operation: &str, payload: Value) -> Result<()> {
        let bytes = serde_json::to_vec(&json!({
            "id": id,
            "op": operation,
            "payload": payload,
        }))?;
        if bytes.len() > MAX_WORKER_REQUEST_LINE_BYTES {
            return Err(EngineError::ComfyUi(
                "ComfyUI worker request exceeds the protocol bound".to_owned(),
            ));
        }
        self.stdin.write_all(&bytes)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn wait_response<T: for<'de> Deserialize<'de>>(
        &mut self,
        id: u64,
        wait: Duration,
        cancellation: &CancellationToken,
    ) -> Result<T> {
        let deadline = Instant::now() + wait;
        loop {
            cancellation.check()?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(EngineError::ComfyUi(format!(
                    "ComfyUI worker did not reply to request {id} before timeout"
                )));
            }
            match self.read_message(remaining.min(Duration::from_millis(100)))? {
                Some(message) if message.id == id => {
                    if !message.ok {
                        return Err(EngineError::ComfyUi(message.error.unwrap_or_else(|| {
                            "ComfyUI worker returned an unknown error".to_owned()
                        })));
                    }
                    let value = message.result.ok_or_else(|| {
                        EngineError::ComfyUi("ComfyUI worker response omitted result".to_owned())
                    })?;
                    return serde_json::from_value(value).map_err(EngineError::from);
                }
                Some(_) | None => continue,
            }
        }
    }

    fn read_message(&mut self, wait: Duration) -> Result<Option<WorkerMessage>> {
        let read = match self
            .stdout_rx
            .as_ref()
            .ok_or_else(|| {
                EngineError::ComfyUi("ComfyUI worker stdout reader is closed".to_owned())
            })?
            .recv_timeout(wait)
        {
            Ok(read) => read,
            Err(RecvTimeoutError::Timeout) => return Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                return Err(EngineError::ComfyUi(
                    "ComfyUI worker stdout reader stopped".to_owned(),
                ))
            }
        };
        let line = match read {
            WorkerRead::Line(line) => line,
            WorkerRead::Eof => return Err(self.exit_error("ComfyUI worker exited before replying")),
            WorkerRead::Error(error) => return Err(EngineError::ComfyUi(error)),
        };
        serde_json::from_str(line.trim_end())
            .map(Some)
            .map_err(|error| {
                EngineError::ComfyUi(format!(
                    "decoding ComfyUI worker protocol line failed: {error}"
                ))
            })
    }

    fn stop(&mut self) {
        let _ = self.send(0, "shutdown", Value::Null);
        self.stdout_rx.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.stdout_reader.take() {
            let _ = reader.join();
        }
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
    }

    fn exit_error(&mut self, message: &str) -> EngineError {
        let status = match self.child.try_wait() {
            Ok(Some(status)) => status.to_string(),
            Ok(None) => self
                .child
                .wait()
                .map(|status| status.to_string())
                .unwrap_or_else(|error| format!("unavailable ({error})")),
            Err(error) => format!("unavailable ({error})"),
        };
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
        let stderr = worker_stderr_text(&self.stderr_tail);
        if stderr.is_empty() {
            EngineError::ComfyUi(format!("{message}; exit status {status}; stderr was empty"))
        } else {
            EngineError::ComfyUi(format!(
                "{message}; exit status {status}; stderr tail: {stderr}"
            ))
        }
    }
}

fn default_cache_root() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    env::temp_dir().join(format!("mayhem-comfyui-{}-{stamp}", std::process::id()))
}

fn short_socket_dir() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    #[cfg(unix)]
    {
        PathBuf::from(format!("/tmp/mhcf-{}-{stamp}", std::process::id()))
    }
    #[cfg(not(unix))]
    {
        env::temp_dir().join(format!("mhcf-{}-{stamp}", std::process::id()))
    }
}

fn configure_worker_environment(
    command: &mut SandboxedCommand,
    python: &Path,
    cache_root: &Path,
) -> Result<()> {
    for (name, path) in [
        ("HOME", cache_root.join("home")),
        ("TMPDIR", cache_root.join("tmp")),
        ("TMP", cache_root.join("tmp")),
        ("TEMP", cache_root.join("tmp")),
        ("DARWIN_USER_CACHE_DIR", cache_root.join("darwin-cache")),
        ("DARWIN_USER_TEMP_DIR", cache_root.join("darwin-tmp")),
        ("CFFIXED_USER_HOME", cache_root.join("home")),
        ("XDG_CACHE_HOME", cache_root.join("xdg")),
        ("XDG_RUNTIME_DIR", cache_root.join("xdg-runtime")),
        ("HF_HOME", cache_root.join("huggingface")),
        ("HF_HUB_CACHE", cache_root.join("huggingface/hub")),
        ("TRANSFORMERS_CACHE", cache_root.join("transformers")),
        ("TORCH_HOME", cache_root.join("torch")),
    ] {
        fs::create_dir_all(&path).map_err(|error| {
            EngineError::ComfyUi(format!(
                "creating ComfyUI cache directory {} failed: {error}",
                path.display()
            ))
        })?;
        command.env(name, path);
    }
    command
        .env("HF_HUB_OFFLINE", "1")
        .env("HF_DATASETS_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .env("HF_HUB_DISABLE_TELEMETRY", "1")
        .env("DO_NOT_TRACK", "1")
        .env("PIP_NO_INDEX", "1")
        .env("UV_OFFLINE", "1")
        .env("TOKENIZERS_PARALLELISM", "false")
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for name in [
        "PYTHONHOME",
        "PYTHONPATH",
        "PYTHONSTARTUP",
        "PYTHONINSPECT",
        "LD_PRELOAD",
        "DYLD_INSERT_LIBRARIES",
        "HF_TOKEN",
        "HUGGING_FACE_HUB_TOKEN",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
    ] {
        command.env_remove(name);
    }
    if let Some(python_bin) = python.parent() {
        let mut paths = vec![python_bin.to_path_buf()];
        #[cfg(unix)]
        paths.extend([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
        #[cfg(windows)]
        if let Some(system_root) = env::var_os("SystemRoot") {
            paths.push(PathBuf::from(system_root).join("System32"));
        }
        let path = env::join_paths(paths).map_err(|error| {
            EngineError::ComfyUi(format!("building ComfyUI PATH failed: {error}"))
        })?;
        command.env("PATH", path);
    }
    Ok(())
}

fn python_runtime_roots(python: &Path) -> Result<(PathBuf, Vec<PathBuf>)> {
    let python = resolve_python_program(python)?;
    let canonical_python = fs::canonicalize(&python).map_err(|error| {
        EngineError::ComfyUi(format!(
            "canonicalizing ComfyUI Python {} failed: {error}",
            python.display()
        ))
    })?;
    let executable_root = canonical_python.parent().ok_or_else(|| {
        EngineError::ComfyUi(format!(
            "ComfyUI Python {} has no runtime directory",
            canonical_python.display()
        ))
    })?;
    let mut candidates = vec![executable_root.to_path_buf()];
    if let Some(environment_root) = python
        .parent()
        .filter(|parent| {
            parent
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("bin") || name == "Scripts")
        })
        .and_then(Path::parent)
    {
        candidates.push(environment_root.to_path_buf());
        collect_python_library_roots(environment_root, &mut candidates)?;
        let config = environment_root.join("pyvenv.cfg");
        if let Ok(config) = fs::read_to_string(config) {
            if let Some(home) = config.lines().find_map(|line| {
                line.split_once('=')
                    .filter(|(key, _)| key.trim().eq_ignore_ascii_case("home"))
                    .map(|(_, value)| PathBuf::from(value.trim()))
            }) {
                let base = if home
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("bin") || name == "Scripts")
                {
                    home.parent().unwrap_or(&home)
                } else {
                    home.as_path()
                };
                candidates.push(base.to_path_buf());
                collect_python_library_roots(base, &mut candidates)?;
            }
        }
    }
    let mut roots = Vec::new();
    for candidate in candidates {
        if !candidate.is_dir() {
            continue;
        }
        let candidate = fs::canonicalize(candidate).map_err(|error| {
            EngineError::ComfyUi(format!("canonicalizing runtime failed: {error}"))
        })?;
        insert_non_overlapping_root(&mut roots, candidate);
    }
    if roots.is_empty() {
        return Err(EngineError::ComfyUi(
            "ComfyUI Python has no readable runtime roots".to_owned(),
        ));
    }
    Ok((python, roots))
}

fn resolve_python_program(python: &Path) -> Result<PathBuf> {
    let candidates = if python.is_absolute() || python.components().count() > 1 {
        vec![if python.is_absolute() {
            python.to_path_buf()
        } else {
            env::current_dir()?.join(python)
        }]
    } else {
        env::var_os("PATH")
            .into_iter()
            .flat_map(|value| env::split_paths(&value).collect::<Vec<_>>())
            .map(|directory| directory.join(python))
            .collect()
    };
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            EngineError::ComfyUi(format!("ComfyUI Python {} was not found", python.display()))
        })
}

fn collect_python_library_roots(root: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for library_root in [root.join("lib"), root.join("lib64")] {
        let Ok(entries) = fs::read_dir(&library_root) else {
            continue;
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("python"))
            {
                output.push(entry.path());
            }
        }
    }
    let windows_packages = root.join("Lib/site-packages");
    if windows_packages.is_dir() {
        output.push(windows_packages);
    }
    Ok(())
}

fn insert_non_overlapping_root(roots: &mut Vec<PathBuf>, candidate: PathBuf) {
    if roots.iter().any(|root| candidate.starts_with(root)) {
        return;
    }
    roots.retain(|root| !root.starts_with(&candidate));
    roots.push(candidate);
}

fn sha256_json(value: &impl serde::Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(value)?;
    Ok(format!("{:x}", sha2::Sha256::digest(bytes)))
}

enum WorkerRead {
    Line(String),
    Eof,
    Error(String),
}

fn read_worker_stdout(stdout: SandboxedChildStdout, sender: mpsc::SyncSender<WorkerRead>) {
    let mut stdout = BufReader::new(stdout);
    loop {
        match read_bounded_line(&mut stdout, MAX_WORKER_RESPONSE_LINE_BYTES) {
            Ok(Some(line)) => {
                if sender.send(WorkerRead::Line(line)).is_err() {
                    return;
                }
            }
            Ok(None) => {
                let _ = sender.send(WorkerRead::Eof);
                return;
            }
            Err(error) => {
                let _ = sender.send(WorkerRead::Error(format!(
                    "reading ComfyUI worker stdout failed: {error}"
                )));
                return;
            }
        }
    }
}

fn read_bounded_line(reader: &mut impl BufRead, maximum: usize) -> std::io::Result<Option<String>> {
    let mut bytes = Vec::new();
    loop {
        let (chunk, done) = {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                if bytes.is_empty() {
                    return Ok(None);
                }
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "worker protocol line ended without a newline",
                ));
            }
            let length = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |index| index + 1);
            (available[..length].to_vec(), available[length - 1] == b'\n')
        };
        if bytes.len().saturating_add(chunk.len()) > maximum {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "worker protocol line exceeded its bound",
            ));
        }
        reader.consume(chunk.len());
        bytes.extend_from_slice(&chunk);
        if done {
            return String::from_utf8(bytes)
                .map(Some)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error));
        }
    }
}

fn read_worker_stderr(mut stderr: SandboxedChildStderr, tail: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0_u8; 4096];
    loop {
        match stderr.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                let mut tail = match tail.lock() {
                    Ok(tail) => tail,
                    Err(_) => return,
                };
                if read >= WORKER_STDERR_TAIL_BYTES {
                    tail.clear();
                    tail.extend_from_slice(&chunk[read - WORKER_STDERR_TAIL_BYTES..read]);
                    continue;
                }
                let overflow = tail
                    .len()
                    .saturating_add(read)
                    .saturating_sub(WORKER_STDERR_TAIL_BYTES);
                if overflow > 0 {
                    tail.drain(..overflow);
                }
                tail.extend_from_slice(&chunk[..read]);
            }
        }
    }
}

fn worker_stderr_text(tail: &Arc<Mutex<Vec<u8>>>) -> String {
    tail.lock()
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .unwrap_or_default()
}
