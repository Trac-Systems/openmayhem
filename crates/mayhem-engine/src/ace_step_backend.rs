use super::{
    validate_load_config, verify_artifact, ArtifactChunk, ArtifactFormat, ArtifactSink,
    CancellationToken, EngineBackend, EngineError, GenerateOutput, GenerateRequest, LoadConfig,
    LoadedModelInfo, MediaGenerationOutput, MediaGenerationRequest, MediaGenerationValidation,
    Result, TokenSink, Tokenization,
};
use base64::Engine as _;
use flate2::read::GzDecoder;
use mayhem_enclave::{
    SandboxConfig, SandboxedChild, SandboxedChildStderr, SandboxedChildStdin, SandboxedChildStdout,
    SandboxedCommand, SandboxedStderr,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const WORKER: &str = include_str!("ace_step_worker.py");
const WORKER_STDIN_BOOTSTRAP: &str = concat!(
    "import base64,sys;",
    "exec(compile(base64.b64decode(sys.stdin.buffer.readline()),",
    "'<mayhem-ace-step-worker>','exec'))"
);
const SOURCE_ARCHIVE: &[u8] = include_bytes!("../resources/ace-step-v0.1.8.tar.gz");
const SOURCE_TOP_LEVEL: &str = "ACE-Step-1.5-v0.1.8";
const SOURCE_CACHE_NAMESPACE: &str = "ace-step-source";
const SOURCE_COMPLETE_MARKER: &str = ".mayhem-complete";
const MAX_SOURCE_STAGING_ATTEMPTS: usize = 1_024;
static SOURCE_STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const PYTHON_ENV: &str = "MAYHEM_ACE_STEP_PYTHON";
const MAX_ARCHIVE_ENTRIES: usize = 20_000;
const MAX_EXTRACTED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_INPUT_AUDIO_BYTES: usize = 64 * 1024 * 1024;
const MAX_OUTPUT_AUDIO_BYTES: usize = 512 * 1024 * 1024;
const MAX_AUDIO_DURATION_SECONDS: usize = 600;
const AUDIO_CODES_PER_SECOND: usize = 5;
const MAX_AUDIO_CODES: usize = MAX_AUDIO_DURATION_SECONDS * AUDIO_CODES_PER_SECOND;
const MAX_AUDIO_CODE_BYTES: usize = 64 * 1_024;
const WORKER_STDERR_TAIL_BYTES: usize = 64 * 1_024;
const MAX_BATCH_SIZE: u32 = 8;
const CPU_OFFLOAD_THRESHOLD_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const DIT_OFFLOAD_THRESHOLD_BYTES: u64 = 12 * 1024 * 1024 * 1024;
const MEMORY_CALIBRATION: &str = "acestep/gpu_config.py:v0.1.8-tier-vram-calibration";
const ENDPOINT_MAYHEM_MUSIC_GENERATIONS: &str = "mayhem_music_generations";
const ENDPOINT_MAYHEM_AUDIO_GENERATIONS: &str = "mayhem_audio_generations";
const ENDPOINT_HF_TEXT_TO_AUDIO: &str = "hf_text_to_audio";

pub const ACE_STEP_SOURCE_SHA256: &str =
    "816a58b7cdc66b3817625dd67e7407b77c0d05e8526a70f6a43cd93889655080";
pub const ACE_STEP_SOURCE_COMMIT: &str = "dce621408bee8c31b4fcf4811682eb9359e1bc94";

pub fn ensure_ace_step_source(cache_root: &Path) -> Result<PathBuf> {
    let actual = format!("{:x}", Sha256::digest(SOURCE_ARCHIVE));
    if actual != ACE_STEP_SOURCE_SHA256 {
        return Err(EngineError::AceStep(format!(
            "embedded ACE-Step source archive hash mismatch: expected {ACE_STEP_SOURCE_SHA256}, got {actual}"
        )));
    }

    fs::create_dir_all(cache_root).map_err(|error| {
        EngineError::AceStep(format!(
            "creating ACE-Step cache root {} failed: {error}",
            cache_root.display()
        ))
    })?;
    let namespace = cache_root.join(SOURCE_CACHE_NAMESPACE);
    fs::create_dir_all(&namespace).map_err(|error| {
        EngineError::AceStep(format!(
            "creating ACE-Step source cache {} failed: {error}",
            namespace.display()
        ))
    })?;
    let destination = namespace.join(ACE_STEP_SOURCE_SHA256);
    if destination.exists() {
        return validate_cached_source(&destination);
    }

    let staging = create_source_staging_directory(&namespace)?;

    let extraction = extract_source_archive(SOURCE_ARCHIVE, &staging).and_then(|()| {
        fs::write(
            staging.join(SOURCE_COMPLETE_MARKER),
            format!("{ACE_STEP_SOURCE_SHA256}\n{ACE_STEP_SOURCE_COMMIT}\n"),
        )
        .map_err(|error| {
            EngineError::AceStep(format!(
                "writing ACE-Step source completion marker failed: {error}"
            ))
        })
    });
    if let Err(error) = extraction {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }

    if let Err(error) = fs::rename(&staging, &destination) {
        if destination.exists() {
            let _ = fs::remove_dir_all(&staging);
            return validate_cached_source(&destination);
        }
        let _ = fs::remove_dir_all(&staging);
        return Err(EngineError::AceStep(format!(
            "publishing ACE-Step source cache {} failed: {error}",
            destination.display()
        )));
    }
    validate_cached_source(&destination)
}

fn create_source_staging_directory(namespace: &Path) -> Result<PathBuf> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    create_source_staging_directory_with(
        namespace,
        std::process::id(),
        nonce,
        &SOURCE_STAGING_SEQUENCE,
    )
}

fn create_source_staging_directory_with(
    namespace: &Path,
    process_id: u32,
    nonce: u128,
    sequence: &AtomicU64,
) -> Result<PathBuf> {
    for _ in 0..MAX_SOURCE_STAGING_ATTEMPTS {
        let sequence = sequence.fetch_add(1, Ordering::Relaxed);
        let staging = namespace.join(format!(
            ".{ACE_STEP_SOURCE_SHA256}.{process_id}.{nonce}.{sequence}.tmp"
        ));
        match fs::create_dir(&staging) {
            Ok(()) => return Ok(staging),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(EngineError::AceStep(format!(
                    "creating ACE-Step source staging directory {} failed: {error}",
                    staging.display()
                )));
            }
        }
    }
    Err(EngineError::AceStep(format!(
        "creating a unique ACE-Step source staging directory under {} failed after {MAX_SOURCE_STAGING_ATTEMPTS} collisions",
        namespace.display()
    )))
}

fn validate_cached_source(destination: &Path) -> Result<PathBuf> {
    let destination_metadata = fs::symlink_metadata(destination).map_err(|error| {
        EngineError::AceStep(format!(
            "reading ACE-Step source cache {} failed: {error}",
            destination.display()
        ))
    })?;
    if !destination_metadata.is_dir() || destination_metadata.file_type().is_symlink() {
        return Err(EngineError::AceStep(format!(
            "ACE-Step source cache {} is not a real directory",
            destination.display()
        )));
    }
    let marker = destination.join(SOURCE_COMPLETE_MARKER);
    let marker_metadata = fs::symlink_metadata(&marker).map_err(|error| {
        EngineError::AceStep(format!(
            "ACE-Step source cache marker {} is missing or unreadable: {error}",
            marker.display()
        ))
    })?;
    if !marker_metadata.is_file() || marker_metadata.file_type().is_symlink() {
        return Err(EngineError::AceStep(format!(
            "ACE-Step source cache marker {} is not a regular file",
            marker.display()
        )));
    }
    let expected_marker = format!("{ACE_STEP_SOURCE_SHA256}\n{ACE_STEP_SOURCE_COMMIT}\n");
    let actual_marker = fs::read_to_string(&marker).map_err(|error| {
        EngineError::AceStep(format!(
            "ACE-Step source cache marker {} is missing or unreadable: {error}",
            marker.display()
        ))
    })?;
    if actual_marker != expected_marker {
        return Err(EngineError::AceStep(format!(
            "ACE-Step source cache marker {} is invalid",
            marker.display()
        )));
    }
    verify_extracted_source(SOURCE_ARCHIVE, destination)?;
    let source_root = destination.join(SOURCE_TOP_LEVEL);
    for required in ["pyproject.toml", "uv.lock"] {
        let path = source_root.join(required);
        if !path.is_file() {
            return Err(EngineError::AceStep(format!(
                "ACE-Step source cache is missing {}",
                path.display()
            )));
        }
    }
    Ok(source_root)
}

fn verify_extracted_source(bytes: &[u8], destination: &Path) -> Result<()> {
    let decoder = GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    let mut expected_paths = HashSet::new();
    let mut entry_count = 0_usize;
    let mut extracted_bytes = 0_u64;

    for entry in archive.entries().map_err(ace_step_archive_error)? {
        let mut entry = entry.map_err(ace_step_archive_error)?;
        entry_count = entry_count.saturating_add(1);
        if entry_count > MAX_ARCHIVE_ENTRIES {
            return Err(EngineError::AceStep(
                "ACE-Step source archive contains too many entries".to_owned(),
            ));
        }
        let entry_type = entry.header().entry_type();
        if entry_type.is_pax_global_extensions() {
            continue;
        }
        let archive_path = entry.path().map_err(ace_step_archive_error)?.into_owned();
        let normalized = validate_archive_path(&archive_path)?;
        add_expected_path_and_parents(&mut expected_paths, &normalized);
        let cached_path = destination.join(&archive_path);
        let metadata = fs::symlink_metadata(&cached_path).map_err(|error| {
            EngineError::AceStep(format!(
                "ACE-Step source cache is missing {}: {error}",
                cached_path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(EngineError::AceStep(format!(
                "ACE-Step source cache path {} is a symbolic link",
                cached_path.display()
            )));
        }

        if entry_type.is_dir() {
            if !metadata.is_dir() {
                return Err(EngineError::AceStep(format!(
                    "ACE-Step source cache path {} is not a directory",
                    cached_path.display()
                )));
            }
            continue;
        }
        if !entry_type.is_file() || !metadata.is_file() {
            return Err(EngineError::AceStep(format!(
                "ACE-Step source cache path {} is not the expected regular file",
                cached_path.display()
            )));
        }

        let size = entry.header().size().map_err(ace_step_archive_error)?;
        extracted_bytes = extracted_bytes.saturating_add(size);
        if extracted_bytes > MAX_EXTRACTED_BYTES {
            return Err(EngineError::AceStep(
                "ACE-Step source archive expands beyond the allowed size".to_owned(),
            ));
        }
        if metadata.len() != size {
            return Err(EngineError::AceStep(format!(
                "ACE-Step source cache file {} has size {}, expected {size}",
                cached_path.display(),
                metadata.len()
            )));
        }
        let mut expected = Vec::with_capacity(size as usize);
        entry
            .read_to_end(&mut expected)
            .map_err(ace_step_archive_error)?;
        let actual = fs::read(&cached_path).map_err(|error| {
            EngineError::AceStep(format!(
                "reading ACE-Step source cache file {} failed: {error}",
                cached_path.display()
            ))
        })?;
        if actual != expected {
            return Err(EngineError::AceStep(format!(
                "ACE-Step source cache file {} differs from the embedded archive",
                cached_path.display()
            )));
        }
    }

    validate_cache_tree(destination, destination, &expected_paths)
}

fn add_expected_path_and_parents(expected_paths: &mut HashSet<String>, normalized: &str) {
    let mut current = Path::new(normalized);
    loop {
        if let Some(path) = current.to_str() {
            expected_paths.insert(path.to_owned());
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent.as_os_str().is_empty() {
            break;
        }
        current = parent;
    }
}

fn validate_cache_tree(
    root: &Path,
    current: &Path,
    expected_paths: &HashSet<String>,
) -> Result<()> {
    let entries = fs::read_dir(current).map_err(|error| {
        EngineError::AceStep(format!(
            "reading ACE-Step source cache directory {} failed: {error}",
            current.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            EngineError::AceStep(format!(
                "reading ACE-Step source cache entry failed: {error}"
            ))
        })?;
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|_| {
            EngineError::AceStep(format!(
                "ACE-Step source cache path {} escaped its root",
                path.display()
            ))
        })?;
        let normalized = relative
            .components()
            .map(|component| match component {
                Component::Normal(value) => value.to_str().ok_or_else(|| {
                    EngineError::AceStep(format!(
                        "ACE-Step source cache path {} is not UTF-8",
                        path.display()
                    ))
                }),
                _ => Err(EngineError::AceStep(format!(
                    "ACE-Step source cache path {} is invalid",
                    path.display()
                ))),
            })
            .collect::<Result<Vec<_>>>()?
            .join("/");
        if normalized == SOURCE_COMPLETE_MARKER {
            continue;
        }
        if !expected_paths.contains(&normalized) {
            return Err(EngineError::AceStep(format!(
                "ACE-Step source cache contains unexpected path {}",
                path.display()
            )));
        }
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            EngineError::AceStep(format!(
                "reading ACE-Step source cache path {} failed: {error}",
                path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(EngineError::AceStep(format!(
                "ACE-Step source cache path {} is a symbolic link",
                path.display()
            )));
        }
        if metadata.is_dir() {
            validate_cache_tree(root, &path, expected_paths)?;
        } else if !metadata.is_file() {
            return Err(EngineError::AceStep(format!(
                "ACE-Step source cache path {} is not a regular file or directory",
                path.display()
            )));
        }
    }
    Ok(())
}

fn extract_source_archive(bytes: &[u8], staging: &Path) -> Result<()> {
    let decoder = GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    let mut exact_paths = HashSet::new();
    let mut folded_paths = HashSet::new();
    let mut entry_count = 0_usize;
    let mut extracted_bytes = 0_u64;

    for entry in archive.entries().map_err(ace_step_archive_error)? {
        let mut entry = entry.map_err(ace_step_archive_error)?;
        entry_count = entry_count.saturating_add(1);
        if entry_count > MAX_ARCHIVE_ENTRIES {
            return Err(EngineError::AceStep(
                "ACE-Step source archive contains too many entries".to_owned(),
            ));
        }
        let entry_type = entry.header().entry_type();
        if entry_type.is_pax_global_extensions() {
            continue;
        }
        let path = entry.path().map_err(ace_step_archive_error)?.into_owned();
        let normalized = validate_archive_path(&path)?;
        if !exact_paths.insert(normalized.clone()) {
            return Err(EngineError::AceStep(format!(
                "ACE-Step source archive contains duplicate path {normalized}"
            )));
        }
        let folded = normalized.to_lowercase();
        if !folded_paths.insert(folded) {
            return Err(EngineError::AceStep(format!(
                "ACE-Step source archive contains a case-colliding path {normalized}"
            )));
        }

        if !entry_type.is_file() && !entry_type.is_dir() {
            return Err(EngineError::AceStep(format!(
                "ACE-Step source archive path {normalized} is a link or special file"
            )));
        }
        let destination = staging.join(&path);
        if entry_type.is_dir() {
            fs::create_dir_all(&destination).map_err(|error| {
                EngineError::AceStep(format!(
                    "creating extracted ACE-Step directory {} failed: {error}",
                    destination.display()
                ))
            })?;
            continue;
        }

        let size = entry.header().size().map_err(ace_step_archive_error)?;
        extracted_bytes = extracted_bytes.saturating_add(size);
        if extracted_bytes > MAX_EXTRACTED_BYTES {
            return Err(EngineError::AceStep(
                "ACE-Step source archive expands beyond the allowed size".to_owned(),
            ));
        }
        let parent = destination.parent().ok_or_else(|| {
            EngineError::AceStep(format!(
                "ACE-Step source archive path {normalized} has no parent"
            ))
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            EngineError::AceStep(format!(
                "creating extracted ACE-Step parent {} failed: {error}",
                parent.display()
            ))
        })?;
        let mut output = fs::File::create(&destination).map_err(|error| {
            EngineError::AceStep(format!(
                "creating extracted ACE-Step file {} failed: {error}",
                destination.display()
            ))
        })?;
        let copied = std::io::copy(&mut entry, &mut output).map_err(|error| {
            EngineError::AceStep(format!(
                "extracting ACE-Step file {} failed: {error}",
                destination.display()
            ))
        })?;
        if copied != size {
            return Err(EngineError::AceStep(format!(
                "ACE-Step source archive file {normalized} had size {copied}, expected {size}"
            )));
        }
        let executable = entry.header().mode().unwrap_or(0) & 0o111 != 0;
        set_extracted_permissions(&destination, false, executable)?;
    }

    if entry_count == 0 {
        return Err(EngineError::AceStep(
            "ACE-Step source archive is empty".to_owned(),
        ));
    }
    harden_extracted_directories(&staging.join(SOURCE_TOP_LEVEL))?;
    Ok(())
}

fn harden_extracted_directories(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        EngineError::AceStep(format!(
            "reading extracted ACE-Step path {} failed: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(EngineError::AceStep(format!(
            "extracted ACE-Step path {} is a symbolic link",
            path.display()
        )));
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|error| {
            EngineError::AceStep(format!(
                "reading extracted ACE-Step directory {} failed: {error}",
                path.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                EngineError::AceStep(format!("reading extracted ACE-Step entry failed: {error}"))
            })?;
            harden_extracted_directories(&entry.path())?;
        }
        set_extracted_permissions(path, true, false)?;
    }
    Ok(())
}

fn validate_archive_path(path: &Path) -> Result<String> {
    let raw = path.to_str().ok_or_else(|| {
        EngineError::AceStep("ACE-Step source archive contains a non-UTF-8 path".to_owned())
    })?;
    let raw = raw.strip_suffix('/').unwrap_or(raw);
    if raw.starts_with('/')
        || raw.is_empty()
        || raw
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(EngineError::AceStep(format!(
            "ACE-Step source archive path {} contains an absolute, parent, current, or empty component",
            path.display()
        )));
    }
    let mut components = path.components();
    let Some(Component::Normal(top_level)) = components.next() else {
        return Err(EngineError::AceStep(
            "ACE-Step source archive contains an absolute or invalid path".to_owned(),
        ));
    };
    if top_level != SOURCE_TOP_LEVEL {
        return Err(EngineError::AceStep(format!(
            "ACE-Step source archive path {} is outside {SOURCE_TOP_LEVEL}",
            path.display()
        )));
    }
    for component in components {
        if !matches!(component, Component::Normal(_)) {
            return Err(EngineError::AceStep(format!(
                "ACE-Step source archive path {} contains an absolute, parent, or current component",
                path.display()
            )));
        }
    }
    Ok(raw.to_owned())
}

#[cfg(unix)]
fn set_extracted_permissions(path: &Path, directory: bool, executable: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = if directory || executable {
        0o555
    } else {
        0o444
    };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|error| {
        EngineError::AceStep(format!(
            "setting extracted ACE-Step permissions on {} failed: {error}",
            path.display()
        ))
    })
}

#[cfg(not(unix))]
fn set_extracted_permissions(path: &Path, _directory: bool, _executable: bool) -> Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

fn ace_step_archive_error(error: std::io::Error) -> EngineError {
    EngineError::AceStep(format!(
        "reading embedded ACE-Step source archive failed: {error}"
    ))
}

pub struct AceStepBackend {
    python: PathBuf,
    worker: Option<AceStepWorker>,
    loaded: Option<LoadedModelInfo>,
    config: Option<LoadConfig>,
    source_root: Option<PathBuf>,
    model_root: Option<PathBuf>,
    worker_cache: Option<PathBuf>,
    execution_config: Option<AceStepExecutionConfig>,
    next_id: u64,
    next_artifact_id: u64,
}

impl AceStepBackend {
    pub fn new() -> Result<Self> {
        let python = env::var_os(PYTHON_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("python3"));
        Self::with_python(python)
    }

    pub fn with_python(python: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self {
            python: python.into(),
            worker: None,
            loaded: None,
            config: None,
            source_root: None,
            model_root: None,
            worker_cache: None,
            execution_config: None,
            next_id: 1,
            next_artifact_id: 1,
        })
    }

    fn ensure_worker_loaded(&mut self) -> Result<()> {
        if self.worker.is_some() {
            return Ok(());
        }
        let config = self.config.clone().ok_or(EngineError::NotLoaded)?;
        let source_root = self
            .source_root
            .clone()
            .ok_or_else(|| EngineError::AceStep("ACE-Step source is not prepared".to_owned()))?;
        let model_root = self.model_root.clone().ok_or_else(|| {
            EngineError::AceStep("ACE-Step model root is not prepared".to_owned())
        })?;
        let worker_cache = self.worker_cache.clone().ok_or_else(|| {
            EngineError::AceStep("ACE-Step worker cache is not prepared".to_owned())
        })?;
        self.worker = Some(AceStepWorker::spawn(
            &self.python,
            config.memory_limit_bytes,
            &worker_cache,
            &source_root,
            &model_root,
        )?);
        let worker_info: WorkerLoadInfo = self.call_existing(
            "load",
            json!({
                "model_root": model_root,
                "source_root": source_root,
                "worker_cache": worker_cache,
            }),
            None,
        )?;
        validate_execution_config(&worker_info.execution_config)?;
        self.execution_config = Some(worker_info.execution_config);
        Ok(())
    }

    fn call<T>(
        &mut self,
        operation: &str,
        payload: Value,
        cancellation: Option<&CancellationToken>,
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        self.ensure_worker_loaded()?;
        self.call_existing(operation, payload, cancellation)
    }

    fn call_existing<T>(
        &mut self,
        operation: &str,
        payload: Value,
        cancellation: Option<&CancellationToken>,
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.worker_mut()?.send(id, operation, payload)?;
        loop {
            if cancellation.is_some_and(CancellationToken::is_cancelled) {
                self.stop_worker();
                return Err(EngineError::Cancelled);
            }
            let message = match self.worker_mut()?.read_message(Duration::from_millis(25)) {
                Ok(Some(message)) => message,
                Ok(None) => continue,
                Err(error) => {
                    self.stop_worker();
                    return Err(error);
                }
            };
            if message.id != id {
                self.stop_worker();
                return Err(EngineError::AceStep(format!(
                    "worker response id {} did not match request id {id}",
                    message.id
                )));
            }
            if message.ok {
                return serde_json::from_value(message.result.unwrap_or(Value::Null)).map_err(
                    |error| {
                        self.stop_worker();
                        EngineError::AceStep(format!(
                            "decoding ACE-Step worker response failed: {error}"
                        ))
                    },
                );
            }
            return Err(EngineError::AceStep(
                message
                    .error
                    .unwrap_or_else(|| "worker returned an unknown error".to_owned()),
            ));
        }
    }

    fn worker_mut(&mut self) -> Result<&mut AceStepWorker> {
        self.worker
            .as_mut()
            .ok_or_else(|| EngineError::AceStep("ACE-Step worker is not running".to_owned()))
    }

    fn stop_worker(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            worker.stop();
        }
    }

    pub fn execution_config(&self) -> Option<&AceStepExecutionConfig> {
        self.execution_config.as_ref()
    }
}

impl EngineBackend for AceStepBackend {
    fn backend_id(&self) -> &'static str {
        "ace-step"
    }

    fn load(&mut self, config: LoadConfig) -> Result<LoadedModelInfo> {
        validate_load_config(&config)?;
        if config.artifact.format != ArtifactFormat::AceStepSafetensors {
            return Err(EngineError::InvalidConfig(format!(
                "ACE-Step requires ACE-Step safetensors artifacts, got {:?}",
                config.artifact.format
            )));
        }
        verify_artifact(&config.artifact)?;
        let model_root = ace_step_model_root(&config.artifact.path)?;
        let cache_root = ace_step_cache_root(config.backend_cache_dir.as_deref());
        let source_root = ensure_ace_step_source(&cache_root)?;
        let worker_cache = ace_step_worker_cache(&cache_root, &model_root)?;
        fs::create_dir_all(&worker_cache).map_err(|error| {
            EngineError::AceStep(format!(
                "creating ACE-Step worker cache {} failed: {error}",
                worker_cache.display()
            ))
        })?;

        self.stop_worker();
        self.config = Some(config.clone());
        self.source_root = Some(source_root.clone());
        self.model_root = Some(model_root.clone());
        self.worker_cache = Some(worker_cache.clone());
        self.execution_config = None;
        self.worker = Some(AceStepWorker::spawn(
            &self.python,
            config.memory_limit_bytes,
            &worker_cache,
            &source_root,
            &model_root,
        )?);
        let worker_info: WorkerLoadInfo = self.call_existing(
            "load",
            json!({
                "model_root": model_root,
                "source_root": source_root,
                "worker_cache": worker_cache,
            }),
            None,
        )?;
        validate_execution_config(&worker_info.execution_config)?;
        self.execution_config = Some(worker_info.execution_config);
        let loaded = LoadedModelInfo {
            backend: self.backend_id().to_owned(),
            artifact: config.artifact,
            ctx_size: config.ctx_size,
            n_ctx_train: worker_info.n_ctx_train,
            n_vocab: worker_info.n_vocab,
        };
        self.loaded = Some(loaded.clone());
        Ok(loaded)
    }

    fn loaded_backend_evidence(&self) -> Option<Value> {
        self.execution_config
            .as_ref()
            .and_then(|config| serde_json::to_value(config).ok())
    }

    fn component_healthy(&mut self) -> bool {
        match self.worker.as_mut() {
            Some(worker) => matches!(worker.child.try_wait(), Ok(None)),
            None => self.loaded.is_none(),
        }
    }

    fn process_ids(&self) -> Vec<u32> {
        self.worker
            .as_ref()
            .map(|worker| vec![worker.child.id()])
            .unwrap_or_default()
    }

    fn tokenize(&self, text: &str) -> Result<Tokenization> {
        self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
        Ok(Tokenization {
            token_ids: text
                .split_whitespace()
                .enumerate()
                .map(|(index, _)| i32::try_from(index).unwrap_or(i32::MAX))
                .collect(),
        })
    }

    fn generate(
        &mut self,
        _request: GenerateRequest,
        _sink: &mut dyn TokenSink,
        _cancellation: &CancellationToken,
    ) -> Result<GenerateOutput> {
        Err(EngineError::InvalidConfig(
            "ACE-Step generates music; use generate_music".to_owned(),
        ))
    }

    fn generate_audio(
        &mut self,
        request: MediaGenerationRequest,
        artifact_sink: &mut dyn ArtifactSink,
        cancellation: &CancellationToken,
    ) -> Result<MediaGenerationOutput> {
        self.generate_music(request, artifact_sink, cancellation)
    }

    fn generate_music(
        &mut self,
        request: MediaGenerationRequest,
        artifact_sink: &mut dyn ArtifactSink,
        cancellation: &CancellationToken,
    ) -> Result<MediaGenerationOutput> {
        if cancellation.is_cancelled() {
            self.stop_worker();
            return Err(EngineError::Cancelled);
        }
        self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
        let normalized = NormalizedRequest::from_media_request(request)?;
        let expected_content_type = content_type_for_format(&normalized.output_format);
        let expected_batch = normalized.batch_size;
        let expected_steps = normalized.inference_steps;
        let response: WorkerGenerateResult = self.call(
            "generate_music",
            normalized.worker_payload(),
            Some(cancellation),
        )?;
        if response.artifacts.len() != usize::try_from(expected_batch).unwrap_or(usize::MAX) {
            self.stop_worker();
            return Err(EngineError::AceStep(format!(
                "worker returned {} artifacts for batch size {expected_batch}",
                response.artifacts.len()
            )));
        }
        if response.step_count != u64::from(expected_steps) {
            self.stop_worker();
            return Err(EngineError::AceStep(format!(
                "worker returned step count {}, expected {expected_steps}",
                response.step_count
            )));
        }

        let mut actual_duration = 0.0_f64;
        for artifact in response.artifacts {
            if cancellation.is_cancelled() {
                self.stop_worker();
                return Err(EngineError::Cancelled);
            }
            if artifact.content_type != expected_content_type {
                self.stop_worker();
                return Err(EngineError::AceStep(format!(
                    "worker returned content type {}, expected {expected_content_type}",
                    artifact.content_type
                )));
            }
            if !artifact.duration_seconds.is_finite()
                || artifact.duration_seconds <= 0.0
                || artifact.duration_seconds > 600.0
            {
                self.stop_worker();
                return Err(EngineError::AceStep(
                    "worker returned an invalid audio duration".to_owned(),
                ));
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(artifact.data_base64)
                .map_err(|error| {
                    self.stop_worker();
                    EngineError::AceStep(format!("worker returned invalid base64 audio: {error}"))
                })?;
            if !valid_audio_byte_length(bytes.len(), MAX_OUTPUT_AUDIO_BYTES) {
                self.stop_worker();
                return Err(EngineError::AceStep(
                    "worker returned an out-of-bounds audio artifact".to_owned(),
                ));
            }
            actual_duration = actual_duration.max(artifact.duration_seconds);
            let artifact_id = format!("ace-step-{}", self.next_artifact_id);
            self.next_artifact_id = self.next_artifact_id.saturating_add(1);
            artifact_sink.on_artifact_chunk(ArtifactChunk {
                artifact_id,
                index: 0,
                content_type: expected_content_type.to_owned(),
                bytes,
                final_chunk: true,
            })?;
        }

        Ok(MediaGenerationOutput {
            duration_seconds: actual_duration.ceil() as u64,
            frame_count: 0,
            step_count: response.step_count,
        })
    }

    fn validate_media_generation(
        &mut self,
        request: MediaGenerationRequest,
        cancellation: &CancellationToken,
    ) -> Result<Option<MediaGenerationValidation>> {
        if cancellation.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        self.loaded.as_ref().ok_or(EngineError::NotLoaded)?;
        let normalized = NormalizedRequest::from_media_request(request)?;
        let evidence = self.call(
            "validate_music",
            normalized.worker_payload(),
            Some(cancellation),
        )?;
        Ok(Some(MediaGenerationValidation {
            evidence,
            handled_request_attributes: normalized.handled_request_attributes,
        }))
    }
}

impl Drop for AceStepBackend {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

fn ace_step_model_root(path: &Path) -> Result<PathBuf> {
    if !path.is_file() {
        return Err(EngineError::InvalidConfig(format!(
            "ACE-Step artifact {} must be a model.safetensors file",
            path.display()
        )));
    }
    if path.file_name().and_then(|name| name.to_str()) != Some("model.safetensors") {
        return Err(EngineError::InvalidConfig(format!(
            "ACE-Step primary artifact {} must be named model.safetensors",
            path.display()
        )));
    }
    let model_dir = path.parent().ok_or_else(|| {
        EngineError::InvalidConfig(format!(
            "ACE-Step artifact {} has no model directory",
            path.display()
        ))
    })?;
    if model_dir.file_name().and_then(|name| name.to_str()) != Some("acestep-v15-sft") {
        return Err(EngineError::InvalidConfig(format!(
            "ACE-Step artifact {} must be under acestep-v15-sft",
            path.display()
        )));
    }
    model_dir.parent().map(Path::to_path_buf).ok_or_else(|| {
        EngineError::InvalidConfig(format!(
            "ACE-Step artifact {} has no local component root",
            path.display()
        ))
    })
}

fn ace_step_cache_root(configured: Option<&Path>) -> PathBuf {
    configured
        .map(Path::to_path_buf)
        .or_else(|| {
            env::var_os("MAYHEM_HOME")
                .map(PathBuf::from)
                .map(|home| home.join("cache/ace-step"))
        })
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".mayhem/cache/ace-step"))
        })
        .unwrap_or_else(|| env::temp_dir().join("mayhem-ace-step-cache"))
}

fn ace_step_worker_cache(cache_root: &Path, model_root: &Path) -> Result<PathBuf> {
    let canonical_model_root = fs::canonicalize(model_root).map_err(|error| {
        EngineError::AceStep(format!(
            "canonicalizing ACE-Step model root {} failed: {error}",
            model_root.display()
        ))
    })?;
    let digest = Sha256::digest(canonical_model_root.to_string_lossy().as_bytes());
    Ok(cache_root.join("workers").join(format!("{digest:x}")))
}

fn ace_step_python_runtime_roots(python: &Path) -> Result<(PathBuf, Vec<PathBuf>)> {
    let python = resolve_python_program(python)?;
    let canonical_python = fs::canonicalize(&python).map_err(|error| {
        EngineError::AceStep(format!(
            "canonicalizing ACE-Step Python {} failed: {error}",
            python.display()
        ))
    })?;
    let executable_root = canonical_python.parent().ok_or_else(|| {
        EngineError::AceStep(format!(
            "ACE-Step Python {} has no runtime directory",
            canonical_python.display()
        ))
    })?;

    let mut candidates = vec![executable_root.to_path_buf()];
    let environment_root = python
        .parent()
        .filter(|parent| {
            parent
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("bin") || name == "Scripts")
        })
        .and_then(Path::parent);
    if let Some(environment_root) = environment_root {
        candidates.push(environment_root.to_path_buf());
        collect_python_library_roots(environment_root, &mut candidates)?;
        let pyvenv = environment_root.join("pyvenv.cfg");
        if pyvenv.is_file() {
            let config = fs::read_to_string(&pyvenv).map_err(|error| {
                EngineError::AceStep(format!(
                    "reading managed ACE-Step Python config {} failed: {error}",
                    pyvenv.display()
                ))
            })?;
            if let Some(home) = config.lines().find_map(|line| {
                line.split_once('=')
                    .filter(|(key, _)| key.trim().eq_ignore_ascii_case("home"))
                    .map(|(_, value)| PathBuf::from(value.trim()))
            }) {
                let base_root = if home
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("bin") || name == "Scripts")
                {
                    home.parent().unwrap_or(&home)
                } else {
                    home.as_path()
                };
                candidates.push(base_root.to_path_buf());
                collect_python_library_roots(base_root, &mut candidates)?;
            }
        }
    }

    let mut canonical_roots = Vec::new();
    for candidate in candidates {
        if !candidate.is_dir() {
            continue;
        }
        let candidate = fs::canonicalize(&candidate).map_err(|error| {
            EngineError::AceStep(format!(
                "canonicalizing ACE-Step Python runtime {} failed: {error}",
                candidate.display()
            ))
        })?;
        if !canonical_roots
            .iter()
            .any(|root: &PathBuf| root == &candidate)
        {
            canonical_roots.push(candidate);
        }
    }
    canonical_roots.sort_by(|left, right| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then_with(|| left.cmp(right))
    });
    let mut roots = Vec::new();
    for candidate in canonical_roots {
        if roots
            .iter()
            .any(|root: &PathBuf| candidate.starts_with(root))
        {
            continue;
        }
        roots.push(candidate);
    }
    if roots.is_empty() {
        return Err(EngineError::AceStep(format!(
            "ACE-Step Python {} has no readable runtime roots",
            python.display()
        )));
    }
    Ok((python, roots))
}

fn resolve_python_program(python: &Path) -> Result<PathBuf> {
    let has_directory = python.is_absolute() || python.components().count() > 1;
    let candidates = if has_directory {
        let path = if python.is_absolute() {
            python.to_path_buf()
        } else {
            env::current_dir()
                .map_err(|error| {
                    EngineError::AceStep(format!(
                        "resolving the current directory for ACE-Step Python failed: {error}"
                    ))
                })?
                .join(python)
        };
        vec![path]
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
            EngineError::AceStep(format!(
                "ACE-Step Python executable {} was not found",
                python.display()
            ))
        })
}

fn collect_python_library_roots(root: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for library_root in [root.join("lib"), root.join("lib64")] {
        let Ok(entries) = fs::read_dir(&library_root) else {
            continue;
        };
        for entry in entries {
            let entry = entry.map_err(|error| {
                EngineError::AceStep(format!(
                    "reading ACE-Step Python runtime {} failed: {error}",
                    library_root.display()
                ))
            })?;
            if entry.file_type().map_err(EngineError::Io)?.is_dir()
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

fn validate_execution_config(config: &AceStepExecutionConfig) -> Result<()> {
    if config.source_commit != ACE_STEP_SOURCE_COMMIT
        || config.memory_calibration != MEMORY_CALIBRATION
    {
        return Err(EngineError::AceStep(
            "worker execution evidence does not match the pinned ACE-Step source".to_owned(),
        ));
    }
    match config.device_kind.as_str() {
        "cuda" | "xpu" => {
            let free = config.free_memory_bytes.ok_or_else(|| {
                EngineError::AceStep(
                    "accelerator execution evidence is missing free memory".to_owned(),
                )
            })?;
            let total = config.total_memory_bytes.ok_or_else(|| {
                EngineError::AceStep(
                    "accelerator execution evidence is missing total memory".to_owned(),
                )
            })?;
            if free == 0 || total == 0 || free > total {
                return Err(EngineError::AceStep(
                    "accelerator execution evidence has invalid memory values".to_owned(),
                ));
            }
            let expected_cpu_offload = free < CPU_OFFLOAD_THRESHOLD_BYTES;
            let expected_dit_offload = free < DIT_OFFLOAD_THRESHOLD_BYTES;
            let expected_quantization =
                expected_cpu_offload.then_some("int8_weight_only".to_owned());
            if config.selection_basis != "load-time-free-accelerator-memory"
                || config.offload_to_cpu != expected_cpu_offload
                || config.offload_dit_to_cpu != expected_dit_offload
                || config.quantization != expected_quantization
            {
                return Err(EngineError::AceStep(
                    "worker execution evidence does not match free-memory policy".to_owned(),
                ));
            }
        }
        "mps" | "cpu" => {
            let expected_basis = format!("pinned-v0.1.8-{}-policy", config.device_kind);
            if config.free_memory_bytes.is_some()
                || config.total_memory_bytes.is_some()
                || config.offload_to_cpu
                || config.offload_dit_to_cpu
                || config.quantization.is_some()
                || config.selection_basis != expected_basis
            {
                return Err(EngineError::AceStep(
                    "worker execution evidence does not match the pinned non-CUDA policy"
                        .to_owned(),
                ));
            }
        }
        _ => {
            return Err(EngineError::AceStep(format!(
                "worker reported unsupported execution device {}",
                config.device_kind
            )))
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AceStepExecutionConfig {
    pub device_kind: String,
    pub free_memory_bytes: Option<u64>,
    pub total_memory_bytes: Option<u64>,
    pub offload_to_cpu: bool,
    pub offload_dit_to_cpu: bool,
    pub quantization: Option<String>,
    pub selection_basis: String,
    pub memory_calibration: String,
    pub source_commit: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerLoadInfo {
    n_ctx_train: u32,
    n_vocab: i32,
    execution_config: AceStepExecutionConfig,
}

#[derive(Debug, Deserialize)]
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
struct WorkerGenerateResult {
    artifacts: Vec<WorkerArtifact>,
    step_count: u64,
}

#[derive(Debug, Deserialize)]
struct WorkerArtifact {
    data_base64: String,
    content_type: String,
    duration_seconds: f64,
}

struct AceStepWorker {
    child: SandboxedChild,
    stdin: SandboxedChildStdin,
    stdout_rx: Option<Receiver<WorkerRead>>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    stderr_reader: Option<JoinHandle<()>>,
}

impl AceStepWorker {
    fn spawn(
        python: &Path,
        memory_limit_bytes: Option<u64>,
        cache_root: &Path,
        source_root: &Path,
        model_root: &Path,
    ) -> Result<Self> {
        let (python, runtime_roots) = ace_step_python_runtime_roots(python)?;
        let executable_runtime_roots = runtime_roots.clone();
        let mut read_only_dirs = vec![model_root.to_path_buf(), source_root.to_path_buf()];
        read_only_dirs.extend(runtime_roots);
        let sandbox = SandboxConfig::new(read_only_dirs, vec![cache_root.to_path_buf()]);
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
            EngineError::AceStep(format!(
                "starting sandboxed ACE-Step worker with {} failed: {error}",
                python.display()
            ))
        })?;
        let mut stdin = child.take_stdin().ok_or_else(|| {
            EngineError::AceStep("opening ACE-Step worker stdin failed".to_owned())
        })?;
        let worker_source = base64::engine::general_purpose::STANDARD.encode(WORKER.as_bytes());
        stdin
            .write_all(worker_source.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .map_err(|error| {
                EngineError::AceStep(format!(
                    "sending ACE-Step worker source over the private bootstrap pipe failed: {error}"
                ))
            })?;
        let stdout = child.take_stdout().ok_or_else(|| {
            EngineError::AceStep("opening ACE-Step worker stdout failed".to_owned())
        })?;
        let stderr = child.take_stderr().ok_or_else(|| {
            EngineError::AceStep("opening ACE-Step worker stderr failed".to_owned())
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
        serde_json::to_writer(
            &mut self.stdin,
            &json!({
                "id": id,
                "op": operation,
                "payload": payload,
            }),
        )?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn read_message(&mut self, wait: Duration) -> Result<Option<WorkerMessage>> {
        let read = match self
            .stdout_rx
            .as_ref()
            .ok_or_else(|| {
                EngineError::AceStep("ACE-Step worker stdout reader is closed".to_owned())
            })?
            .recv_timeout(wait)
        {
            Ok(read) => read,
            Err(RecvTimeoutError::Timeout) => return Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                return Err(EngineError::AceStep(
                    "ACE-Step worker stdout reader stopped".to_owned(),
                ))
            }
        };
        let line = match read {
            WorkerRead::Line(line) => line,
            WorkerRead::Eof => {
                return Err(self.exit_error("ACE-Step worker exited before replying"))
            }
            WorkerRead::Error(error) => return Err(EngineError::AceStep(error)),
        };
        serde_json::from_str(line.trim_end())
            .map(Some)
            .map_err(|error| {
                EngineError::AceStep(format!(
                    "decoding ACE-Step worker protocol line failed: {error}"
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
            EngineError::AceStep(format!("{message}; exit status {status}; stderr was empty"))
        } else {
            EngineError::AceStep(format!(
                "{message}; exit status {status}; stderr tail: {stderr}"
            ))
        }
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
        ("CLANG_MODULE_CACHE_PATH", cache_root.join("clang-modules")),
        ("SWIFT_MODULECACHE_PATH", cache_root.join("swift-modules")),
        ("XDG_CACHE_HOME", cache_root.join("xdg")),
        ("XDG_RUNTIME_DIR", cache_root.join("xdg-runtime")),
        ("HF_HOME", cache_root.join("huggingface")),
        ("HF_HUB_CACHE", cache_root.join("huggingface/hub")),
        ("TRANSFORMERS_CACHE", cache_root.join("transformers")),
        ("TORCH_HOME", cache_root.join("torch")),
        ("MPLCONFIGDIR", cache_root.join("matplotlib")),
        ("NUMBA_CACHE_DIR", cache_root.join("numba")),
    ] {
        fs::create_dir_all(&path).map_err(|error| {
            EngineError::AceStep(format!(
                "creating ACE-Step cache directory {} failed: {error}",
                path.display()
            ))
        })?;
        command.env(name, path);
    }
    command
        .env("HF_HUB_OFFLINE", "1")
        .env("HF_DATASETS_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .env("DIFFUSERS_OFFLINE", "1")
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
        "MAX_CUDA_VRAM",
        "MAX_MPS_VRAM",
        "MAX_XPU_VRAM",
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
        if let Ok(path) = env::join_paths(paths) {
            command.env("PATH", path);
        }
    }
    Ok(())
}

enum WorkerRead {
    Line(String),
    Eof,
    Error(String),
}

fn read_worker_stdout(stdout: SandboxedChildStdout, sender: mpsc::SyncSender<WorkerRead>) {
    let mut stdout = BufReader::new(stdout);
    loop {
        let mut line = String::new();
        match stdout.read_line(&mut line) {
            Ok(0) => {
                let _ = sender.send(WorkerRead::Eof);
                return;
            }
            Ok(_) => {
                if sender.send(WorkerRead::Line(line)).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(WorkerRead::Error(format!(
                    "reading ACE-Step worker stdout failed: {error}"
                )));
                return;
            }
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
        .map(|tail| String::from_utf8_lossy(&tail).trim().to_owned())
        .unwrap_or_else(|_| "<stderr capture unavailable>".to_owned())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMusicRequest {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    caption: Option<String>,
    #[serde(default)]
    style: Option<String>,
    #[serde(default)]
    genre: Option<String>,
    #[serde(default)]
    tags: Option<RawTags>,
    #[serde(default)]
    lyrics: Option<String>,
    #[serde(default)]
    instrumental: Option<bool>,
    #[serde(default)]
    vocal_language: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    bpm: Option<i64>,
    #[serde(default)]
    keyscale: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    timesignature: Option<String>,
    #[serde(default)]
    time_signature: Option<String>,
    #[serde(default)]
    duration: Option<Value>,
    #[serde(default)]
    audio_duration: Option<Value>,
    #[serde(default)]
    duration_seconds: Option<Value>,
    #[serde(default)]
    inference_steps: Option<u64>,
    #[serde(default)]
    steps: Option<u64>,
    #[serde(default)]
    guidance_scale: Option<f64>,
    #[serde(default)]
    seed: Option<i64>,
    #[serde(default)]
    seeds: Option<Vec<i64>>,
    #[serde(default)]
    sample_mode: Option<bool>,
    #[serde(default)]
    sample_query: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    use_format: Option<bool>,
    #[serde(default)]
    #[serde(rename = "format")]
    format_alias: Option<bool>,
    #[serde(default)]
    thinking: Option<bool>,
    #[serde(default)]
    lm_temperature: Option<f64>,
    #[serde(default)]
    temperature: Option<f64>,
    #[serde(default)]
    lm_cfg_scale: Option<f64>,
    #[serde(default)]
    lm_cfg: Option<f64>,
    #[serde(default)]
    lm_top_k: Option<u64>,
    #[serde(default)]
    top_k: Option<u64>,
    #[serde(default)]
    lm_top_p: Option<f64>,
    #[serde(default)]
    top_p: Option<f64>,
    #[serde(default)]
    lm_negative_prompt: Option<String>,
    #[serde(default)]
    negative_prompt: Option<String>,
    #[serde(default)]
    use_cot_metas: Option<bool>,
    #[serde(default)]
    cot_metas: Option<bool>,
    #[serde(default)]
    use_cot_caption: Option<bool>,
    #[serde(default)]
    cot_caption: Option<bool>,
    #[serde(default)]
    use_cot_language: Option<bool>,
    #[serde(default)]
    cot_language: Option<bool>,
    #[serde(default)]
    use_constrained_decoding: Option<bool>,
    #[serde(default)]
    constrained_decoding: Option<bool>,
    #[serde(default)]
    task_type: Option<String>,
    #[serde(default)]
    no_fsq: Option<bool>,
    #[serde(default)]
    instruction: Option<String>,
    #[serde(default)]
    source_audio: Option<RawAudio>,
    #[serde(default)]
    src_audio: Option<RawAudio>,
    #[serde(default)]
    ctx_audio: Option<RawAudio>,
    #[serde(default)]
    reference_audio: Option<RawAudio>,
    #[serde(default)]
    ref_audio: Option<RawAudio>,
    #[serde(default)]
    melody: Option<RawAudio>,
    #[serde(default)]
    input_audio: Option<RawAudio>,
    #[serde(default)]
    audio: Option<RawAudio>,
    #[serde(default)]
    audio_codes: Option<RawAudioCodes>,
    #[serde(default)]
    audio_code_string: Option<RawAudioCodes>,
    #[serde(default)]
    repainting_start: Option<f64>,
    #[serde(default)]
    repaint_start: Option<f64>,
    #[serde(default)]
    repainting_end: Option<f64>,
    #[serde(default)]
    repaint_end: Option<f64>,
    #[serde(default)]
    repaint_strength: Option<f64>,
    #[serde(default)]
    chunk_mask_mode: Option<String>,
    #[serde(default)]
    repaint_mode: Option<String>,
    #[serde(default)]
    audio_cover_strength: Option<f64>,
    #[serde(default)]
    cover_strength: Option<f64>,
    #[serde(default)]
    cover_noise_strength: Option<f64>,
    #[serde(default)]
    infer_method: Option<String>,
    #[serde(default)]
    inference_method: Option<String>,
    #[serde(default)]
    sampler: Option<String>,
    #[serde(default)]
    sampler_mode: Option<String>,
    #[serde(default)]
    velocity_norm_threshold: Option<f64>,
    #[serde(default)]
    velocity_ema_factor: Option<f64>,
    #[serde(default)]
    dcw_enabled: Option<bool>,
    #[serde(default)]
    use_dcw: Option<bool>,
    #[serde(default)]
    dcw_mode: Option<String>,
    #[serde(default)]
    dcw_scaler: Option<f64>,
    #[serde(default)]
    dcw_high_scaler: Option<f64>,
    #[serde(default)]
    dcw_wavelet: Option<String>,
    #[serde(default)]
    shift: Option<f64>,
    #[serde(default)]
    timesteps: Option<Vec<f64>>,
    #[serde(default)]
    custom_timesteps: Option<Vec<f64>>,
    #[serde(default)]
    cfg_interval_start: Option<f64>,
    #[serde(default)]
    cfg_interval_end: Option<f64>,
    #[serde(default)]
    use_adg: Option<bool>,
    #[serde(default)]
    adg: Option<bool>,
    #[serde(default)]
    enable_normalization: Option<bool>,
    #[serde(default)]
    normalization_db: Option<f64>,
    #[serde(default)]
    fade_in_duration: Option<f64>,
    #[serde(default)]
    fade_out_duration: Option<f64>,
    #[serde(default)]
    latent_shift: Option<f64>,
    #[serde(default)]
    latent_rescale: Option<f64>,
    #[serde(default)]
    retake_seed: Option<i64>,
    #[serde(default)]
    retake_variance: Option<f64>,
    #[serde(default)]
    flow_edit_morph: Option<bool>,
    #[serde(default)]
    flow_edit: Option<bool>,
    #[serde(default)]
    flow_edit_source_caption: Option<String>,
    #[serde(default)]
    flow_edit_source_lyrics: Option<String>,
    #[serde(default)]
    flow_edit_n_min: Option<f64>,
    #[serde(default)]
    flow_edit_n_max: Option<f64>,
    #[serde(default)]
    flow_edit_n_avg: Option<u64>,
    #[serde(default)]
    batch_size: Option<u64>,
    #[serde(default)]
    batch: Option<u64>,
    #[serde(default)]
    n: Option<u64>,
    #[serde(default)]
    response_format: Option<String>,
    #[serde(default)]
    audio_format: Option<String>,
    #[serde(default)]
    mp3_bitrate: Option<String>,
    #[serde(default)]
    mp3_sample_rate: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawTags {
    String(String),
    List(Vec<String>),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum RawAudioCodes {
    String(String),
    List(Vec<String>),
}

impl RawAudioCodes {
    fn is_empty(&self) -> bool {
        match self {
            Self::String(value) => value.is_empty(),
            Self::List(values) => values.is_empty(),
        }
    }

    fn batch_len(&self) -> Option<usize> {
        match self {
            Self::String(_) => None,
            Self::List(values) => Some(values.len()),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawAudio {
    Base64(String),
    Descriptor(RawAudioDescriptor),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAudioDescriptor {
    encoding: String,
    data: String,
    content_type: String,
}

struct NormalizedRequest {
    params: Value,
    preprocess: Value,
    config: Value,
    source_audio: Value,
    reference_audio: Value,
    output_format: String,
    batch_size: u32,
    inference_steps: u32,
    handled_request_attributes: BTreeSet<String>,
}

impl NormalizedRequest {
    fn from_media_request(request: MediaGenerationRequest) -> Result<Self> {
        let handled_request_attributes = ace_source_request_attributes(&request.request);
        let request = canonicalize_ace_media_request(request)?;
        validate_ace_media_request(&request)?;
        let raw: RawMusicRequest = serde_json::from_value(request.request).map_err(|error| {
            EngineError::InvalidConfig(format!("invalid ACE-Step request payload: {error}"))
        })?;
        let steps_were_supplied = raw.inference_steps.is_some() || raw.steps.is_some();
        let shift_was_supplied = raw.shift.is_some();
        let repaint_strength_was_supplied = raw.repaint_strength.is_some();
        let dcw_controls_were_supplied = raw.dcw_mode.is_some()
            || raw.dcw_scaler.is_some()
            || raw.dcw_high_scaler.is_some()
            || raw.dcw_wavelet.is_some();
        let dcw_high_scaler_was_supplied = raw.dcw_high_scaler.is_some();
        let normalization_db_was_supplied = raw.normalization_db.is_some();
        if raw
            .model
            .as_deref()
            .is_some_and(|model| model.trim().is_empty() || model.len() > 512)
        {
            return invalid("model must be a non-empty string of at most 512 bytes");
        }
        if let Some(prompt) = raw.prompt.as_deref() {
            if prompt != request.prompt {
                return invalid("request prompt does not match the media generation prompt");
            }
        }
        if raw.prompt.is_some() && raw.caption.is_some() {
            return invalid("prompt and caption aliases cannot both be supplied");
        }
        let base_caption = raw
            .caption
            .as_deref()
            .or(raw.prompt.as_deref())
            .unwrap_or(&request.prompt)
            .trim();
        validate_text("prompt/caption", base_caption, 0, 512)?;
        let style = normalize_optional_text("style", raw.style.as_deref(), 505)?;
        let genre = normalize_optional_text("genre", raw.genre.as_deref(), 505)?;
        let tags = normalize_tags(raw.tags)?;
        let caption = compose_caption(base_caption, style.as_deref(), genre.as_deref(), &tags)?;
        let mut lyrics = raw.lyrics.unwrap_or_default();
        validate_text("lyrics", &lyrics, 0, 4096)?;
        let instrumental = raw.instrumental.unwrap_or(false);
        if instrumental {
            let supplied = lyrics.trim();
            if !supplied.is_empty() && supplied != "[Instrumental]" {
                return invalid("instrumental=true cannot be combined with vocal lyrics");
            }
            lyrics = "[Instrumental]".to_owned();
        }
        let vocal_language = one_alias(
            "vocal_language/language",
            [
                ("vocal_language", raw.vocal_language),
                ("language", raw.language),
            ],
        )?
        .unwrap_or_else(|| "unknown".to_owned());
        if !VALID_LANGUAGES.contains(&vocal_language.as_str()) {
            return invalid("vocal_language is not supported by ACE-Step v0.1.8");
        }
        let bpm = raw
            .bpm
            .map(|value| {
                if (30..=300).contains(&value) {
                    Ok(value)
                } else {
                    invalid("bpm must be between 30 and 300")
                }
            })
            .transpose()?;
        let keyscale = one_alias(
            "keyscale/key",
            [("keyscale", raw.keyscale), ("key", raw.key)],
        )?
        .unwrap_or_default();
        validate_keyscale(&keyscale)?;
        let timesignature = one_alias(
            "timesignature/time_signature",
            [
                ("timesignature", raw.timesignature),
                ("time_signature", raw.time_signature),
            ],
        )?
        .map(normalize_timesignature)
        .transpose()?
        .unwrap_or_default();
        let duration = normalize_duration(
            one_alias(
                "duration/audio_duration",
                [
                    ("duration", raw.duration),
                    ("audio_duration", raw.audio_duration),
                ],
            )?,
            raw.duration_seconds,
            request.duration_seconds,
        )?;
        let raw_steps = one_alias(
            "inference_steps/steps",
            [
                ("inference_steps", raw.inference_steps),
                ("steps", raw.steps),
            ],
        )?;
        if let (Some(raw_steps), Some(declared_steps)) = (raw_steps, request.step_count) {
            if raw_steps != declared_steps {
                return invalid(
                    "inference_steps does not match media generation step_count metadata",
                );
            }
        }
        let mut inference_steps = normalize_u32(
            "inference_steps",
            raw_steps.or(request.step_count),
            50,
            1,
            200,
        )?;
        let guidance_scale = bounded_f64(
            "guidance_scale",
            raw.guidance_scale.unwrap_or(7.0),
            1.0,
            15.0,
        )?;
        let seed = raw.seed.unwrap_or(-1);
        if !(-1..=i64::from(u32::MAX)).contains(&seed) {
            return invalid("seed must be -1 or an unsigned 32-bit integer");
        }
        if raw.seed.is_some() && raw.seeds.is_some() {
            return invalid("seed and seeds cannot both be supplied");
        }
        let thinking = raw.thinking.unwrap_or(false);
        let lm_temperature_value = one_alias(
            "lm_temperature/temperature",
            [
                ("lm_temperature", raw.lm_temperature),
                ("temperature", raw.temperature),
            ],
        )?;
        let lm_cfg_scale_value = one_alias(
            "lm_cfg_scale/lm_cfg",
            [("lm_cfg_scale", raw.lm_cfg_scale), ("lm_cfg", raw.lm_cfg)],
        )?;
        let lm_top_k_value = one_alias(
            "lm_top_k/top_k",
            [("lm_top_k", raw.lm_top_k), ("top_k", raw.top_k)],
        )?;
        let lm_top_p_value = one_alias(
            "lm_top_p/top_p",
            [("lm_top_p", raw.lm_top_p), ("top_p", raw.top_p)],
        )?;
        let lm_temperature = bounded_f64(
            "lm_temperature",
            lm_temperature_value.unwrap_or(0.85),
            0.0,
            2.0,
        )?;
        let lm_cfg_scale =
            bounded_f64("lm_cfg_scale", lm_cfg_scale_value.unwrap_or(2.0), 0.0, 15.0)?;
        let lm_top_k = normalize_u32("lm_top_k", lm_top_k_value, 0, 0, 1000)?;
        let lm_top_p = bounded_f64("lm_top_p", lm_top_p_value.unwrap_or(0.9), 0.0, 1.0)?;
        let lm_negative_prompt = one_alias(
            "lm_negative_prompt/negative_prompt",
            [
                ("lm_negative_prompt", raw.lm_negative_prompt),
                ("negative_prompt", raw.negative_prompt),
            ],
        )?
        .unwrap_or_else(|| "NO USER INPUT".to_owned());
        validate_text("lm_negative_prompt", &lm_negative_prompt, 0, 1024)?;
        let mut task_type = raw.task_type.unwrap_or_else(|| "text2music".to_owned());
        if raw.no_fsq.unwrap_or(false) {
            if task_type == "cover" {
                task_type = "cover-nofsq".to_owned();
            } else if task_type != "cover-nofsq" {
                return invalid("no_fsq=true requires task_type=cover or cover-nofsq");
            }
        }
        if !matches!(
            task_type.as_str(),
            "text2music" | "cover" | "cover-nofsq" | "repaint"
        ) {
            return invalid(
                "task_type must be text2music, cover, cover-nofsq, or repaint for the SFT checkpoint",
            );
        }
        let sample_mode = raw.sample_mode.unwrap_or(false);
        let sample_query = normalize_optional_text(
            "sample_query",
            one_alias(
                "sample_query/description/desc",
                [
                    ("sample_query", raw.sample_query),
                    ("description", raw.description),
                    ("desc", raw.desc),
                ],
            )?
            .as_deref(),
            512,
        )?;
        let use_format = one_alias(
            "use_format/format",
            [("use_format", raw.use_format), ("format", raw.format_alias)],
        )?
        .unwrap_or(false);
        if (sample_mode || sample_query.is_some() || use_format) && task_type != "text2music" {
            return invalid(
                "sample_mode, sample_query, and use_format are supported only for text2music",
            );
        }
        if use_format && caption.is_empty() && lyrics.is_empty() {
            return invalid("use_format requires caption or lyrics");
        }
        if task_type == "text2music"
            && caption.is_empty()
            && lyrics.trim().is_empty()
            && !sample_mode
            && sample_query.is_none()
        {
            return invalid("text2music requires caption, lyrics, or sample mode input");
        }
        let instruction = raw.instruction.unwrap_or_else(|| {
            match task_type.as_str() {
                "repaint" => "Repaint the mask area based on the given conditions:",
                "cover" | "cover-nofsq" => {
                    "Generate audio semantic tokens based on the given conditions:"
                }
                _ => "Fill the audio semantic mask based on the given conditions:",
            }
            .to_owned()
        });
        validate_text("instruction", &instruction, 1, 1024)?;

        let flow_controls_supplied = raw.flow_edit_source_caption.is_some()
            || raw.flow_edit_source_lyrics.is_some()
            || raw.flow_edit_n_min.is_some()
            || raw.flow_edit_n_max.is_some()
            || raw.flow_edit_n_avg.is_some();
        let flow_edit_morph = one_alias(
            "flow_edit_morph/flow_edit",
            [
                ("flow_edit_morph", raw.flow_edit_morph),
                ("flow_edit", raw.flow_edit),
            ],
        )?
        .unwrap_or(false);
        let source_audio = normalize_audio(
            one_alias(
                "source_audio/src_audio/ctx_audio/input_audio/audio",
                [
                    ("source_audio", raw.source_audio),
                    ("src_audio", raw.src_audio),
                    ("ctx_audio", raw.ctx_audio),
                    ("input_audio", raw.input_audio),
                    ("audio", raw.audio),
                ],
            )?,
            "source_audio",
        )?;
        let reference_audio = normalize_audio(
            one_alias(
                "reference_audio/ref_audio/melody",
                [
                    ("reference_audio", raw.reference_audio),
                    ("ref_audio", raw.ref_audio),
                    ("melody", raw.melody),
                ],
            )?,
            "reference_audio",
        )?;
        let audio_codes = one_alias(
            "audio_codes/audio_code_string",
            [
                ("audio_codes", raw.audio_codes),
                ("audio_code_string", raw.audio_code_string),
            ],
        )?
        .unwrap_or_else(|| RawAudioCodes::String(String::new()));
        validate_audio_codes_value(&audio_codes)?;
        let has_audio_codes = !audio_codes.is_empty();
        let has_source_audio = !source_audio.is_null();
        if task_type == "text2music" && has_audio_codes {
            return invalid("audio_codes require an explicit cover or cover-nofsq task");
        }
        if task_type == "repaint" && has_audio_codes {
            return invalid("audio_codes are not supported for repaint");
        }
        if has_audio_codes && has_source_audio {
            return invalid(
                "audio_codes and source_audio cannot be combined because ACE-Step would ignore source_audio",
            );
        }
        if flow_edit_morph && has_audio_codes {
            return invalid("audio_codes cannot be combined with flow_edit_morph");
        }
        if matches!(task_type.as_str(), "cover" | "cover-nofsq" | "repaint")
            && !has_source_audio
            && !has_audio_codes
        {
            return invalid(format!(
                "{task_type} requires bounded base64 source_audio or validated audio_codes"
            ));
        }
        if task_type == "text2music" && has_source_audio && !flow_edit_morph {
            return invalid(
                "plain text2music does not consume source_audio; enable flow editing or choose a source-driven task",
            );
        }
        if thinking && matches!(task_type.as_str(), "cover" | "cover-nofsq" | "repaint") {
            return invalid("thinking is not consumed by cover or repaint tasks");
        }
        if (sample_mode || sample_query.is_some() || use_format) && has_audio_codes {
            return invalid(
                "sample_mode, sample_query, and use_format cannot be combined with audio_codes",
            );
        }
        let repainting_start = bounded_f64(
            "repainting_start",
            one_alias(
                "repainting_start/repaint_start",
                [
                    ("repainting_start", raw.repainting_start),
                    ("repaint_start", raw.repaint_start),
                ],
            )?
            .unwrap_or(0.0),
            0.0,
            600.0,
        )?;
        let repainting_end = one_alias(
            "repainting_end/repaint_end",
            [
                ("repainting_end", raw.repainting_end),
                ("repaint_end", raw.repaint_end),
            ],
        )?
        .unwrap_or(-1.0);
        if !repainting_end.is_finite()
            || (repainting_end != -1.0
                && (!(0.0..=600.0).contains(&repainting_end) || repainting_end <= repainting_start))
        {
            return invalid(
                "repainting_end must be -1 or greater than repainting_start and at most 600",
            );
        }
        let repaint_strength = bounded_f64(
            "repaint_strength",
            raw.repaint_strength.unwrap_or(0.5),
            0.0,
            1.0,
        )?;
        let chunk_mask_mode = raw.chunk_mask_mode.unwrap_or_else(|| "auto".to_owned());
        if !matches!(chunk_mask_mode.as_str(), "auto" | "explicit") {
            return invalid("chunk_mask_mode must be auto or explicit");
        }
        let repaint_mode = raw.repaint_mode.unwrap_or_else(|| "balanced".to_owned());
        if !matches!(
            repaint_mode.as_str(),
            "conservative" | "balanced" | "aggressive"
        ) {
            return invalid("repaint_mode must be conservative, balanced, or aggressive");
        }
        if repaint_strength_was_supplied && repaint_mode != "balanced" {
            return invalid("repaint_strength is active only when repaint_mode=balanced");
        }
        let audio_cover_strength = bounded_f64(
            "audio_cover_strength",
            one_alias(
                "audio_cover_strength/cover_strength",
                [
                    ("audio_cover_strength", raw.audio_cover_strength),
                    ("cover_strength", raw.cover_strength),
                ],
            )?
            .unwrap_or(1.0),
            0.0,
            1.0,
        )?;
        let cover_noise_strength = bounded_f64(
            "cover_noise_strength",
            raw.cover_noise_strength.unwrap_or(0.0),
            0.0,
            1.0,
        )?;
        let infer_method = one_alias(
            "infer_method/inference_method",
            [
                ("infer_method", raw.infer_method),
                ("inference_method", raw.inference_method),
            ],
        )?
        .unwrap_or_else(|| "ode".to_owned());
        if !matches!(infer_method.as_str(), "ode" | "sde") {
            return invalid("infer_method must be ode or sde");
        }
        let sampler_mode = one_alias(
            "sampler/sampler_mode",
            [("sampler", raw.sampler), ("sampler_mode", raw.sampler_mode)],
        )?
        .unwrap_or_else(|| "euler".to_owned());
        if !matches!(sampler_mode.as_str(), "euler" | "heun") {
            return invalid("sampler_mode must be euler or heun");
        }
        if infer_method == "sde" && sampler_mode == "heun" {
            return invalid("sampler=heun is not honored with infer_method=sde");
        }
        let velocity_norm_threshold = bounded_f64(
            "velocity_norm_threshold",
            raw.velocity_norm_threshold.unwrap_or(0.0),
            0.0,
            5.0,
        )?;
        let velocity_ema_factor = bounded_f64(
            "velocity_ema_factor",
            raw.velocity_ema_factor.unwrap_or(0.0),
            0.0,
            0.5,
        )?;
        let dcw_enabled = one_alias(
            "dcw_enabled/use_dcw",
            [("dcw_enabled", raw.dcw_enabled), ("use_dcw", raw.use_dcw)],
        )?
        .unwrap_or(false);
        let dcw_mode = raw.dcw_mode.unwrap_or_else(|| "double".to_owned());
        if !matches!(dcw_mode.as_str(), "low" | "high" | "double" | "pix") {
            return invalid("dcw_mode must be low, high, double, or pix");
        }
        if !dcw_enabled && dcw_controls_were_supplied {
            return invalid("DCW controls require dcw_enabled=true");
        }
        if dcw_high_scaler_was_supplied && dcw_mode != "double" {
            return invalid("dcw_high_scaler is active only when dcw_mode=double");
        }
        let default_dcw_scaler = if thinking { 0.02 } else { 0.05 };
        let default_dcw_high_scaler = if thinking { 0.06 } else { 0.02 };
        let dcw_scaler = bounded_f64(
            "dcw_scaler",
            raw.dcw_scaler.unwrap_or(default_dcw_scaler),
            0.0,
            0.1,
        )?;
        let dcw_high_scaler = bounded_f64(
            "dcw_high_scaler",
            raw.dcw_high_scaler.unwrap_or(default_dcw_high_scaler),
            0.0,
            0.1,
        )?;
        let dcw_wavelet = raw.dcw_wavelet.unwrap_or_else(|| "haar".to_owned());
        if !matches!(
            dcw_wavelet.as_str(),
            "haar" | "db2" | "db4" | "sym4" | "sym8" | "coif2"
        ) {
            return invalid("dcw_wavelet is not one of the pinned runtime choices");
        }
        let mut shift = bounded_f64("shift", raw.shift.unwrap_or(3.0), 1.0, 5.0)?;
        let timesteps = one_alias(
            "timesteps/custom_timesteps",
            [
                ("timesteps", raw.timesteps),
                ("custom_timesteps", raw.custom_timesteps),
            ],
        )?;
        validate_timesteps(timesteps.as_deref())?;
        if let Some(timesteps) = timesteps.as_ref() {
            if steps_were_supplied || shift_was_supplied {
                return invalid(
                    "custom_timesteps overrides steps and shift; do not supply them together",
                );
            }
            inference_steps = u32::try_from(timesteps.len() - 1).map_err(|_| {
                EngineError::InvalidConfig("ACE-Step timestep count exceeds u32".to_owned())
            })?;
            shift = 1.0;
        }
        let expected_inference_steps = effective_ace_step_count(
            inference_steps,
            shift,
            timesteps.as_deref(),
            cover_noise_strength,
        )?;
        let cfg_interval_start = bounded_f64(
            "cfg_interval_start",
            raw.cfg_interval_start.unwrap_or(0.0),
            0.0,
            1.0,
        )?;
        let cfg_interval_end = bounded_f64(
            "cfg_interval_end",
            raw.cfg_interval_end.unwrap_or(1.0),
            0.0,
            1.0,
        )?;
        if cfg_interval_start > cfg_interval_end {
            return invalid("cfg_interval_start must not exceed cfg_interval_end");
        }
        let enable_normalization = raw.enable_normalization.unwrap_or(true);
        if !enable_normalization && normalization_db_was_supplied {
            return invalid("normalization_db requires enable_normalization=true");
        }
        let normalization_db = bounded_f64(
            "normalization_db",
            raw.normalization_db.unwrap_or(-1.0),
            -10.0,
            0.0,
        )?;
        let fade_in_duration = bounded_f64(
            "fade_in_duration",
            raw.fade_in_duration.unwrap_or(0.0),
            0.0,
            10.0,
        )?;
        let fade_out_duration = bounded_f64(
            "fade_out_duration",
            raw.fade_out_duration.unwrap_or(0.0),
            0.0,
            10.0,
        )?;
        if duration > 0.0 && fade_in_duration + fade_out_duration > duration {
            return invalid("fade durations must not exceed the requested duration");
        }
        let latent_shift = bounded_f64("latent_shift", raw.latent_shift.unwrap_or(0.0), -0.2, 0.2)?;
        let latent_rescale = bounded_f64(
            "latent_rescale",
            raw.latent_rescale.unwrap_or(1.0),
            0.5,
            1.5,
        )?;
        let retake_variance = bounded_f64(
            "retake_variance",
            raw.retake_variance.unwrap_or(0.0),
            0.0,
            1.0,
        )?;
        let retake_seed = raw.retake_seed;
        if let Some(retake_seed) = retake_seed {
            if !(-1..=i64::from(u32::MAX)).contains(&retake_seed) {
                return invalid("retake_seed must be -1 or an unsigned 32-bit integer");
            }
            if retake_variance == 0.0 {
                return invalid("retake_seed requires retake_variance greater than zero");
            }
        }
        let flow_edit_source_caption = raw.flow_edit_source_caption.unwrap_or_default();
        validate_text(
            "flow_edit_source_caption",
            &flow_edit_source_caption,
            0,
            512,
        )?;
        let flow_edit_source_lyrics = raw.flow_edit_source_lyrics.unwrap_or_default();
        validate_text("flow_edit_source_lyrics", &flow_edit_source_lyrics, 0, 4096)?;
        let flow_edit_n_min = bounded_f64(
            "flow_edit_n_min",
            raw.flow_edit_n_min.unwrap_or(0.0),
            0.0,
            1.0,
        )?;
        let flow_edit_n_max = bounded_f64(
            "flow_edit_n_max",
            raw.flow_edit_n_max.unwrap_or(1.0),
            0.0,
            1.0,
        )?;
        if flow_edit_n_min > flow_edit_n_max {
            return invalid("flow_edit_n_min must not exceed flow_edit_n_max");
        }
        let flow_edit_n_avg = normalize_u32("flow_edit_n_avg", raw.flow_edit_n_avg, 1, 1, 8)?;
        if !flow_edit_morph && flow_controls_supplied {
            return invalid("flow-edit controls require flow_edit_morph=true");
        }
        if flow_edit_morph {
            if !matches!(task_type.as_str(), "text2music" | "cover" | "cover-nofsq") {
                return invalid("flow_edit_morph supports text2music and cover tasks only");
            }
            if source_audio.is_null() {
                return invalid("flow_edit_morph requires bounded base64 source_audio");
            }
        }
        let batch_size = normalize_u32(
            "batch_size",
            one_alias(
                "batch_size/batch/n",
                [
                    ("batch_size", raw.batch_size),
                    ("batch", raw.batch),
                    ("n", raw.n),
                ],
            )?,
            2,
            1,
            MAX_BATCH_SIZE,
        )?;
        if audio_codes
            .batch_len()
            .is_some_and(|length| length != batch_size as usize)
        {
            return invalid(
                "audio_codes arrays must contain exactly one code string per batch item",
            );
        }
        let seeds = raw
            .seeds
            .map(|seeds| validate_batch_seeds(seeds, batch_size))
            .transpose()?;
        let raw_output_format = one_alias(
            "response_format/audio_format",
            [
                ("response_format", raw.response_format),
                ("audio_format", raw.audio_format),
            ],
        )?;
        let output_format = normalize_output_format(
            raw_output_format.as_deref(),
            request.response_format.as_deref(),
        )?;
        let mp3_controls_supplied = raw.mp3_bitrate.is_some() || raw.mp3_sample_rate.is_some();
        let mp3_bitrate = raw.mp3_bitrate.unwrap_or_else(|| "128k".to_owned());
        if !matches!(mp3_bitrate.as_str(), "128k" | "192k" | "256k" | "320k") {
            return invalid("mp3_bitrate must be 128k, 192k, 256k, or 320k");
        }
        let mp3_sample_rate = normalize_u32(
            "mp3_sample_rate",
            raw.mp3_sample_rate,
            48_000,
            44_100,
            48_000,
        )?;
        if !matches!(mp3_sample_rate, 44_100 | 48_000) {
            return invalid("mp3_sample_rate must be 44100 or 48000");
        }
        if output_format != "mp3" && mp3_controls_supplied {
            return invalid("MP3 controls require response_format=mp3");
        }
        let use_adg = one_alias("use_adg/adg", [("use_adg", raw.use_adg), ("adg", raw.adg)])?
            .unwrap_or(false);
        let lm_path_allowed =
            !matches!(task_type.as_str(), "cover" | "cover-nofsq" | "repaint") && !flow_edit_morph;
        let use_cot_metas = one_alias(
            "use_cot_metas/cot_metas",
            [
                ("use_cot_metas", raw.use_cot_metas),
                ("cot_metas", raw.cot_metas),
            ],
        )?
        .unwrap_or(lm_path_allowed && !(sample_mode || sample_query.is_some()));
        let use_cot_caption = one_alias(
            "use_cot_caption/cot_caption",
            [
                ("use_cot_caption", raw.use_cot_caption),
                ("cot_caption", raw.cot_caption),
            ],
        )?
        .unwrap_or(lm_path_allowed);
        let use_cot_language = one_alias(
            "use_cot_language/cot_language",
            [
                ("use_cot_language", raw.use_cot_language),
                ("cot_language", raw.cot_language),
            ],
        )?
        .unwrap_or(lm_path_allowed && vocal_language == "unknown");
        let use_constrained_decoding = one_alias(
            "use_constrained_decoding/constrained_decoding",
            [
                ("use_constrained_decoding", raw.use_constrained_decoding),
                ("constrained_decoding", raw.constrained_decoding),
            ],
        )?
        .unwrap_or(lm_path_allowed);
        if !lm_path_allowed
            && (thinking
                || use_cot_metas
                || use_cot_caption
                || use_cot_language
                || use_constrained_decoding)
        {
            return invalid("LM and CoT controls are not consumed by source or flow-edit tasks");
        }
        if vocal_language != "unknown" && use_cot_language {
            return invalid(
                "use_cot_language=true would replace the explicit vocal_language with LM output",
            );
        }
        if flow_edit_morph {
            if infer_method != "ode" {
                return invalid("flow editing v1 does not honor infer_method=sde");
            }
            if sampler_mode != "euler" {
                return invalid("flow editing v1 does not honor sampler=heun");
            }
            if use_adg {
                return invalid("flow editing v1 bypasses ADG");
            }
            if dcw_enabled {
                return invalid("flow editing v1 bypasses DCW");
            }
        }

        let effective_seed = seeds
            .as_ref()
            .and_then(|seeds| seeds.first())
            .copied()
            .unwrap_or(seed);
        let params = Value::Object(
            [
                ("task_type", json!(task_type)),
                ("instruction", json!(instruction)),
                ("audio_codes", json!(audio_codes)),
                ("caption", json!(caption)),
                ("lyrics", json!(lyrics)),
                ("instrumental", json!(instrumental)),
                ("vocal_language", json!(vocal_language)),
                ("bpm", json!(bpm)),
                ("keyscale", json!(keyscale)),
                ("timesignature", json!(timesignature)),
                ("duration", json!(duration)),
                ("inference_steps", json!(inference_steps)),
                ("seed", json!(effective_seed)),
                ("guidance_scale", json!(guidance_scale)),
                ("enable_normalization", json!(enable_normalization)),
                ("normalization_db", json!(normalization_db)),
                ("fade_in_duration", json!(fade_in_duration)),
                ("fade_out_duration", json!(fade_out_duration)),
                ("latent_shift", json!(latent_shift)),
                ("latent_rescale", json!(latent_rescale)),
                ("use_adg", json!(use_adg)),
                ("cfg_interval_start", json!(cfg_interval_start)),
                ("cfg_interval_end", json!(cfg_interval_end)),
                ("shift", json!(shift)),
                ("infer_method", json!(infer_method)),
                ("sampler_mode", json!(sampler_mode)),
                ("velocity_norm_threshold", json!(velocity_norm_threshold)),
                ("velocity_ema_factor", json!(velocity_ema_factor)),
                ("dcw_enabled", json!(dcw_enabled)),
                ("dcw_mode", json!(dcw_mode)),
                ("dcw_scaler", json!(dcw_scaler)),
                ("dcw_high_scaler", json!(dcw_high_scaler)),
                ("dcw_wavelet", json!(dcw_wavelet)),
                ("timesteps", json!(timesteps)),
                ("repainting_start", json!(repainting_start)),
                ("repainting_end", json!(repainting_end)),
                ("chunk_mask_mode", json!(chunk_mask_mode)),
                ("repaint_mode", json!(repaint_mode)),
                ("repaint_strength", json!(repaint_strength)),
                ("audio_cover_strength", json!(audio_cover_strength)),
                ("cover_noise_strength", json!(cover_noise_strength)),
                ("retake_seed", json!(retake_seed)),
                ("retake_variance", json!(retake_variance)),
                ("flow_edit_morph", json!(flow_edit_morph)),
                ("flow_edit_source_caption", json!(flow_edit_source_caption)),
                ("flow_edit_source_lyrics", json!(flow_edit_source_lyrics)),
                ("flow_edit_n_min", json!(flow_edit_n_min)),
                ("flow_edit_n_max", json!(flow_edit_n_max)),
                ("flow_edit_n_avg", json!(flow_edit_n_avg)),
                ("thinking", json!(thinking)),
                ("lm_temperature", json!(lm_temperature)),
                ("lm_cfg_scale", json!(lm_cfg_scale)),
                ("lm_top_k", json!(lm_top_k)),
                ("lm_top_p", json!(lm_top_p)),
                ("lm_negative_prompt", json!(lm_negative_prompt)),
                ("use_cot_metas", json!(use_cot_metas)),
                ("use_cot_caption", json!(use_cot_caption)),
                ("use_cot_language", json!(use_cot_language)),
                ("use_constrained_decoding", json!(use_constrained_decoding)),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
        );
        let preprocess = json!({
            "sample_mode": sample_mode,
            "sample_query": sample_query,
            "use_format": use_format,
        });
        let config = json!({
            "batch_size": batch_size,
            "allow_lm_batch": batch_size > 1,
            "use_random_seed": seeds.is_none() && seed == -1,
            "seeds": seeds.or_else(|| (seed != -1).then_some(vec![seed])),
            "audio_format": output_format,
            "mp3_bitrate": mp3_bitrate,
            "mp3_sample_rate": mp3_sample_rate,
        });
        Ok(Self {
            params,
            preprocess,
            config,
            source_audio,
            reference_audio,
            output_format,
            batch_size,
            inference_steps: expected_inference_steps,
            handled_request_attributes,
        })
    }

    fn worker_payload(&self) -> Value {
        json!({
            "params": self.params,
            "preprocess": self.preprocess,
            "config": self.config,
            "source_audio": self.source_audio,
            "reference_audio": self.reference_audio,
        })
    }
}

fn ace_source_request_attributes(request: &Value) -> BTreeSet<String> {
    fn collect(value: &Value, prefix: &str, paths: &mut BTreeSet<String>) {
        let Value::Object(object) = value else {
            return;
        };
        for (name, child) in object {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}.{name}")
            };
            paths.insert(path.clone());
            collect(child, &path, paths);
        }
    }

    let mut paths = BTreeSet::new();
    collect(request, "", &mut paths);
    paths
}

fn canonicalize_ace_media_request(
    mut request: MediaGenerationRequest,
) -> Result<MediaGenerationRequest> {
    let Some(body) = request.request.as_object() else {
        return invalid("media generation request must be an object");
    };
    let mut canonical = serde_json::Map::new();
    match request.endpoint_family.as_str() {
        ENDPOINT_MAYHEM_MUSIC_GENERATIONS => return Ok(request),
        ENDPOINT_MAYHEM_AUDIO_GENERATIONS => {
            reject_unknown_fields(
                body,
                &[
                    "model",
                    "prompt",
                    "duration_seconds",
                    "response_format",
                    "guidance_scale",
                    "seed",
                ],
                ENDPOINT_MAYHEM_AUDIO_GENERATIONS,
            )?;
            copy_fields(
                body,
                &mut canonical,
                &[
                    "model",
                    "prompt",
                    "duration_seconds",
                    "response_format",
                    "guidance_scale",
                    "seed",
                ],
            );
        }
        ENDPOINT_HF_TEXT_TO_AUDIO => {
            reject_unknown_fields(body, &["inputs", "parameters"], ENDPOINT_HF_TEXT_TO_AUDIO)?;
            if let Some(inputs) = body.get("inputs") {
                canonical.insert("prompt".to_owned(), inputs.clone());
            }
            if let Some(parameters) = body.get("parameters") {
                let parameters = parameters.as_object().ok_or_else(|| {
                    EngineError::InvalidConfig(
                        "hf_text_to_audio parameters must be an object".to_owned(),
                    )
                })?;
                reject_unknown_fields(
                    parameters,
                    &["duration_seconds", "guidance_scale", "seed"],
                    "hf_text_to_audio parameters",
                )?;
                copy_fields(
                    parameters,
                    &mut canonical,
                    &["duration_seconds", "guidance_scale", "seed"],
                );
            }
        }
        other => return invalid(format!("ACE-Step does not support endpoint family {other}")),
    }
    request.request = Value::Object(canonical);
    Ok(request)
}

fn reject_unknown_fields(
    object: &serde_json::Map<String, Value>,
    allowed: &[&str],
    context: &str,
) -> Result<()> {
    if let Some(field) = object.keys().find(|field| {
        !allowed
            .iter()
            .any(|allowed_field| *allowed_field == field.as_str())
    }) {
        return invalid(format!("{context} does not support field {field}"));
    }
    Ok(())
}

fn copy_fields(
    source: &serde_json::Map<String, Value>,
    destination: &mut serde_json::Map<String, Value>,
    fields: &[&str],
) {
    for field in fields {
        if let Some(value) = source.get(*field) {
            destination.insert((*field).to_owned(), value.clone());
        }
    }
}

fn validate_ace_media_request(request: &MediaGenerationRequest) -> Result<()> {
    if request.endpoint_family.trim().is_empty() {
        return invalid("media generation endpoint_family must not be empty");
    }
    if !request.request.is_object() {
        return invalid("media generation request must be an object");
    }
    for (field, value) in [
        ("duration_seconds", request.duration_seconds),
        ("frame_count", request.frame_count),
        ("step_count", request.step_count),
    ] {
        if value == Some(0) {
            return invalid(format!("{field} must be positive when supplied"));
        }
    }
    Ok(())
}

fn one_alias<T, const N: usize>(
    field: &str,
    candidates: [(&'static str, Option<T>); N],
) -> Result<Option<T>> {
    let mut selected = None;
    let mut selected_name = None;
    for (name, value) in candidates {
        if let Some(value) = value {
            if let Some(previous) = selected_name {
                return invalid(format!(
                    "{field} aliases {previous} and {name} cannot both be supplied"
                ));
            }
            selected_name = Some(name);
            selected = Some(value);
        }
    }
    Ok(selected)
}

fn normalize_optional_text(field: &str, value: Option<&str>, max: usize) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    validate_text(field, value, 1, max)?;
    Ok(Some(value.to_owned()))
}

fn normalize_tags(tags: Option<RawTags>) -> Result<String> {
    let tags = match tags {
        None => return Ok(String::new()),
        Some(RawTags::String(tags)) => {
            let tags = tags.trim();
            if tags.is_empty() {
                return Ok(String::new());
            }
            validate_text("tags", tags, 1, 506)?;
            tags.to_owned()
        }
        Some(RawTags::List(tags)) => {
            if tags.is_empty() || tags.len() > 32 {
                return invalid("tags must contain between 1 and 32 strings");
            }
            let mut normalized = Vec::with_capacity(tags.len());
            for tag in tags {
                let tag = tag.trim();
                validate_text("each tag", tag, 1, 64)?;
                normalized.push(tag.to_owned());
            }
            normalized.join(", ")
        }
    };
    validate_text("combined tags", &tags, 1, 506)?;
    Ok(tags)
}

fn compose_caption(
    base: &str,
    style: Option<&str>,
    genre: Option<&str>,
    tags: &str,
) -> Result<String> {
    let mut caption = base.to_owned();
    for (label, value) in [("Style", style), ("Genre", genre)] {
        if let Some(value) = value {
            if !caption.is_empty() {
                caption.push('\n');
            }
            caption.push_str(label);
            caption.push_str(": ");
            caption.push_str(value);
        }
    }
    if !tags.is_empty() {
        if !caption.is_empty() {
            caption.push('\n');
        }
        caption.push_str("Tags: ");
        caption.push_str(tags);
    }
    validate_text("composed caption", &caption, 0, 512)?;
    Ok(caption)
}

fn validate_audio_codes_value(value: &RawAudioCodes) -> Result<()> {
    match value {
        RawAudioCodes::String(value) => validate_audio_code_string(value).map(|_| ()),
        RawAudioCodes::List(values) => {
            if values.is_empty() || values.len() > MAX_BATCH_SIZE as usize {
                return invalid(format!(
                    "audio_codes arrays must contain between 1 and {MAX_BATCH_SIZE} strings"
                ));
            }
            let mut token_count = None;
            for value in values {
                if value.is_empty() {
                    return invalid("audio_codes arrays cannot contain empty code strings");
                }
                let count = validate_audio_code_string(value)?;
                if token_count
                    .replace(count)
                    .is_some_and(|previous| previous != count)
                {
                    return invalid(
                        "audio_codes arrays must use equal-duration code strings for every batch item",
                    );
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
fn validate_audio_codes(value: &str) -> Result<()> {
    validate_audio_code_string(value).map(|_| ())
}

fn validate_audio_code_string(value: &str) -> Result<usize> {
    if value.is_empty() {
        return Ok(0);
    }
    if value.len() > MAX_AUDIO_CODE_BYTES {
        return invalid(format!(
            "audio_codes must not exceed {MAX_AUDIO_CODE_BYTES} bytes"
        ));
    }
    let mut remaining = value;
    let mut count = 0_usize;
    while !remaining.is_empty() {
        let code = remaining
            .strip_prefix("<|audio_code_")
            .and_then(|rest| rest.find("|>").map(|end| (&rest[..end], &rest[end + 2..])))
            .ok_or_else(|| {
                EngineError::InvalidConfig(
                    "ACE-Step audio_codes must contain only <|audio_code_N|> tokens".to_owned(),
                )
            })?;
        if code.0.is_empty()
            || !code.0.bytes().all(|byte| byte.is_ascii_digit())
            || code.0.parse::<u32>().map_or(true, |code| code > 63_999)
        {
            return invalid("audio_codes values must be decimal integers from 0 to 63999");
        }
        count = count.saturating_add(1);
        if count > MAX_AUDIO_CODES {
            return invalid(format!(
                "audio_codes must contain at most {MAX_AUDIO_CODES} tokens"
            ));
        }
        remaining = code.1;
    }
    Ok(count)
}

fn validate_batch_seeds(seeds: Vec<i64>, batch_size: u32) -> Result<Vec<i64>> {
    if seeds.len() != batch_size as usize {
        return invalid("seeds must contain exactly one value per batch item");
    }
    if seeds
        .iter()
        .any(|seed| !(-1..=i64::from(u32::MAX)).contains(seed))
    {
        return invalid("each seeds value must be -1 or an unsigned 32-bit integer");
    }
    Ok(seeds)
}

fn normalize_duration(
    duration: Option<Value>,
    duration_seconds: Option<Value>,
    declared_duration: Option<u64>,
) -> Result<f64> {
    if duration.is_some() && duration_seconds.is_some() {
        return invalid("duration and duration_seconds cannot both be supplied");
    }
    let value = match duration.or(duration_seconds) {
        Some(Value::Null) => -1.0,
        Some(Value::String(value)) if value == "auto" => -1.0,
        Some(Value::Number(value)) => value.as_f64().ok_or_else(|| {
            EngineError::InvalidConfig("duration must be a finite number or auto".to_owned())
        })?,
        Some(_) => return invalid("duration must be a finite number or auto"),
        None => -1.0,
    };
    if value != -1.0 && (!value.is_finite() || !(10.0..=600.0).contains(&value)) {
        return invalid("duration must be auto or between 10 and 600 seconds");
    }
    if let Some(declared) = declared_duration {
        if value == -1.0 {
            return invalid("duration_seconds transport metadata conflicts with duration auto");
        }
        if value.ceil() as u64 != declared {
            return invalid("duration does not match media generation duration_seconds");
        }
    }
    Ok(value)
}

fn normalize_audio(audio: Option<RawAudio>, field: &str) -> Result<Value> {
    let Some(audio) = audio else {
        return Ok(Value::Null);
    };
    let (encoded, content_type) = match audio {
        RawAudio::Base64(encoded) => (encoded, "audio/wav".to_owned()),
        RawAudio::Descriptor(descriptor) => {
            if descriptor.encoding != "base64" {
                return invalid(format!("{field}.encoding must be base64"));
            }
            (
                descriptor.data,
                descriptor.content_type.to_ascii_lowercase(),
            )
        }
    };
    if !matches!(
        content_type.as_str(),
        "audio/aac"
            | "audio/flac"
            | "audio/m4a"
            | "audio/mp4"
            | "audio/mpeg"
            | "audio/mp3"
            | "audio/ogg"
            | "audio/opus"
            | "audio/wav"
            | "audio/x-wav"
    ) {
        return invalid(format!("{field}.content_type is unsupported"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| {
            EngineError::InvalidConfig(format!("{field} is not valid base64: {error}"))
        })?;
    if !valid_audio_byte_length(bytes.len(), MAX_INPUT_AUDIO_BYTES) {
        return invalid(format!(
            "{field} must contain between 1 and {MAX_INPUT_AUDIO_BYTES} decoded bytes"
        ));
    }
    if !audio_signature_matches(&bytes, &content_type) {
        return invalid(format!(
            "{field} bytes do not match declared content_type {content_type}"
        ));
    }
    Ok(json!({
        "data_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
        "content_type": content_type,
    }))
}

fn valid_audio_byte_length(length: usize, maximum: usize) -> bool {
    (1..=maximum).contains(&length)
}

fn audio_signature_matches(bytes: &[u8], content_type: &str) -> bool {
    match content_type {
        "audio/wav" | "audio/x-wav" => {
            bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE")
        }
        "audio/flac" => bytes.starts_with(b"fLaC"),
        "audio/ogg" | "audio/opus" => bytes.starts_with(b"OggS"),
        "audio/mpeg" | "audio/mp3" => {
            bytes.starts_with(b"ID3")
                || bytes
                    .get(..2)
                    .is_some_and(|header| header[0] == 0xff && header[1] & 0xe0 == 0xe0)
        }
        "audio/aac" => bytes
            .get(..2)
            .is_some_and(|header| header[0] == 0xff && header[1] & 0xf6 == 0xf0),
        "audio/m4a" | "audio/mp4" => bytes.get(4..8) == Some(b"ftyp"),
        _ => false,
    }
}

fn normalize_output_format(raw: Option<&str>, declared: Option<&str>) -> Result<String> {
    if let (Some(raw), Some(declared)) = (raw, declared) {
        if raw != declared {
            return invalid(
                "response_format does not match media generation response_format metadata",
            );
        }
    }
    let format = raw.or(declared).unwrap_or("flac").to_ascii_lowercase();
    if !matches!(
        format.as_str(),
        "flac" | "mp3" | "opus" | "aac" | "wav" | "wav32"
    ) {
        return invalid("response_format must be flac, mp3, opus, aac, wav, or wav32");
    }
    Ok(format)
}

fn content_type_for_format(format: &str) -> &'static str {
    match format {
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "opus" => "audio/ogg",
        "aac" => "audio/aac",
        "wav" | "wav32" => "audio/wav",
        _ => unreachable!("validated ACE-Step output format"),
    }
}

fn validate_text(field: &str, value: &str, min: usize, max: usize) -> Result<()> {
    let length = value.chars().count();
    if length < min || length > max || value.contains('\0') {
        return invalid(format!(
            "{field} must contain between {min} and {max} characters and no NUL bytes"
        ));
    }
    Ok(())
}

fn validate_keyscale(value: &str) -> Result<()> {
    if value.is_empty() {
        return Ok(());
    }
    let Some((note, mode)) = value.split_once(' ') else {
        return invalid("keyscale must use a note followed by major or minor");
    };
    let mut chars = note.chars();
    if !matches!(chars.next(), Some('A'..='G'))
        || !matches!(chars.as_str(), "" | "#" | "b" | "♯" | "♭")
        || !matches!(mode, "major" | "minor")
    {
        return invalid("keyscale is not a supported ACE-Step key");
    }
    Ok(())
}

fn normalize_timesignature(value: String) -> Result<String> {
    let normalized = match value.trim() {
        "" => "",
        "2" | "2/4" => "2",
        "3" | "3/4" => "3",
        "4" | "4/4" => "4",
        "6" | "6/8" => "6",
        _ => return invalid("timesignature must be 2, 3, 4, 6, or empty"),
    };
    Ok(normalized.to_owned())
}

fn validate_timesteps(timesteps: Option<&[f64]>) -> Result<()> {
    let Some(timesteps) = timesteps else {
        return Ok(());
    };
    if !(2..=200).contains(&timesteps.len()) {
        return invalid("timesteps must contain between 2 and 200 values");
    }
    if timesteps
        .iter()
        .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        || timesteps.windows(2).any(|pair| pair[0] <= pair[1])
    {
        return invalid("timesteps must be finite values from 1 to 0 in strict descending order");
    }
    Ok(())
}

fn effective_ace_step_count(
    inference_steps: u32,
    shift: f64,
    timesteps: Option<&[f64]>,
    cover_noise_strength: f64,
) -> Result<u32> {
    if cover_noise_strength <= 0.0 {
        return Ok(inference_steps);
    }
    let schedule = if let Some(timesteps) = timesteps {
        timesteps.to_vec()
    } else {
        let steps = usize::try_from(inference_steps).map_err(|_| {
            EngineError::InvalidConfig("ACE-Step step count exceeds usize".to_owned())
        })?;
        (0..=steps)
            .map(|index| {
                let timestep = 1.0 - index as f64 / steps as f64;
                shift * timestep / (1.0 + (shift - 1.0) * timestep)
            })
            .collect::<Vec<_>>()
    };
    let effective_noise = 1.0 - cover_noise_strength;
    let start = schedule[..schedule.len() - 1]
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| {
            (*left - effective_noise)
                .abs()
                .total_cmp(&(*right - effective_noise).abs())
        })
        .map(|(index, _)| index)
        .unwrap_or(0);
    u32::try_from(schedule.len() - 1 - start).map_err(|_| {
        EngineError::InvalidConfig("ACE-Step effective step count exceeds u32".to_owned())
    })
}

fn bounded_f64(field: &str, value: f64, min: f64, max: f64) -> Result<f64> {
    if !value.is_finite() || value < min || value > max {
        return invalid(format!("{field} must be between {min} and {max}"));
    }
    Ok(value)
}

fn normalize_u32(field: &str, value: Option<u64>, default: u32, min: u32, max: u32) -> Result<u32> {
    let value = value.unwrap_or(u64::from(default));
    let value = u32::try_from(value)
        .map_err(|_| EngineError::InvalidConfig(format!("{field} exceeds u32")))?;
    if !(min..=max).contains(&value) {
        return invalid(format!("{field} must be between {min} and {max}"));
    }
    Ok(value)
}

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(EngineError::InvalidConfig(format!(
        "ACE-Step {}",
        message.into()
    )))
}

const VALID_LANGUAGES: &[&str] = &[
    "ar", "az", "bg", "bn", "ca", "cs", "da", "de", "el", "en", "es", "fa", "fi", "fr", "he", "hi",
    "hr", "ht", "hu", "id", "is", "it", "ja", "ko", "la", "lt", "ms", "ne", "nl", "no", "pa", "pl",
    "pt", "ro", "ru", "sa", "sk", "sr", "sv", "sw", "ta", "te", "th", "tl", "tr", "uk", "ur", "vi",
    "yue", "zh", "unknown",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModelArtifact, NoopArtifactSink};
    use std::sync::OnceLock;

    static TEST_CACHE: OnceLock<PathBuf> = OnceLock::new();

    struct TestTree {
        root: PathBuf,
    }

    impl TestTree {
        fn new(label: &str) -> Self {
            let root = unique_temp_path(label);
            fs::create_dir_all(&root).expect("create test root");
            Self { root }
        }

        fn model_fixture(&self) -> PathBuf {
            let model_root = self.root.join("models");
            for component in [
                "acestep-v15-sft",
                "Qwen3-Embedding-0.6B",
                "acestep-5Hz-lm-1.7B",
                "vae",
            ] {
                fs::create_dir_all(model_root.join(component)).expect("create component");
            }
            let primary = model_root.join("acestep-v15-sft/model.safetensors");
            write_minimal_safetensors(&primary);
            primary
        }

        #[cfg(unix)]
        fn mock_worker(&self) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;

            let runtime_bin = self.root.join("runtime/bin");
            fs::create_dir_all(&runtime_bin).expect("create mock runtime");
            fs::write(
                runtime_bin.join("ace_runtime_probe.py"),
                "IDENTITY = 'managed-read-only-runtime'\n",
            )
            .expect("write runtime import probe");
            let worker = runtime_bin.join("mock-ace-step-worker.py");
            let interpreter =
                resolve_python_program(Path::new("python3")).expect("find mock Python");
            let mock_worker = MOCK_WORKER.replacen(
                "#!/usr/bin/env python3",
                &format!("#!{}", interpreter.display()),
                1,
            );
            fs::write(&worker, mock_worker).expect("write mock worker");
            fs::set_permissions(&worker, fs::Permissions::from_mode(0o755))
                .expect("make mock worker executable");
            worker
        }
    }

    impl Drop for TestTree {
        fn drop(&mut self) {
            make_test_tree_writable(&self.root);
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn managed_python_runtime_roots_cover_venv_and_base_without_overlap() {
        let tree = TestTree::new("python-runtime-roots");
        let environment_root = tree.root.join("venv");
        let base_root = tree.root.join("base-python");
        let scripts = environment_root.join(if cfg!(windows) { "Scripts" } else { "bin" });
        let python = scripts.join(if cfg!(windows) {
            "python.exe"
        } else {
            "python"
        });
        fs::create_dir_all(&scripts).expect("create scripts");
        fs::create_dir_all(environment_root.join(if cfg!(windows) {
            "Lib/site-packages"
        } else {
            "lib/python3.12/site-packages"
        }))
        .expect("create environment packages");
        fs::create_dir_all(base_root.join(if cfg!(windows) {
            "Lib/site-packages"
        } else {
            "lib/python3.12/site-packages"
        }))
        .expect("create base packages");
        fs::write(&python, b"fixture").expect("write Python fixture");
        fs::write(
            environment_root.join("pyvenv.cfg"),
            format!("home = {}\n", base_root.display()),
        )
        .expect("write pyvenv config");

        let (resolved, roots) =
            ace_step_python_runtime_roots(&python).expect("resolve Python roots");
        assert_eq!(resolved, python);
        assert!(roots.contains(&fs::canonicalize(&environment_root).unwrap()));
        assert!(roots.contains(&fs::canonicalize(&base_root).unwrap()));
        for (index, root) in roots.iter().enumerate() {
            assert!(
                roots
                    .iter()
                    .enumerate()
                    .all(|(other_index, other)| index == other_index
                        || (!root.starts_with(other) && !other.starts_with(root))),
                "runtime roots overlap: {roots:?}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires MAYHEM_ACE_STEP_PYTHON and MAYHEM_ACE_STEP_SANDBOX_CACHE"]
    fn managed_python_starts_inside_windows_appcontainer() {
        let python = PathBuf::from(
            env::var_os("MAYHEM_ACE_STEP_PYTHON")
                .expect("MAYHEM_ACE_STEP_PYTHON must name the managed Python"),
        );
        let cache = PathBuf::from(
            env::var_os("MAYHEM_ACE_STEP_SANDBOX_CACHE")
                .expect("MAYHEM_ACE_STEP_SANDBOX_CACHE must name a writable directory"),
        );
        fs::create_dir_all(&cache).expect("create sandbox cache");
        let (python, mut runtime_roots) =
            ace_step_python_runtime_roots(&python).expect("resolve managed Python");
        let site_packages = env::var_os("MAYHEM_ACE_STEP_SITE_PACKAGES").map(PathBuf::from);
        if let Some(site_packages) = &site_packages {
            runtime_roots.push(fs::canonicalize(site_packages).expect("resolve site-packages"));
        }
        let sandbox = SandboxConfig::new(runtime_roots.clone(), vec![cache.clone()]);
        let mut command = SandboxedCommand::new(&python);
        for root in runtime_roots {
            command.executable_read_only_dir(root);
        }
        configure_worker_environment(&mut command, &python, &cache)
            .expect("configure managed Python environment");
        let probe = env::var("MAYHEM_ACE_STEP_SANDBOX_PROBE")
            .unwrap_or_else(|_| "import sys; print(sys.version)".to_owned());
        command
            .allow_code_generation()
            .current_dir(&cache)
            .stderr(SandboxedStderr::Piped)
            .arg("-I")
            .arg("-c")
            .arg(probe);
        let mut child = command.spawn(&sandbox).expect("spawn managed Python");
        let mut stdout = String::new();
        child
            .take_stdout()
            .expect("capture stdout")
            .read_to_string(&mut stdout)
            .expect("read stdout");
        let mut stderr = String::new();
        child
            .take_stderr()
            .expect("capture stderr")
            .read_to_string(&mut stderr)
            .expect("read stderr");
        let status = child.wait().expect("wait for managed Python");
        assert!(
            status.success(),
            "managed Python failed with {status}; stderr={stderr:?}; stdout={stdout:?}"
        );
        assert!(stdout.contains("3.12"), "unexpected stdout: {stdout:?}");
    }

    fn make_test_tree_writable(path: &Path) {
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return;
        };
        let mut permissions = metadata.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(if metadata.is_dir() { 0o755 } else { 0o644 });
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        let _ = fs::set_permissions(path, permissions);
        if metadata.is_dir() {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    make_test_tree_writable(&entry.path());
                }
            }
        }
    }

    fn unique_temp_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        env::temp_dir().join(format!(
            "mayhem-engine-ace-step-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn source_staging_directory_retries_an_atomic_name_collision() {
        let tree = TestTree::new("source-staging-collision");
        let namespace = tree.root.join("source");
        fs::create_dir_all(&namespace).expect("create source namespace");
        let sequence = AtomicU64::new(7);
        let collision = namespace.join(format!(".{ACE_STEP_SOURCE_SHA256}.42.99.7.tmp"));
        fs::create_dir(&collision).expect("create colliding staging directory");

        let staging = create_source_staging_directory_with(&namespace, 42, 99, &sequence)
            .expect("allocate staging directory after collision");

        assert_eq!(
            staging.file_name().and_then(|name| name.to_str()),
            Some(format!(".{ACE_STEP_SOURCE_SHA256}.42.99.8.tmp").as_str())
        );
        assert!(staging.is_dir());
        assert!(collision.is_dir());
    }

    fn shared_cache() -> &'static Path {
        TEST_CACHE
            .get_or_init(|| {
                let path = unique_temp_path("shared-cache");
                fs::create_dir_all(&path).expect("create shared cache");
                path
            })
            .as_path()
    }

    fn write_minimal_safetensors(path: &Path) {
        let header = br#"{"weight":{"dtype":"F32","shape":[1],"data_offsets":[0,4]}}"#;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(header.len() as u64).to_le_bytes());
        bytes.extend_from_slice(header);
        bytes.extend_from_slice(&0_f32.to_le_bytes());
        fs::write(path, bytes).expect("write minimal safetensors");
    }

    fn load_config(primary: &Path) -> LoadConfig {
        let mut config = LoadConfig::ace_step_safetensors(primary);
        config.backend_cache_dir = Some(shared_cache().to_path_buf());
        config
    }

    fn media_request(body: Value) -> MediaGenerationRequest {
        let prompt = body
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or("A precise test song")
            .to_owned();
        MediaGenerationRequest {
            endpoint_family: "mayhem_music_generations".to_owned(),
            prompt,
            request: body,
            duration_seconds: None,
            frame_count: None,
            step_count: None,
            response_format: None,
        }
    }

    fn encoded_audio(content_type: &str) -> String {
        let bytes = match content_type {
            "audio/wav" => {
                let samples = [1_000_i16, -1_000, 2_000, -2_000];
                let data_len = (samples.len() * std::mem::size_of::<i16>()) as u32;
                let mut wav = Vec::new();
                wav.extend_from_slice(b"RIFF");
                wav.extend_from_slice(&(36 + data_len).to_le_bytes());
                wav.extend_from_slice(b"WAVEfmt ");
                wav.extend_from_slice(&16_u32.to_le_bytes());
                wav.extend_from_slice(&1_u16.to_le_bytes());
                wav.extend_from_slice(&1_u16.to_le_bytes());
                wav.extend_from_slice(&8_000_u32.to_le_bytes());
                wav.extend_from_slice(&16_000_u32.to_le_bytes());
                wav.extend_from_slice(&2_u16.to_le_bytes());
                wav.extend_from_slice(&16_u16.to_le_bytes());
                wav.extend_from_slice(b"data");
                wav.extend_from_slice(&data_len.to_le_bytes());
                for sample in samples {
                    wav.extend_from_slice(&sample.to_le_bytes());
                }
                wav
            }
            "audio/flac" => b"fLaC\0\0\0\x22".to_vec(),
            "audio/mpeg" => b"ID3\x04\0\0\0\0\0\0".to_vec(),
            other => panic!("unsupported test content type {other}"),
        };
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn inline_audio(content_type: &str) -> Value {
        json!({
            "encoding": "base64",
            "data": encoded_audio(content_type),
            "content_type": content_type,
        })
    }

    fn json_object<const N: usize>(entries: [(&str, Value); N]) -> Value {
        Value::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        )
    }

    fn read_log(model_root: &Path) -> Vec<Value> {
        let worker_cache =
            ace_step_worker_cache(shared_cache(), model_root).expect("derive worker cache");
        fs::read_to_string(worker_cache.join("mock-worker.jsonl"))
            .expect("read mock worker log")
            .lines()
            .map(|line| serde_json::from_str(line).expect("parse mock log line"))
            .collect()
    }

    #[test]
    fn embedded_source_identity_and_cache_layout_are_stable() {
        assert_eq!(
            format!("{:x}", Sha256::digest(SOURCE_ARCHIVE)),
            ACE_STEP_SOURCE_SHA256
        );
        let source = ensure_ace_step_source(shared_cache()).expect("extract source");
        assert_eq!(
            source.file_name().and_then(|name| name.to_str()),
            Some(SOURCE_TOP_LEVEL)
        );
        assert!(source.join("pyproject.toml").is_file());
        assert!(source.join("uv.lock").is_file());
        assert_eq!(
            ensure_ace_step_source(shared_cache()).expect("reuse source"),
            source
        );
    }

    #[test]
    fn cached_source_rejects_runtime_mutation_and_unexpected_files() {
        let tree = TestTree::new("mutated-source-cache");
        let source = ensure_ace_step_source(&tree.root).expect("extract source");
        let pyproject = source.join("pyproject.toml");
        let mut permissions = fs::metadata(&pyproject)
            .expect("read pyproject metadata")
            .permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o644);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        fs::set_permissions(&pyproject, permissions).expect("make pyproject writable for test");
        fs::write(&pyproject, b"mutated provider runtime").expect("mutate cached runtime");
        assert!(ensure_ace_step_source(&tree.root).is_err());

        make_test_tree_writable(&tree.root);
        fs::remove_dir_all(&tree.root).expect("remove mutated cache");
        fs::create_dir_all(&tree.root).expect("recreate cache root");
        let source = ensure_ace_step_source(&tree.root).expect("extract source again");
        let mut source_permissions = fs::metadata(&source)
            .expect("read source metadata")
            .permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            source_permissions.set_mode(0o755);
        }
        #[cfg(not(unix))]
        source_permissions.set_readonly(false);
        fs::set_permissions(&source, source_permissions)
            .expect("make source directory writable for test");
        fs::write(source.join("provider-runtime.py"), b"untrusted").expect("add unexpected source");
        assert!(ensure_ace_step_source(&tree.root).is_err());
    }

    #[test]
    fn archive_path_validation_rejects_escape_and_ambiguous_paths() {
        for path in [
            Path::new("/absolute"),
            Path::new("../parent"),
            Path::new("ACE-Step-1.5-v0.1.8/../escape"),
            Path::new("ACE-Step-1.5-v0.1.8/./ambiguous"),
            Path::new("another-root/file"),
        ] {
            assert!(validate_archive_path(path).is_err(), "{path:?}");
        }
    }

    #[test]
    fn archive_extraction_rejects_links_devices_duplicates_and_case_collisions() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Cursor;
        use tar::{Builder, EntryType, Header};

        fn archive(entries: &[(&str, EntryType)]) -> Vec<u8> {
            let encoder = GzEncoder::new(Vec::new(), Compression::default());
            let mut builder = Builder::new(encoder);
            for (path, entry_type) in entries {
                let mut header = Header::new_gnu();
                header.set_mode(0o644);
                header.set_entry_type(*entry_type);
                if entry_type.is_file() {
                    header.set_size(1);
                    header.set_cksum();
                    builder
                        .append_data(&mut header, path, Cursor::new([1_u8]))
                        .expect("append test file");
                } else {
                    header.set_size(0);
                    if entry_type.is_symlink() {
                        header
                            .set_link_name("/server/private/runtime.py")
                            .expect("set test link");
                    }
                    header.set_cksum();
                    builder
                        .append_data(&mut header, path, Cursor::new([]))
                        .expect("append special entry");
                }
            }
            builder
                .into_inner()
                .expect("finish tar")
                .finish()
                .expect("finish gzip")
        }

        let cases = [
            archive(&[("ACE-Step-1.5-v0.1.8/link", EntryType::Symlink)]),
            archive(&[("ACE-Step-1.5-v0.1.8/device", EntryType::Block)]),
            archive(&[
                ("ACE-Step-1.5-v0.1.8/file", EntryType::Regular),
                ("ACE-Step-1.5-v0.1.8/file", EntryType::Regular),
            ]),
            archive(&[
                ("ACE-Step-1.5-v0.1.8/File", EntryType::Regular),
                ("ACE-Step-1.5-v0.1.8/file", EntryType::Regular),
            ]),
        ];
        for (index, bytes) in cases.iter().enumerate() {
            let tree = TestTree::new(&format!("malicious-archive-{index}"));
            assert!(extract_source_archive(bytes, &tree.root).is_err());
        }
    }

    #[test]
    fn constructors_expose_dedicated_ace_step_format() {
        let artifact = ModelArtifact::ace_step_safetensors("/tmp/model.safetensors");
        assert_eq!(artifact.format, ArtifactFormat::AceStepSafetensors);
        assert_eq!(
            LoadConfig::ace_step_safetensors("/tmp/model.safetensors")
                .artifact
                .format,
            ArtifactFormat::AceStepSafetensors
        );
        let tree = TestTree::new("directory-primary");
        let directory_artifact = ModelArtifact::ace_step_safetensors(&tree.root);
        assert!(verify_artifact(&directory_artifact).is_err());
    }

    #[test]
    fn signed_empty_optional_music_defaults_normalize_for_the_engine() {
        let normalized = NormalizedRequest::from_media_request(media_request(json!({
            "model": "acestep/ace-step-1.5",
            "prompt": "A precise test song",
            "style": "",
            "genre": "",
            "tags": "",
            "sample_query": "",
            "duration_seconds": 10,
            "steps": 50,
            "seed": 7
        })))
        .expect("normalize signed empty defaults");
        let payload = normalized.worker_payload();
        assert_eq!(payload["params"]["caption"], "A precise test song");
        assert_eq!(payload["params"]["timesignature"], "");
    }

    #[test]
    fn current_endpoint_aliases_normalize_without_becoming_worker_paths() {
        let mut request = media_request(json!({
            "prompt": "Alias contract",
            "language": "en",
            "key": "C major",
            "time_signature": "4/4",
            "duration_seconds": 12,
            "steps": 64,
            "temperature": 0.6,
            "lm_cfg": 2.5,
            "top_k": 20,
            "top_p": 0.75,
            "cot_metas": false,
            "cot_caption": false,
            "cot_language": false,
            "constrained_decoding": false,
            "melody": inline_audio("audio/flac"),
            "adg": true,
            "n": 2,
            "audio_format": "opus"
        }));
        request.duration_seconds = Some(12);
        request.step_count = Some(64);
        let normalized =
            NormalizedRequest::from_media_request(request).expect("normalize current aliases");
        let payload = normalized.worker_payload();
        assert_eq!(payload["params"]["vocal_language"], "en");
        assert_eq!(payload["params"]["keyscale"], "C major");
        assert_eq!(payload["params"]["timesignature"], "4");
        assert_eq!(payload["params"]["inference_steps"], 64);
        assert_eq!(payload["params"]["lm_temperature"], 0.6);
        assert_eq!(payload["params"]["use_adg"], true);
        assert_eq!(payload["params"]["timesteps"], Value::Null);
        assert_eq!(payload["config"]["batch_size"], 2);
        assert_eq!(payload["config"]["audio_format"], "opus");
        assert!(payload["source_audio"].is_null());
        assert_eq!(payload["reference_audio"]["content_type"], "audio/flac");
        assert!(payload["params"].get("src_audio").is_none());
        assert!(payload["params"].get("reference_audio").is_none());

        let source_alias = NormalizedRequest::from_media_request(media_request(json!({
            "prompt": "Source alias",
            "task_type": "cover",
            "src_audio": inline_audio("audio/wav")
        })))
        .expect("normalize source alias")
        .worker_payload();
        assert_eq!(source_alias["source_audio"]["content_type"], "audio/wav");
    }

    #[test]
    fn remaining_accepted_aliases_are_consumed_and_forwarded() {
        let source = inline_audio("audio/wav");
        let cases = [
            (
                "caption",
                json!({"caption": "Caption alias"}),
                "/params/caption",
                json!("Caption alias"),
            ),
            (
                "string tags",
                json!({"prompt": "Tags", "tags": "dry, intimate"}),
                "/params/caption",
                json!("Tags\nTags: dry, intimate"),
            ),
            (
                "auto duration",
                json!({"prompt": "Automatic duration", "duration": "auto"}),
                "/params/duration",
                json!(-1.0),
            ),
            (
                "input_audio",
                json!({"task_type": "cover", "input_audio": source.clone()}),
                "/source_audio/content_type",
                json!("audio/wav"),
            ),
            (
                "audio",
                json!({"task_type": "cover", "audio": source.clone()}),
                "/source_audio/content_type",
                json!("audio/wav"),
            ),
            (
                "audio_code_string",
                json!({
                    "task_type": "cover",
                    "audio_code_string": "<|audio_code_4|>"
                }),
                "/params/audio_codes",
                json!("<|audio_code_4|>"),
            ),
            (
                "use_dcw",
                json!({"prompt": "DCW alias", "use_dcw": false}),
                "/params/dcw_enabled",
                json!(false),
            ),
            (
                "flow_edit",
                json!({
                    "prompt": "Flow alias",
                    "task_type": "cover",
                    "source_audio": source.clone(),
                    "flow_edit": true
                }),
                "/params/flow_edit_morph",
                json!(true),
            ),
            (
                "batch",
                json!({"prompt": "Batch alias", "batch": 2}),
                "/config/batch_size",
                json!(2),
            ),
        ];
        for (label, body, pointer, expected) in cases {
            let payload = NormalizedRequest::from_media_request(media_request(body))
                .unwrap_or_else(|error| panic!("{label} failed: {error}"))
                .worker_payload();
            assert_eq!(payload.pointer(pointer), Some(&expected), "{label}");
        }
    }

    #[test]
    fn compatible_audio_families_map_only_lossless_controls() {
        let mayhem_audio = MediaGenerationRequest {
            endpoint_family: ENDPOINT_MAYHEM_AUDIO_GENERATIONS.to_owned(),
            prompt: "arbitrary audio request".to_owned(),
            request: json!({
                "model": "test/music",
                "prompt": "arbitrary audio request",
                "duration_seconds": 12.5,
                "response_format": "opus",
                "guidance_scale": 6.5,
                "seed": 19
            }),
            duration_seconds: Some(13),
            frame_count: None,
            step_count: None,
            response_format: Some("opus".to_owned()),
        };
        let payload = NormalizedRequest::from_media_request(mayhem_audio)
            .expect("Mayhem audio compatibility")
            .worker_payload();
        assert_eq!(payload["params"]["caption"], "arbitrary audio request");
        assert_eq!(payload["params"]["duration"], 12.5);
        assert_eq!(payload["params"]["guidance_scale"], 6.5);
        assert_eq!(payload["params"]["seed"], 19);
        assert_eq!(payload["config"]["audio_format"], "opus");

        let hf_audio = MediaGenerationRequest {
            endpoint_family: ENDPOINT_HF_TEXT_TO_AUDIO.to_owned(),
            prompt: "arbitrary Hugging Face audio request".to_owned(),
            request: json!({
                "inputs": "arbitrary Hugging Face audio request",
                "parameters": {
                    "duration_seconds": 14.0,
                    "guidance_scale": 5.0,
                    "seed": 23
                }
            }),
            duration_seconds: Some(14),
            frame_count: None,
            step_count: None,
            response_format: None,
        };
        let payload = NormalizedRequest::from_media_request(hf_audio)
            .expect("HF audio compatibility")
            .worker_payload();
        assert_eq!(
            payload["params"]["caption"],
            "arbitrary Hugging Face audio request"
        );
        assert_eq!(payload["params"]["duration"], 14.0);
        assert_eq!(payload["params"]["guidance_scale"], 5.0);
        assert_eq!(payload["params"]["seed"], 23);
        assert_eq!(payload["config"]["audio_format"], "flac");
    }

    #[test]
    fn compatible_audio_families_reject_unmapped_controls() {
        let request = MediaGenerationRequest {
            endpoint_family: ENDPOINT_MAYHEM_AUDIO_GENERATIONS.to_owned(),
            prompt: "do not drop controls".to_owned(),
            request: json!({
                "model": "test/music",
                "prompt": "do not drop controls",
                "temperature": 0.5
            }),
            duration_seconds: None,
            frame_count: None,
            step_count: None,
            response_format: None,
        };
        let error = match NormalizedRequest::from_media_request(request) {
            Ok(_) => panic!("unsupported controls must not be dropped"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("temperature"));
    }

    #[test]
    fn defaults_and_forwarded_generation_params_match_the_sft_contract() {
        let normalized =
            NormalizedRequest::from_media_request(media_request(json!({"prompt": "Defaults"})))
                .expect("normalize defaults");
        let payload = normalized.worker_payload();
        assert_eq!(payload["params"]["duration"], -1.0);
        assert_eq!(payload["params"]["inference_steps"], 50);
        assert_eq!(payload["params"]["guidance_scale"], 7.0);
        assert_eq!(payload["params"]["thinking"], false);
        assert_eq!(payload["params"]["use_cot_caption"], true);
        assert_eq!(payload["params"]["shift"], 3.0);
        assert_eq!(payload["params"]["dcw_enabled"], false);
        assert_eq!(payload["config"]["audio_format"], "flac");
        assert_eq!(payload["config"]["batch_size"], 2);
        assert_eq!(payload["preprocess"]["sample_mode"], false);
        assert_eq!(payload["preprocess"]["use_format"], false);

        let actual = payload["params"]
            .as_object()
            .expect("params object")
            .keys()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let expected = [
            "task_type",
            "instruction",
            "audio_codes",
            "caption",
            "lyrics",
            "instrumental",
            "vocal_language",
            "bpm",
            "keyscale",
            "timesignature",
            "duration",
            "inference_steps",
            "seed",
            "guidance_scale",
            "enable_normalization",
            "normalization_db",
            "fade_in_duration",
            "fade_out_duration",
            "latent_shift",
            "latent_rescale",
            "use_adg",
            "cfg_interval_start",
            "cfg_interval_end",
            "shift",
            "infer_method",
            "sampler_mode",
            "velocity_norm_threshold",
            "velocity_ema_factor",
            "dcw_enabled",
            "dcw_mode",
            "dcw_scaler",
            "dcw_high_scaler",
            "dcw_wavelet",
            "timesteps",
            "repainting_start",
            "repainting_end",
            "chunk_mask_mode",
            "repaint_mode",
            "repaint_strength",
            "audio_cover_strength",
            "cover_noise_strength",
            "retake_seed",
            "retake_variance",
            "flow_edit_morph",
            "flow_edit_source_caption",
            "flow_edit_source_lyrics",
            "flow_edit_n_min",
            "flow_edit_n_max",
            "flow_edit_n_avg",
            "thinking",
            "lm_temperature",
            "lm_cfg_scale",
            "lm_top_k",
            "lm_top_p",
            "lm_negative_prompt",
            "use_cot_metas",
            "use_cot_caption",
            "use_cot_language",
            "use_constrained_decoding",
        ]
        .into_iter()
        .collect::<HashSet<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn helper_modes_flow_edit_audio_codes_and_per_batch_seeds_are_forwarded() {
        let sample = NormalizedRequest::from_media_request(media_request(json!({
            "prompt": "Sample seed",
            "sample_mode": true,
            "sample_query": "A restrained chamber pop song",
            "use_format": true,
        })))
        .expect("normalize helper modes")
        .worker_payload();
        assert_eq!(sample["preprocess"]["sample_mode"], true);
        assert_eq!(
            sample["preprocess"]["sample_query"],
            "A restrained chamber pop song"
        );
        assert_eq!(sample["preprocess"]["use_format"], true);
        assert!(sample["params"].get("sample_mode").is_none());
        assert!(WORKER.contains("create_sample("));
        assert!(WORKER.contains("format_sample("));

        let flow = NormalizedRequest::from_media_request(media_request(json!({
            "prompt": "Target arrangement",
            "task_type": "cover-nofsq",
            "source_audio": inline_audio("audio/wav"),
            "flow_edit_morph": true,
            "flow_edit_source_caption": "Original arrangement",
            "flow_edit_source_lyrics": "[Verse]\nOriginal",
            "flow_edit_n_min": 0.2,
            "flow_edit_n_max": 0.8,
            "flow_edit_n_avg": 4,
            "batch_size": 2,
            "seeds": [11, 12],
            "response_format": "mp3",
            "mp3_bitrate": "320k",
            "mp3_sample_rate": 44100,
        })))
        .expect("normalize flow edit")
        .worker_payload();
        assert_eq!(flow["params"]["flow_edit_morph"], true);
        assert_eq!(flow["params"]["flow_edit_n_avg"], 4);
        assert_eq!(flow["config"]["seeds"], json!([11, 12]));
        assert_eq!(flow["config"]["use_random_seed"], false);
        assert_eq!(flow["config"]["mp3_bitrate"], "320k");
        assert_eq!(flow["config"]["mp3_sample_rate"], 44100);

        let codes = "<|audio_code_0|><|audio_code_63999|>";
        let coded = NormalizedRequest::from_media_request(media_request(json!({
            "prompt": "Code-controlled cover",
            "task_type": "cover",
            "audio_codes": codes,
        })))
        .expect("normalize audio codes")
        .worker_payload();
        assert_eq!(coded["params"]["audio_codes"], codes);
        assert!(coded["source_audio"].is_null());

        let per_batch = NormalizedRequest::from_media_request(media_request(json!({
            "prompt": "Per-item code-controlled covers",
            "task_type": "cover",
            "n": 2,
            "audio_codes": [
                "<|audio_code_1|><|audio_code_2|>",
                "<|audio_code_3|><|audio_code_4|>"
            ],
        })))
        .expect("normalize per-batch audio codes")
        .worker_payload();
        assert_eq!(
            per_batch["params"]["audio_codes"]
                .as_array()
                .expect("audio-code array")
                .len(),
            2
        );
    }

    #[test]
    fn audio_code_grammar_and_cap_come_from_pinned_five_hz_duration() {
        assert_eq!(MAX_AUDIO_CODES, 5 * 600);
        let at_cap = "<|audio_code_1|>".repeat(MAX_AUDIO_CODES);
        validate_audio_codes(&at_cap).expect("accept duration-derived token cap");
        assert!(validate_audio_codes(&(at_cap + "<|audio_code_2|>")).is_err());
        for invalid_codes in [
            " <|audio_code_1|>",
            "<|audio_code_64000|>",
            "<|audio_code_-1|>",
            "<|audio_code_1|>suffix",
        ] {
            assert!(
                validate_audio_codes(invalid_codes).is_err(),
                "{invalid_codes}"
            );
        }
    }

    #[test]
    fn input_and_output_audio_caps_match_protocol_and_long_wav32_requirements() {
        assert_eq!(MAX_INPUT_AUDIO_BYTES, 64 * 1024 * 1024);
        assert_eq!(MAX_OUTPUT_AUDIO_BYTES, 512 * 1024 * 1024);
        for maximum in [MAX_INPUT_AUDIO_BYTES, MAX_OUTPUT_AUDIO_BYTES] {
            assert!(!valid_audio_byte_length(0, maximum));
            assert!(valid_audio_byte_length(1, maximum));
            assert!(valid_audio_byte_length(maximum, maximum));
            assert!(!valid_audio_byte_length(maximum + 1, maximum));
        }
        let wav32_600s_stereo_bytes = 600_usize * 48_000 * 2 * 4 + 44;
        assert!(wav32_600s_stereo_bytes > 128 * 1024 * 1024);
        assert!(valid_audio_byte_length(
            wav32_600s_stereo_bytes,
            MAX_OUTPUT_AUDIO_BYTES
        ));
        assert!(WORKER.contains("_MAX_INPUT_AUDIO_BYTES = 64 * 1024 * 1024"));
        assert!(WORKER.contains("_MAX_OUTPUT_AUDIO_BYTES = 512 * 1024 * 1024"));
        assert!(!WORKER.contains("_MAX_AUDIO_BYTES"));
    }

    #[test]
    fn every_advertised_output_codec_normalizes_to_its_exact_content_type() {
        for (format, content_type) in [
            ("flac", "audio/flac"),
            ("mp3", "audio/mpeg"),
            ("opus", "audio/ogg"),
            ("aac", "audio/aac"),
            ("wav", "audio/wav"),
            ("wav32", "audio/wav"),
        ] {
            let payload = NormalizedRequest::from_media_request(media_request(json!({
                "prompt": "Codec boundary",
                "response_format": format,
            })))
            .unwrap_or_else(|error| panic!("{format} normalization failed: {error}"))
            .worker_payload();
            assert_eq!(payload["config"]["audio_format"], format);
            assert_eq!(content_type_for_format(format), content_type);
        }
    }

    #[test]
    fn task_aware_inputs_accept_lyrics_only_and_source_driven_sft_tasks() {
        let mut lyrics_only = media_request(json!({
            "lyrics": "[Verse]\nLyrics carry this request",
            "instrumental": false,
        }));
        lyrics_only.prompt.clear();
        let lyrics_payload = NormalizedRequest::from_media_request(lyrics_only)
            .expect("accept lyrics-only text2music")
            .worker_payload();
        assert_eq!(lyrics_payload["params"]["caption"], "");
        assert_eq!(
            lyrics_payload["params"]["lyrics"],
            "[Verse]\nLyrics carry this request"
        );

        for task in ["cover", "cover-nofsq", "repaint"] {
            let mut request = media_request(json!({
                "task_type": task,
                "source_audio": inline_audio("audio/wav"),
            }));
            request.prompt.clear();
            let payload = NormalizedRequest::from_media_request(request)
                .unwrap_or_else(|error| panic!("{task} source request failed: {error}"))
                .worker_payload();
            assert_eq!(payload["params"]["task_type"], task);
            assert!(!payload["source_audio"].is_null());
        }
    }

    #[test]
    fn task_aware_inputs_reject_missing_required_content() {
        let mut empty_text = media_request(json!({}));
        empty_text.prompt.clear();
        assert!(NormalizedRequest::from_media_request(empty_text).is_err());

        for task in ["cover", "cover-nofsq", "repaint"] {
            let request = media_request(json!({"prompt": "Missing source", "task_type": task}));
            assert!(
                NormalizedRequest::from_media_request(request).is_err(),
                "{task}"
            );
        }
        let flow_without_source = media_request(json!({
            "prompt": "Flow target",
            "flow_edit_morph": true,
        }));
        assert!(NormalizedRequest::from_media_request(flow_without_source).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn primary_catalog_sha_is_verified_before_worker_start() {
        let tree = TestTree::new("primary-sha");
        let primary = tree.model_fixture();
        let mut config = load_config(&primary);
        config.artifact.sha256 = Some("00".repeat(32));
        let mut backend =
            AceStepBackend::with_python(tree.mock_worker()).expect("construct backend");
        let error = backend
            .load(config)
            .expect_err("reject mismatched primary sha");
        assert!(matches!(error, EngineError::ArtifactHashMismatch { .. }));
        assert!(backend.process_ids().is_empty());
    }

    #[test]
    fn embedded_worker_forces_trusted_code_and_local_auto_models() {
        assert!(WORKER.contains("sys.path.insert(0, str(source_root))"));
        assert!(WORKER.contains("kwargs[\"trust_remote_code\"] = False"));
        assert!(WORKER.contains("ensure_main_model = _forbid_download"));
        assert!(WORKER.contains("backend=\"pt\""));
        assert!(WORKER.contains("getattr(accelerator, \"mem_get_info\", None)"));
        assert!(WORKER.contains("_verify_effective_execution_config"));
        assert!(WORKER.contains("_require_local_file_under"));
        assert!(WORKER.contains("FILE_ATTRIBUTE_REPARSE_POINT"));
        assert!(WORKER.contains(
            "def _generate(payload):\n    if _dit_handler is None or _model_root is None or _worker_cache is None:\n        raise RuntimeError(\"ACE-Step model is not loaded\")\n    from acestep.inference import generate_music"
        ));
        assert!(!WORKER.contains(".resolve("));
        assert!(!WORKER.contains("snapshot_download("));
        assert!(!WORKER.contains("CUDA out of memory"));
    }

    #[test]
    fn execution_evidence_is_bound_to_load_time_free_memory_policy() {
        let config = AceStepExecutionConfig {
            device_kind: "cuda".to_owned(),
            free_memory_bytes: Some(7_700_000_000),
            total_memory_bytes: Some(24 * 1024 * 1024 * 1024),
            offload_to_cpu: true,
            offload_dit_to_cpu: true,
            quantization: Some("int8_weight_only".to_owned()),
            selection_basis: "load-time-free-accelerator-memory".to_owned(),
            memory_calibration: MEMORY_CALIBRATION.to_owned(),
            source_commit: ACE_STEP_SOURCE_COMMIT.to_owned(),
        };
        validate_execution_config(&config).expect("accept low-free-memory policy");

        let mut total_vram_policy = config.clone();
        total_vram_policy.offload_to_cpu = false;
        total_vram_policy.offload_dit_to_cpu = false;
        total_vram_policy.quantization = None;
        assert!(validate_execution_config(&total_vram_policy).is_err());

        let mut above_dit_threshold = config;
        above_dit_threshold.free_memory_bytes = Some(13 * 1024 * 1024 * 1024);
        above_dit_threshold.offload_dit_to_cpu = false;
        validate_execution_config(&above_dit_threshold)
            .expect("DiT offload stops at twelve GiB of free memory");
    }

    #[test]
    fn embedded_worker_selects_from_free_memory_not_total_vram() {
        let worker = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ace_step_worker.py");
        let probe = r#"
import json
import runpy
import sys

module = runpy.run_path(sys.argv[1], run_name="ace_step_policy_test")
for maximum in (
    module["_MAX_INPUT_AUDIO_BYTES"],
    module["_MAX_OUTPUT_AUDIO_BYTES"],
):
    module["_validate_audio_byte_length"](maximum, maximum, "boundary")
    try:
        module["_validate_audio_byte_length"](maximum + 1, maximum, "boundary")
    except ValueError:
        pass
    else:
        raise AssertionError("worker accepted audio above its byte ceiling")

class FakeMps:
    @staticmethod
    def is_available():
        return False

class FakeBackends:
    mps = FakeMps()

class FakeXpu:
    @staticmethod
    def is_available():
        return False

class FakeCuda:
    free = 0

    @staticmethod
    def is_available():
        return True

    @classmethod
    def mem_get_info(cls):
        return cls.free, 24 * 1024**3

class FakeTorch:
    cuda = FakeCuda
    xpu = FakeXpu()
    backends = FakeBackends()

result = []
for free in (7_700_000_000, 13 * 1024**3, 21 * 1024**3):
    FakeCuda.free = free
    result.append(module["_select_execution_config"](FakeTorch))
print(json.dumps(result))
"#;
        let python =
            resolve_python_program(Path::new("python3")).expect("find Python for policy test");
        let output = std::process::Command::new(python)
            .args(["-I", "-c", probe])
            .arg(worker)
            .output()
            .expect("run worker policy probe");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let configs: Vec<AceStepExecutionConfig> =
            serde_json::from_slice(&output.stdout).expect("decode worker policy evidence");
        assert!(configs[0].offload_to_cpu);
        assert!(configs[0].offload_dit_to_cpu);
        assert_eq!(configs[0].quantization.as_deref(), Some("int8_weight_only"));
        assert!(configs[1].offload_to_cpu);
        assert!(!configs[1].offload_dit_to_cpu);
        assert!(!configs[2].offload_to_cpu);
        assert!(!configs[2].offload_dit_to_cpu);
        assert_eq!(configs[0].total_memory_bytes, configs[2].total_memory_bytes);
    }

    #[test]
    fn embedded_worker_generate_path_reaches_artifact_response() {
        let worker = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ace_step_worker.py");
        let probe = r#"
import json
import pathlib
import runpy
import sys
import tempfile
import types

module = runpy.run_path(sys.argv[1], run_name="ace_step_generate_test")
state = {}

class FakeTensor:
    shape = (1, 16000)

class FakeResult:
    success = True
    error = None
    status_message = None

    @property
    def audios(self):
        return [{
            "path": str(state["path"]),
            "tensor": FakeTensor(),
            "sample_rate": 16000,
        }]

def fake_generate_music(*args, **kwargs):
    return FakeResult()

inference = types.ModuleType("acestep.inference")
inference.generate_music = fake_generate_music
package = types.ModuleType("acestep")
package.__path__ = []
package.inference = inference
sys.modules["acestep"] = package
sys.modules["acestep.inference"] = inference

def fake_prepare(payload, temporary, apply_preprocess):
    outputs = pathlib.Path(temporary) / "outputs"
    outputs.mkdir()
    state["path"] = outputs / "result.wav"
    state["path"].write_bytes(b"RIFF\x00\x00\x00\x00WAVE")
    return ({}, object(), object(), 1, "audio/wav", outputs, None, None)

with tempfile.TemporaryDirectory() as root:
    root = pathlib.Path(root)
    worker_globals = module["_generate"].__globals__
    worker_globals["_dit_handler"] = object()
    worker_globals["_llm_handler"] = object()
    worker_globals["_model_root"] = root
    worker_globals["_worker_cache"] = root
    worker_globals["_prepare_generation"] = fake_prepare
    worker_globals["_canonicalize_generated_wave"] = lambda *args: None
    worker_globals["_validate_generated_audio"] = lambda *args: None
    result = module["_generate"]({"config": {"audio_format": "wav"}})

assert result["step_count"] == 1
assert len(result["artifacts"]) == 1
assert result["artifacts"][0]["content_type"] == "audio/wav"
print(json.dumps(result))
"#;
        let python =
            resolve_python_program(Path::new("python3")).expect("find Python for generate test");
        let output = std::process::Command::new(python)
            .args(["-I", "-c", probe])
            .arg(worker)
            .output()
            .expect("run embedded worker generate probe");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value =
            serde_json::from_slice(&output.stdout).expect("decode worker generation result");
        assert_eq!(result["step_count"], 1);
        assert_eq!(result["artifacts"][0]["content_type"], "audio/wav");
    }

    #[test]
    fn embedded_worker_canonicalizes_wav_and_wav32_explicitly() {
        let worker = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ace_step_worker.py");
        let probe = r#"
import pathlib
import runpy
import sys
import tempfile
import types

module = runpy.run_path(sys.argv[1], run_name="ace_step_wave_test")
state = {}

class FakeFinite:
    @staticmethod
    def all():
        return True

class FakeArray:
    ndim = 2
    shape = (2, 16)
    T = "samples-first"

class FakeTensor:
    shape = (2, 16)

    def detach(self):
        return self

    def cpu(self):
        return self

    def float(self):
        return self

    @staticmethod
    def numpy():
        return FakeArray()

numpy = types.ModuleType("numpy")
numpy.float32 = "float32"
numpy.isfinite = lambda _samples: FakeFinite()
numpy.ascontiguousarray = lambda samples, dtype: (samples, dtype)
sys.modules["numpy"] = numpy

soundfile = types.ModuleType("soundfile")
def write(path, samples, sample_rate, *, format, subtype):
    state["samples"] = samples
    state["sample_rate"] = sample_rate
    state["format"] = format
    state["subtype"] = subtype
    pathlib.Path(path).write_bytes(subtype.encode("ascii"))
soundfile.write = write
sys.modules["soundfile"] = soundfile

with tempfile.TemporaryDirectory() as root:
    root = pathlib.Path(root)
    for audio_format, expected_subtype in (("wav", "PCM_16"), ("wav32", "FLOAT")):
        output = root / f"result-{audio_format}.wav"
        output.write_bytes(b"uncanonical")
        module["_canonicalize_generated_wave"](
            output, FakeTensor(), 48000, audio_format
        )
        assert output.read_bytes() == expected_subtype.encode("ascii")
        assert state == {
            "samples": ("samples-first", "float32"),
            "sample_rate": 48000,
            "format": "WAV",
            "subtype": expected_subtype,
        }
        state.clear()
        assert not list(root.glob(".*.mayhem-canonical.wav"))

print("ok")
"#;
        let python =
            resolve_python_program(Path::new("python3")).expect("find Python for wave test");
        let output = std::process::Command::new(python)
            .args(["-I", "-c", probe])
            .arg(worker)
            .output()
            .expect("run embedded worker wave probe");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ok");
    }

    #[cfg(unix)]
    #[test]
    fn load_uses_derived_components_pinned_source_and_offline_environment() {
        let tree = TestTree::new("offline");
        let primary = tree.model_fixture();
        let model_root = primary.parent().unwrap().parent().unwrap();
        let mut backend =
            AceStepBackend::with_python(tree.mock_worker()).expect("construct backend");
        let loaded = backend.load(load_config(&primary)).expect("load backend");

        assert_eq!(backend.backend_id(), "ace-step");
        assert_eq!(loaded.backend, "ace-step");
        assert_eq!(backend.process_ids().len(), 1);
        assert!(backend.component_healthy());
        let log = read_log(model_root);
        let load = log.first().expect("load event");
        assert_eq!(load["event"], "load");
        assert_eq!(
            Path::new(load["model_root"].as_str().unwrap()),
            model_root.canonicalize().unwrap()
        );
        assert_eq!(load["env"]["HF_HUB_OFFLINE"], "1");
        assert_eq!(load["env"]["TRANSFORMERS_OFFLINE"], "1");
        assert_eq!(load["env"]["DIFFUSERS_OFFLINE"], "1");
        assert_eq!(load["env"]["PIP_NO_INDEX"], "1");
        assert_eq!(load["env"]["PYTHONPATH"], Value::Null);
        assert!(Path::new(load["env"]["TMPDIR"].as_str().unwrap())
            .starts_with(ace_step_worker_cache(shared_cache(), model_root).unwrap()));
        assert!(
            Path::new(load["env"]["DARWIN_USER_CACHE_DIR"].as_str().unwrap())
                .starts_with(ace_step_worker_cache(shared_cache(), model_root).unwrap())
        );
        assert_eq!(load["runtime_import"], "managed-read-only-runtime");
        assert_eq!(load["runtime_read_only"], true);
        assert_eq!(load["network_denied"], true);
        assert_eq!(
            Path::new(load["worker_cache"].as_str().unwrap()),
            ace_step_worker_cache(shared_cache(), model_root)
                .unwrap()
                .canonicalize()
                .unwrap()
        );
        let source = Path::new(load["source_root"].as_str().unwrap());
        assert!(source.join("pyproject.toml").is_file());
        assert!(source.join("uv.lock").is_file());
        assert!(source.starts_with(shared_cache().canonicalize().unwrap()));
        let evidence = backend
            .loaded_backend_evidence()
            .expect("loaded execution evidence");
        assert_eq!(evidence["device_kind"], "cpu");
        assert_eq!(
            evidence["source_commit"],
            "dce621408bee8c31b4fcf4811682eb9359e1bc94"
        );
    }

    #[cfg(unix)]
    #[test]
    fn controls_and_multiple_artifacts_cross_the_worker_contract() {
        let tree = TestTree::new("controls");
        let primary = tree.model_fixture();
        let model_root = primary.parent().unwrap().parent().unwrap();
        let mut backend =
            AceStepBackend::with_python(tree.mock_worker()).expect("construct backend");
        backend.load(load_config(&primary)).expect("load backend");

        let body = json_object([
            ("model", json!("ACE-Step/acestep-v15-sft")),
            ("prompt", json!("Synth pop with a bright chorus")),
            ("style", json!("polished and bright")),
            ("genre", json!("synth pop")),
            ("tags", json!(["anthemic", "wide stereo"])),
            ("lyrics", json!("[Verse]\nTest the contract")),
            ("instrumental", json!(false)),
            ("vocal_language", json!("en")),
            ("bpm", json!(128)),
            ("keyscale", json!("F# minor")),
            ("timesignature", json!("4/4")),
            ("duration", json!(20.0)),
            ("inference_steps", json!(120)),
            ("guidance_scale", json!(9.5)),
            ("seed", json!(42)),
            ("thinking", json!(true)),
            ("lm_temperature", json!(0.7)),
            ("lm_cfg_scale", json!(3.0)),
            ("lm_top_k", json!(40)),
            ("lm_top_p", json!(0.8)),
            ("lm_negative_prompt", json!("muddy mix")),
            ("use_cot_metas", json!(false)),
            ("use_cot_caption", json!(true)),
            ("use_cot_language", json!(false)),
            ("use_constrained_decoding", json!(true)),
            ("task_type", json!("text2music")),
            ("instruction", json!("Generate the requested music:")),
            ("reference_audio", inline_audio("audio/mpeg")),
            ("infer_method", json!("ode")),
            ("sampler_mode", json!("heun")),
            ("velocity_norm_threshold", json!(2.0)),
            ("velocity_ema_factor", json!(0.1)),
            ("dcw_enabled", json!(true)),
            ("dcw_mode", json!("double")),
            ("dcw_scaler", json!(0.04)),
            ("dcw_high_scaler", json!(0.01)),
            ("dcw_wavelet", json!("db4")),
            ("shift", json!(1.5)),
            ("cfg_interval_start", json!(0.1)),
            ("cfg_interval_end", json!(0.9)),
            ("use_adg", json!(true)),
            ("enable_normalization", json!(true)),
            ("normalization_db", json!(-2.0)),
            ("fade_in_duration", json!(0.5)),
            ("fade_out_duration", json!(0.75)),
            ("latent_shift", json!(0.05)),
            ("latent_rescale", json!(1.1)),
            ("retake_seed", json!(77)),
            ("retake_variance", json!(0.2)),
            ("batch_size", json!(2)),
            ("response_format", json!("wav32")),
        ]);
        let mut request = media_request(body);
        request.duration_seconds = Some(20);
        request.response_format = Some("wav32".to_owned());
        let mut artifacts = Vec::new();
        let output = backend
            .generate_music(
                request,
                &mut |chunk: ArtifactChunk| {
                    artifacts.push(chunk);
                    Ok(())
                },
                &CancellationToken::new(),
            )
            .expect("generate music");

        assert_eq!(output.duration_seconds, 20);
        assert_eq!(output.step_count, 120);
        assert_eq!(artifacts.len(), 2);
        assert_ne!(artifacts[0].artifact_id, artifacts[1].artifact_id);
        assert!(artifacts.iter().all(|artifact| {
            artifact.content_type == "audio/wav"
                && artifact.final_chunk
                && artifact.index == 0
                && !artifact.bytes.is_empty()
        }));

        let log = read_log(model_root);
        let generate = log
            .iter()
            .find(|entry| entry["event"] == "generate")
            .expect("generate event");
        let payload = &generate["payload"];
        assert_eq!(
            payload["params"]["caption"],
            "Synth pop with a bright chorus\nStyle: polished and bright\nGenre: synth pop\nTags: anthemic, wide stereo"
        );
        assert_eq!(payload["params"]["task_type"], "text2music");
        assert_eq!(payload["params"]["infer_method"], "ode");
        assert_eq!(payload["params"]["timesteps"], Value::Null);
        assert_eq!(payload["params"]["use_adg"], true);
        assert_eq!(payload["params"]["lm_top_k"], 40);
        assert_eq!(payload["params"]["sampler_mode"], "heun");
        assert_eq!(payload["params"]["dcw_wavelet"], "db4");
        assert_eq!(payload["params"]["retake_seed"], 77);
        assert_eq!(payload["config"]["batch_size"], 2);
        assert_eq!(payload["config"]["audio_format"], "wav32");
        assert!(payload["source_audio"].is_null());
        assert_eq!(payload["reference_audio"]["content_type"], "audio/mpeg");
    }

    #[cfg(unix)]
    #[test]
    fn worker_semantic_validation_consumes_controls_without_generating() {
        let tree = TestTree::new("semantic-validation");
        let primary = tree.model_fixture();
        let model_root = primary.parent().unwrap().parent().unwrap();
        let mut backend =
            AceStepBackend::with_python(tree.mock_worker()).expect("construct backend");
        backend.load(load_config(&primary)).expect("load backend");

        let validation = backend
            .validate_media_generation(
                media_request(json!({
                    "prompt": "Validate controls",
                    "duration_seconds": 600,
                    "steps": 200,
                    "response_format": "wav",
                    "thinking": true,
                })),
                &CancellationToken::new(),
            )
            .expect("validate request")
            .expect("ACE-Step supports semantic validation");

        assert_eq!(
            validation.evidence["worker_operation"],
            "mock/validate-music-v1"
        );
        assert!(validation.handled_request_attributes.contains("prompt"));
        assert!(validation
            .handled_request_attributes
            .contains("duration_seconds"));
        assert!(validation.handled_request_attributes.contains("steps"));
        assert!(validation
            .handled_request_attributes
            .contains("response_format"));
        assert!(validation.handled_request_attributes.contains("thinking"));
        let log = read_log(model_root);
        assert_eq!(
            log.iter()
                .filter(|entry| entry["event"] == "validate")
                .count(),
            1
        );
        assert_eq!(
            log.iter()
                .filter(|entry| entry["event"] == "generate")
                .count(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_kills_worker_and_next_request_lazily_reloads() {
        let tree = TestTree::new("cancel");
        let primary = tree.model_fixture();
        let model_root = primary.parent().unwrap().parent().unwrap();
        let mut backend =
            AceStepBackend::with_python(tree.mock_worker()).expect("construct backend");
        backend.load(load_config(&primary)).expect("load backend");
        let first_pid = backend.process_ids()[0];

        let cancellation = CancellationToken::new();
        let cancellation_thread = cancellation.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            cancellation_thread.cancel();
        });
        let error = backend
            .generate_music(
                media_request(json!({
                    "prompt": "cancel-me",
                    "duration": 10,
                })),
                &mut NoopArtifactSink,
                &cancellation,
            )
            .expect_err("generation should be cancelled");
        canceller.join().expect("join canceller");
        assert!(matches!(error, EngineError::Cancelled));
        assert!(backend.process_ids().is_empty());
        assert!(!backend.component_healthy());

        let mut artifacts = Vec::new();
        backend
            .generate_audio(
                media_request(json!({
                    "prompt": "reload prompt",
                    "duration": 10,
                    "response_format": "flac",
                })),
                &mut |chunk: ArtifactChunk| {
                    artifacts.push(chunk);
                    Ok(())
                },
                &CancellationToken::new(),
            )
            .expect("generate generic audio after lazy reload");
        let second_pid = backend.process_ids()[0];
        assert_ne!(first_pid, second_pid);
        assert_eq!(artifacts.len(), 2);
        let loads = read_log(model_root)
            .into_iter()
            .filter(|entry| entry["event"] == "load")
            .collect::<Vec<_>>();
        assert_eq!(loads.len(), 2);
        assert_ne!(loads[0]["pid"], loads[1]["pid"]);
    }

    #[cfg(unix)]
    #[test]
    fn malformed_payloads_are_rejected_before_worker_generation() {
        let tree = TestTree::new("invalid");
        let primary = tree.model_fixture();
        let model_root = primary.parent().unwrap().parent().unwrap();
        let mut backend =
            AceStepBackend::with_python(tree.mock_worker()).expect("construct backend");
        backend.load(load_config(&primary)).expect("load backend");

        let invalid_bodies = [
            ("unknown field", json!({"prompt": "test", "unknown": true})),
            (
                "prompt alias conflict",
                json!({"prompt": "test", "caption": "also test"}),
            ),
            ("duration low", json!({"prompt": "test", "duration": 9})),
            (
                "steps high",
                json!({"prompt": "test", "inference_steps": 201}),
            ),
            (
                "guidance high",
                json!({"prompt": "test", "guidance_scale": 15.1}),
            ),
            (
                "extract task",
                json!({"prompt": "test", "task_type": "extract"}),
            ),
            ("lego task", json!({"prompt": "test", "task_type": "lego"})),
            (
                "complete task",
                json!({"prompt": "test", "task_type": "complete"}),
            ),
            (
                "server path",
                json!({"prompt": "test", "source_audio": "/server/private/audio.wav"}),
            ),
            (
                "file encoding",
                json!({
                    "prompt": "test",
                    "source_audio": {
                        "encoding": "file",
                        "data": "/server/private/audio.wav",
                        "content_type": "audio/wav"
                    }
                }),
            ),
            (
                "descriptor path",
                json!({
                    "prompt": "test",
                    "source_audio": {
                        "encoding": "base64",
                        "data": "YQ==",
                        "content_type": "audio/wav",
                        "path": "/server/private/audio.wav"
                    }
                }),
            ),
            (
                "timestep order",
                json!({"prompt": "test", "timesteps": [0.0, 1.0]}),
            ),
            (
                "cfg interval",
                json!({"prompt": "test", "cfg_interval_start": 0.8, "cfg_interval_end": 0.2}),
            ),
            (
                "steps alias conflict",
                json!({"prompt": "test", "steps": 10, "inference_steps": 10}),
            ),
            (
                "audio alias conflict",
                json!({"prompt": "test", "source_audio": "YQ==", "src_audio": "Yg=="}),
            ),
            (
                "reserved cot lyrics",
                json!({"prompt": "test", "use_cot_lyrics": true}),
            ),
            (
                "lego-only global caption",
                json!({"prompt": "test", "global_caption": "unsupported"}),
            ),
            (
                "overwritten latent crossfade",
                json!({"prompt": "test", "repaint_latent_crossfade_frames": 20}),
            ),
            (
                "overwritten waveform crossfade",
                json!({"prompt": "test", "repaint_wav_crossfade_sec": 0.5}),
            ),
            (
                "unsupported LM sampling",
                json!({"prompt": "test", "typical_p": 0.9}),
            ),
            (
                "unsupported sample toggle",
                json!({"prompt": "test", "do_sample": true}),
            ),
            (
                "unsupported token cap",
                json!({"prompt": "test", "max_new_tokens": 128}),
            ),
            (
                "dropped repetition field",
                json!({"prompt": "test", "lm_repetition_penalty": 1.1}),
            ),
            (
                "provider model path",
                json!({"prompt": "test", "model_path": "/server/model"}),
            ),
            (
                "provider backend",
                json!({"prompt": "test", "lm_backend": "pt"}),
            ),
            (
                "provider debug",
                json!({"prompt": "test", "constrained_decoding_debug": true}),
            ),
            (
                "provider chunk",
                json!({"prompt": "test", "lm_batch_chunk_size": 2}),
            ),
            (
                "provider offload",
                json!({"prompt": "test", "offload_to_cpu": true}),
            ),
            (
                "bad audio codes",
                json!({"prompt": "test", "audio_codes": "<|audio_code_64000|>"}),
            ),
            (
                "seed alias conflict",
                json!({"prompt": "test", "seed": 1, "seeds": [1]}),
            ),
            (
                "seed batch mismatch",
                json!({"prompt": "test", "batch_size": 2, "seeds": [1]}),
            ),
            (
                "flow controls disabled",
                json!({"prompt": "test", "flow_edit_n_avg": 2}),
            ),
            (
                "mp3 controls on flac",
                json!({"prompt": "test", "response_format": "flac", "mp3_bitrate": "320k"}),
            ),
        ];
        for (label, body) in invalid_bodies {
            let error = backend
                .generate_music(
                    media_request(body),
                    &mut NoopArtifactSink,
                    &CancellationToken::new(),
                )
                .unwrap_err();
            assert!(
                matches!(error, EngineError::InvalidConfig(_)),
                "{label}: {error}"
            );
        }
        assert_eq!(
            read_log(model_root)
                .iter()
                .filter(|entry| entry["event"] == "generate")
                .count(),
            0
        );
    }

    #[cfg(unix)]
    const MOCK_WORKER: &str = r#"#!/usr/bin/env python3
import base64
import ace_runtime_probe
import errno
import json
import os
import pathlib
import socket
import sys
import time

log_path = None

bootstrap_source = base64.b64decode(sys.stdin.buffer.readline())
if b"def main():" not in bootstrap_source:
    raise RuntimeError("missing ACE-Step private-pipe worker bootstrap")

def log(value):
    with log_path.open("a", encoding="utf-8") as output:
        output.write(json.dumps(value, separators=(",", ":")) + "\n")

def reply(message_id, result=None, error=None):
    value = {"id": message_id, "ok": error is None}
    if error is None:
        value["result"] = result
    else:
        value["error"] = error
    print(json.dumps(value, separators=(",", ":")), flush=True)

for line in sys.stdin:
    message = json.loads(line)
    message_id = message["id"]
    operation = message["op"]
    payload = message["payload"]
    if operation == "load":
        model_root = pathlib.Path(payload["model_root"]).resolve()
        worker_cache = pathlib.Path(payload["worker_cache"]).resolve()
        log_path = worker_cache / "mock-worker.jsonl"
        runtime_path = pathlib.Path(ace_runtime_probe.__file__).resolve()
        try:
            runtime_path.write_text("mutated = True\n", encoding="utf-8")
            runtime_read_only = False
        except OSError:
            runtime_read_only = True
        network_denied = None
        if "offline" in str(model_root):
            network_probe = socket.socket()
            try:
                network_result = network_probe.connect_ex(("127.0.0.1", 9))
            finally:
                network_probe.close()
            network_denied = network_result in (errno.EPERM, errno.EACCES)
        log({
            "event": "load",
            "pid": os.getpid(),
            "model_root": str(model_root),
            "source_root": str(pathlib.Path(payload["source_root"]).resolve()),
            "worker_cache": str(worker_cache),
            "runtime_import": ace_runtime_probe.IDENTITY,
            "runtime_read_only": runtime_read_only,
            "network_denied": network_denied,
            "env": {
                name: os.environ.get(name)
                for name in [
                    "HF_HUB_OFFLINE",
                    "TRANSFORMERS_OFFLINE",
                    "DIFFUSERS_OFFLINE",
                    "PIP_NO_INDEX",
                    "UV_OFFLINE",
                    "PYTHONPATH",
                    "TMPDIR",
                    "DARWIN_USER_CACHE_DIR",
                ]
            },
        })
        reply(message_id, {
            "n_ctx_train": 0,
            "n_vocab": 0,
            "execution_config": {
                "device_kind": "cpu",
                "free_memory_bytes": None,
                "total_memory_bytes": None,
                "offload_to_cpu": False,
                "offload_dit_to_cpu": False,
                "quantization": None,
                "selection_basis": "pinned-v0.1.8-cpu-policy",
                "memory_calibration": "acestep/gpu_config.py:v0.1.8-tier-vram-calibration",
                "source_commit": "dce621408bee8c31b4fcf4811682eb9359e1bc94",
            },
        })
    elif operation == "validate_music":
        log({"event": "validate", "pid": os.getpid(), "payload": payload})
        reply(message_id, {
            "worker_operation": "mock/validate-music-v1",
            "generation_params": payload["params"],
            "generation_config": payload["config"],
            "preprocess": payload["preprocess"],
        })
    elif operation == "generate_music":
        log({"event": "generate", "pid": os.getpid(), "payload": payload})
        if payload["params"]["caption"] == "cancel-me":
            time.sleep(30)
        audio_format = payload["config"]["audio_format"]
        content_types = {
            "flac": "audio/flac",
            "mp3": "audio/mpeg",
            "opus": "audio/ogg",
            "aac": "audio/aac",
            "wav": "audio/wav",
            "wav32": "audio/wav",
        }
        duration = payload["params"]["duration"]
        if duration == -1:
            duration = 10.25
        artifacts = []
        for index in range(payload["config"]["batch_size"]):
            artifacts.append({
                "data_base64": base64.b64encode(
                    f"mock-audio-{os.getpid()}-{index}".encode("ascii")
                ).decode("ascii"),
                "content_type": content_types[audio_format],
                "duration_seconds": duration,
            })
        reply(message_id, {
            "artifacts": artifacts,
            "step_count": payload["params"]["inference_steps"],
        })
    elif operation == "shutdown":
        reply(message_id, {"shutdown": True})
        break
    else:
        reply(message_id, error="unsupported operation")
"#;
}
