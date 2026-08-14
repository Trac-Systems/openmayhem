use super::{
    validate_load_config, verify_artifact, ArtifactChunk, ArtifactSink, CancellationToken,
    EngineBackend, EngineError, GenerateOutput, GenerateRequest, LoadConfig, LoadedModelInfo,
    Result, TokenSink, Tokenization, WorkflowGenerationOutput, WorkflowGenerationRequest,
    WorkflowProgressEvent,
};
use base64::Engine as _;
use flate2::read::GzDecoder;
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
use std::path::{Component, Path, PathBuf};
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
const WORKER_PROTOCOL_PREFIX: &str = "__mayhem_comfyui_worker_v1__";
const PYTHON_ENV: &str = "MAYHEM_COMFYUI_PYTHON";
const DEVICE_ENV: &str = "MAYHEM_COMFYUI_DEVICE";
const ARTIFACT_CHUNK_BYTES: usize = 256 * 1024;
const WORKER_STDERR_TAIL_BYTES: usize = 64 * 1024;
const MAX_WORKER_REQUEST_LINE_BYTES: usize = 64 * 1024 * 1024;
const MAX_WORKER_RESPONSE_LINE_BYTES: usize = 192 * 1024 * 1024;
const LOAD_TIMEOUT: Duration = Duration::from_secs(60);
const SANDBOX_HELPER_ENV: &str = "MAYHEM_ENCLAVE_SANDBOX_HELPER";
const MAX_CUSTOM_NODE_ARCHIVE_ENTRIES: usize = 10_000;
const MAX_CUSTOM_NODE_ARCHIVE_FILE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_CUSTOM_NODE_ARCHIVE_EXPANDED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CUSTOM_NODE_ARCHIVE_PATH_BYTES: usize = 512;

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
        materialize_comfyui_model_files(&cache_root, &config.comfyui_model_files)?;
        materialize_comfyui_custom_nodes(&cache_root, &config.comfyui_custom_nodes)?;
        let python = env::var_os(PYTHON_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("python3"));
        let device = env::var(DEVICE_ENV).unwrap_or_else(|_| default_comfyui_device().to_owned());
        let socket_dir = short_socket_dir();
        let custom_node_whitelist = config
            .comfyui_custom_nodes
            .iter()
            .map(|node| comfyui_custom_node_dir_string(&node.node_dir))
            .collect::<Result<Vec<_>>>()?;
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
                "custom_node_whitelist": custom_node_whitelist,
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
        let input_files = request
            .input_files
            .iter()
            .map(|file| {
                json!({
                    "filename": file.filename,
                    "content_type": file.content_type,
                    "data_base64": base64::engine::general_purpose::STANDARD.encode(&file.bytes),
                })
            })
            .collect::<Vec<_>>();
        self.worker()?.send(
            id,
            "run_workflow",
            json!({
                "workflow": request.workflow,
                "input_files": input_files,
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

fn materialize_comfyui_model_files(
    cache_root: &Path,
    files: &[super::ComfyUiModelFile],
) -> Result<()> {
    let models_root = cache_root.join("base").join("models");
    for file in files {
        validate_comfyui_model_file(file)?;
        let target_dir = models_root.join(&file.model_subdir);
        fs::create_dir_all(&target_dir).map_err(|error| {
            EngineError::ComfyUi(format!(
                "creating ComfyUI model directory {} failed: {error}",
                target_dir.display()
            ))
        })?;
        let target = target_dir.join(&file.model_path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                EngineError::ComfyUi(format!(
                    "creating ComfyUI model directory {} failed: {error}",
                    parent.display()
                ))
            })?;
        }
        if fs::symlink_metadata(&target)
            .is_ok_and(|metadata| metadata.is_file() || metadata.file_type().is_symlink())
        {
            fs::remove_file(&target).map_err(|error| {
                EngineError::ComfyUi(format!(
                    "replacing ComfyUI model file {} failed: {error}",
                    target.display()
                ))
            })?;
        }
        fs::hard_link(&file.source, &target)
            .or_else(|_| fs::copy(&file.source, &target).map(|_| ()))
            .map_err(|error| {
                EngineError::ComfyUi(format!(
                    "materializing ComfyUI model file {} at {} failed: {error}",
                    file.source.display(),
                    target.display()
                ))
            })?;
    }
    Ok(())
}

fn materialize_comfyui_custom_nodes(
    cache_root: &Path,
    packages: &[super::ComfyUiCustomNodePackage],
) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }
    let custom_nodes_root = cache_root.join("base").join("custom_nodes");
    fs::create_dir_all(&custom_nodes_root).map_err(|error| {
        EngineError::ComfyUi(format!(
            "creating ComfyUI custom_nodes directory {} failed: {error}",
            custom_nodes_root.display()
        ))
    })?;
    for package in packages {
        validate_comfyui_custom_node_package(package)?;
        let node_dir = comfyui_custom_node_dir_string(&package.node_dir)?;
        let target = custom_nodes_root.join(&node_dir);
        if fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            fs::remove_file(&target).map_err(|error| {
                EngineError::ComfyUi(format!(
                    "removing stale ComfyUI custom node symlink {} failed: {error}",
                    target.display()
                ))
            })?;
        } else if target.is_file() {
            fs::remove_file(&target).map_err(|error| {
                EngineError::ComfyUi(format!(
                    "removing stale ComfyUI custom node file {} failed: {error}",
                    target.display()
                ))
            })?;
        } else if target.is_dir() {
            fs::remove_dir_all(&target).map_err(|error| {
                EngineError::ComfyUi(format!(
                    "removing stale ComfyUI custom node directory {} failed: {error}",
                    target.display()
                ))
            })?;
        }
        fs::create_dir_all(&target).map_err(|error| {
            EngineError::ComfyUi(format!(
                "creating ComfyUI custom node directory {} failed: {error}",
                target.display()
            ))
        })?;
        extract_comfyui_custom_node_archive(&package.source, &target)?;
        if !target.join("__init__.py").is_file() {
            return Err(EngineError::ComfyUi(format!(
                "ComfyUI custom node package {} must contain __init__.py at archive root",
                package.source.display()
            )));
        }
    }
    Ok(())
}

fn validate_comfyui_model_file(file: &super::ComfyUiModelFile) -> Result<()> {
    let metadata = fs::symlink_metadata(&file.source).map_err(|error| {
        EngineError::ComfyUi(format!(
            "inspecting ComfyUI model source {} failed: {error}",
            file.source.display()
        ))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(EngineError::ComfyUi(format!(
            "ComfyUI model source {} must be a regular non-symlink file",
            file.source.display()
        )));
    }
    if file.model_path.as_os_str().is_empty()
        || file.model_path.is_absolute()
        || file
            .model_path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(EngineError::ComfyUi(
            "ComfyUI model path must be relative under the selected models subdir".to_owned(),
        ));
    }
    if file.model_subdir.as_os_str().is_empty()
        || file.model_subdir.is_absolute()
        || file
            .model_subdir
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(EngineError::ComfyUi(
            "ComfyUI model subdir must be a relative path under models/".to_owned(),
        ));
    }
    Ok(())
}

fn validate_comfyui_custom_node_package(package: &super::ComfyUiCustomNodePackage) -> Result<()> {
    let metadata = fs::symlink_metadata(&package.source).map_err(|error| {
        EngineError::ComfyUi(format!(
            "inspecting ComfyUI custom node source {} failed: {error}",
            package.source.display()
        ))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(EngineError::ComfyUi(format!(
            "ComfyUI custom node source {} must be a regular non-symlink tar.gz file",
            package.source.display()
        )));
    }
    comfyui_custom_node_dir_string(&package.node_dir)?;
    Ok(())
}

fn comfyui_custom_node_dir_string(path: &Path) -> Result<String> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(EngineError::ComfyUi(
            "ComfyUI custom node directory must be a single relative folder name".to_owned(),
        ));
    }
    let mut components = path.components();
    let Some(Component::Normal(value)) = components.next() else {
        return Err(EngineError::ComfyUi(
            "ComfyUI custom node directory must be a normal folder name".to_owned(),
        ));
    };
    if components.next().is_some() {
        return Err(EngineError::ComfyUi(
            "ComfyUI custom node directory must not contain nested path components".to_owned(),
        ));
    }
    let value = value.to_str().ok_or_else(|| {
        EngineError::ComfyUi("ComfyUI custom node directory must be UTF-8".to_owned())
    })?;
    if !safe_comfyui_custom_node_component(value) {
        return Err(EngineError::ComfyUi(
            "ComfyUI custom node directory contains unsafe characters".to_owned(),
        ));
    }
    Ok(value.to_owned())
}

fn extract_comfyui_custom_node_archive(source: &Path, target: &Path) -> Result<()> {
    let file = fs::File::open(source).map_err(|error| {
        EngineError::ComfyUi(format!(
            "opening ComfyUI custom node archive {} failed: {error}",
            source.display()
        ))
    })?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let mut entry_count = 0_usize;
    let mut total_file_bytes = 0_u64;
    for entry in archive.entries().map_err(|error| {
        EngineError::ComfyUi(format!(
            "reading ComfyUI custom node archive {} failed: {error}",
            source.display()
        ))
    })? {
        let mut entry = entry.map_err(|error| {
            EngineError::ComfyUi(format!(
                "reading ComfyUI custom node archive entry from {} failed: {error}",
                source.display()
            ))
        })?;
        entry_count = entry_count.saturating_add(1);
        if entry_count > MAX_CUSTOM_NODE_ARCHIVE_ENTRIES {
            return Err(EngineError::ComfyUi(format!(
                "ComfyUI custom node archive {} exceeds max entry count",
                source.display()
            )));
        }
        let entry_type = entry.header().entry_type();
        if entry_type.is_pax_global_extensions() || entry_type.is_pax_local_extensions() {
            continue;
        }
        let raw_path = entry.path().map_err(|error| {
            EngineError::ComfyUi(format!(
                "reading ComfyUI custom node archive path from {} failed: {error}",
                source.display()
            ))
        })?;
        let relative = normalized_custom_node_archive_path(raw_path.as_ref())?;
        if entry_type.is_dir() {
            if let Some(relative) = relative {
                fs::create_dir_all(target.join(relative)).map_err(|error| {
                    EngineError::ComfyUi(format!(
                        "creating ComfyUI custom node archive directory failed: {error}"
                    ))
                })?;
            }
            continue;
        }
        if !entry_type.is_file() {
            return Err(EngineError::ComfyUi(format!(
                "ComfyUI custom node archive {} contains unsupported entry type",
                source.display()
            )));
        }
        let relative = relative.ok_or_else(|| {
            EngineError::ComfyUi("ComfyUI custom node archive has an empty file path".to_owned())
        })?;
        let declared_size = entry.header().size().map_err(|error| {
            EngineError::ComfyUi(format!(
                "reading ComfyUI custom node archive entry size failed: {error}"
            ))
        })?;
        if declared_size > MAX_CUSTOM_NODE_ARCHIVE_FILE_BYTES {
            return Err(EngineError::ComfyUi(format!(
                "ComfyUI custom node archive file {} exceeds max file bytes",
                relative.display()
            )));
        }
        total_file_bytes = total_file_bytes.saturating_add(declared_size);
        if total_file_bytes > MAX_CUSTOM_NODE_ARCHIVE_EXPANDED_BYTES {
            return Err(EngineError::ComfyUi(format!(
                "ComfyUI custom node archive {} exceeds max expanded bytes",
                source.display()
            )));
        }
        let destination = target.join(&relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                EngineError::ComfyUi(format!(
                    "creating ComfyUI custom node archive parent {} failed: {error}",
                    parent.display()
                ))
            })?;
        }
        let mut output = fs::File::create(&destination).map_err(|error| {
            EngineError::ComfyUi(format!(
                "creating ComfyUI custom node file {} failed: {error}",
                destination.display()
            ))
        })?;
        let copied = std::io::copy(&mut entry, &mut output).map_err(|error| {
            EngineError::ComfyUi(format!(
                "extracting ComfyUI custom node file {} failed: {error}",
                relative.display()
            ))
        })?;
        if copied != declared_size {
            return Err(EngineError::ComfyUi(format!(
                "ComfyUI custom node archive file {} length mismatch",
                relative.display()
            )));
        }
    }
    Ok(())
}

fn normalized_custom_node_archive_path(path: &Path) -> Result<Option<PathBuf>> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(EngineError::ComfyUi(
            "ComfyUI custom node archive path must be relative".to_owned(),
        ));
    }
    if path.as_os_str().as_encoded_bytes().len() > MAX_CUSTOM_NODE_ARCHIVE_PATH_BYTES {
        return Err(EngineError::ComfyUi(
            "ComfyUI custom node archive path exceeds max bytes".to_owned(),
        ));
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let value = value.to_str().ok_or_else(|| {
                    EngineError::ComfyUi(
                        "ComfyUI custom node archive path must be UTF-8".to_owned(),
                    )
                })?;
                if !safe_comfyui_custom_node_component(value) {
                    return Err(EngineError::ComfyUi(
                        "ComfyUI custom node archive path contains unsafe characters".to_owned(),
                    ));
                }
                out.push(value);
            }
            Component::CurDir => {}
            _ => {
                return Err(EngineError::ComfyUi(
                    "ComfyUI custom node archive path cannot contain parent or prefix components"
                        .to_owned(),
                ))
            }
        }
    }
    Ok((!out.as_os_str().is_empty()).then_some(out))
}

fn safe_comfyui_custom_node_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !matches!(value, "." | ".." | ".git" | ".env" | "__pycache__")
        && (!value.starts_with('.')
            || matches!(
                value,
                ".editorconfig" | ".gitattributes" | ".github" | ".gitignore"
            ))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
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
        if let Some(helper) = env::var_os(SANDBOX_HELPER_ENV) {
            command.sandbox_helper(PathBuf::from(helper));
        }
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
        parse_worker_protocol_line(&line)
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

fn parse_worker_protocol_line(line: &str) -> Result<Option<WorkerMessage>> {
    let line = line.trim_end_matches(['\r', '\n']);
    let Some(payload) = line.strip_prefix(WORKER_PROTOCOL_PREFIX) else {
        return Ok(None);
    };
    serde_json::from_str(payload).map(Some).map_err(|error| {
        EngineError::ComfyUi(format!(
            "decoding ComfyUI worker protocol line failed: {error}"
        ))
    })
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

fn default_comfyui_device() -> &'static str {
    "auto"
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Cursor;

    fn test_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_custom_node_archive(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        for (name, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, *name, Cursor::new(*bytes))
                .unwrap();
        }
        archive.finish().unwrap();
    }

    fn write_custom_node_archive_with_global_pax(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let pax = b"21 comment=mayhem-test\n";
        let mut pax_header = tar::Header::new_gnu();
        pax_header.set_entry_type(tar::EntryType::XGlobalHeader);
        pax_header.set_size(pax.len() as u64);
        pax_header.set_mode(0o644);
        pax_header.set_cksum();
        archive
            .append_data(&mut pax_header, "pax_global_header", Cursor::new(pax))
            .unwrap();
        for (name, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, *name, Cursor::new(*bytes))
                .unwrap();
        }
        archive.finish().unwrap();
    }

    #[test]
    fn comfyui_device_defaults_to_runtime_auto_selection() {
        assert_eq!(default_comfyui_device(), "auto");
    }

    #[test]
    fn worker_protocol_ignores_custom_node_stdout_noise() {
        assert!(
            parse_worker_protocol_line("LongCat loading model on cuda\n")
                .unwrap()
                .is_none()
        );
        let message = parse_worker_protocol_line(
            "__mayhem_comfyui_worker_v1__{\"id\":7,\"ok\":true,\"result\":{\"ready\":true}}\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(message.id, 7);
        assert!(message.ok);
        assert_eq!(message.result.unwrap()["ready"], true);
    }

    #[test]
    fn worker_protocol_rejects_malformed_framed_reply() {
        let error =
            parse_worker_protocol_line("__mayhem_comfyui_worker_v1__not-json\n").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("decoding ComfyUI worker protocol line failed"),
            "{error}"
        );
    }

    #[test]
    fn custom_node_archives_materialize_under_whitelisted_directory() {
        let temp = test_temp_dir("mayhem-comfy-custom-node");
        let archive = temp.join("node.tar.gz");
        write_custom_node_archive(
            &archive,
            &[
                ("__init__.py", b"NODE_CLASS_MAPPINGS = {}\n"),
                ("nodes.py", b"class Example: pass\n"),
                ("web/main.js", b"export default {};\n"),
            ],
        );
        let cache = temp.join("cache");

        materialize_comfyui_custom_nodes(
            &cache,
            &[super::super::ComfyUiCustomNodePackage {
                source: archive,
                node_dir: PathBuf::from("ComfyUI-TestNode"),
            }],
        )
        .unwrap();

        assert!(cache
            .join("base/custom_nodes/ComfyUI-TestNode/__init__.py")
            .is_file());
        assert!(cache
            .join("base/custom_nodes/ComfyUI-TestNode/web/main.js")
            .is_file());
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn custom_node_archives_accept_global_pax_metadata() {
        let temp = test_temp_dir("mayhem-comfy-custom-node-pax");
        let archive = temp.join("node.tar.gz");
        write_custom_node_archive_with_global_pax(
            &archive,
            &[
                ("__init__.py", b"NODE_CLASS_MAPPINGS = {}\n"),
                ("nodes.py", b"class Example: pass\n"),
            ],
        );
        let cache = temp.join("cache");

        materialize_comfyui_custom_nodes(
            &cache,
            &[super::super::ComfyUiCustomNodePackage {
                source: archive,
                node_dir: PathBuf::from("ComfyUI-TestNode"),
            }],
        )
        .unwrap();

        assert!(cache
            .join("base/custom_nodes/ComfyUI-TestNode/__init__.py")
            .is_file());
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn custom_node_archives_allow_harmless_dotfiles() {
        let temp = test_temp_dir("mayhem-comfy-custom-node-dotfiles");
        let archive = temp.join("node.tar.gz");
        write_custom_node_archive(
            &archive,
            &[
                ("__init__.py", b"NODE_CLASS_MAPPINGS = {}\n"),
                (".gitignore", b"*.pyc\n"),
                (".github/workflows/publish.yml", b"name: publish\n"),
            ],
        );
        let cache = temp.join("cache");

        materialize_comfyui_custom_nodes(
            &cache,
            &[super::super::ComfyUiCustomNodePackage {
                source: archive,
                node_dir: PathBuf::from("ComfyUI-TestNode"),
            }],
        )
        .unwrap();

        assert!(cache
            .join("base/custom_nodes/ComfyUI-TestNode/.gitignore")
            .is_file());
        assert!(cache
            .join("base/custom_nodes/ComfyUI-TestNode/.github/workflows/publish.yml")
            .is_file());
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn custom_node_archives_reject_unsafe_paths() {
        let temp = test_temp_dir("mayhem-comfy-custom-node-escape");
        let archive = temp.join("node.tar.gz");
        write_custom_node_archive(&archive, &[(".git/config", b"bad\n")]);
        let cache = temp.join("cache");

        let error = materialize_comfyui_custom_nodes(
            &cache,
            &[super::super::ComfyUiCustomNodePackage {
                source: archive,
                node_dir: PathBuf::from("ComfyUI-TestNode"),
            }],
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("archive path contains unsafe characters"),
            "{error}"
        );
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn custom_node_archives_reject_sensitive_dotfiles() {
        let temp = test_temp_dir("mayhem-comfy-custom-node-dotenv");
        let archive = temp.join("node.tar.gz");
        write_custom_node_archive(&archive, &[(".env", b"TOKEN=bad\n")]);
        let cache = temp.join("cache");

        let error = materialize_comfyui_custom_nodes(
            &cache,
            &[super::super::ComfyUiCustomNodePackage {
                source: archive,
                node_dir: PathBuf::from("ComfyUI-TestNode"),
            }],
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("archive path contains unsafe characters"),
            "{error}"
        );
        let _ = fs::remove_dir_all(temp);
    }
}
