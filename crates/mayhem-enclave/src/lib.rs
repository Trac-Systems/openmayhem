#![forbid(unsafe_code)]

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use ed25519_dalek::{Signer, SigningKey};
use hkdf::Hkdf;
use mayhem_proto::{
    attestation_report_head, attestation_signing_bytes, catalog_enclave_id, AttestationBody,
    AttestationReport, AttestationSigner, CatalogEnclaveIdentity, ATTESTATION_ALG,
    ATTESTATION_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;

pub const DEFAULT_CHUNK_SIZE: usize = 8 * 1024 * 1024;
pub const MERKLE_KIND: &str = "blake3_merkle_v1";
pub const SEALED_STORE_MANIFEST: &str = "sealed-manifest.json";
pub const RUNTIME_KEYPAIR_STORE: &str = "runtime-keypair.json";
pub const TIER1_ATTESTATION_TIER: u8 = 1;
pub const SANDBOX_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_TCP_PROBE_TIMEOUT_MS: u64 = 2_000;
pub const SATURATION_SHED_THRESHOLD: f64 = 0.9;

type Result<T> = std::result::Result<T, EnclaveError>;

#[derive(Debug, Error)]
pub enum EnclaveError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("hex error: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("download destination has no parent: {0}")]
    DestinationHasNoParent(PathBuf),
    #[error("download returned unexpected HTTP status {status} for {url}")]
    DownloadStatus {
        url: String,
        status: reqwest::StatusCode,
    },
    #[error("artifact merkle root mismatch: expected {expected}, got {actual}")]
    MerkleMismatch { expected: String, actual: String },
    #[error("sealed store already exists and is not empty: {0}")]
    StoreAlreadyExists(PathBuf),
    #[error("sealed store manifest is missing: {0}")]
    SealedManifestMissing(PathBuf),
    #[error("sealed store context mismatch: {field} expected {expected}, got {actual}")]
    ContextMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
    #[error("sealed artifact authentication failed at chunk {chunk_index}")]
    SealedArtifactAuthenticationFailed { chunk_index: u64 },
    #[error("runtime keypair authentication failed")]
    RuntimeKeypairAuthenticationFailed,
    #[error("runtime keypair store already exists: {0}")]
    RuntimeKeypairStoreAlreadyExists(PathBuf),
    #[error("sandbox command cannot be empty")]
    SandboxCommandEmpty,
    #[error("sandbox is not supported on this platform: {0}")]
    SandboxUnsupported(String),
    #[error("outbound TCP unexpectedly succeeded to {addr}")]
    OutboundTcpUnexpectedlySucceeded { addr: String },
    #[error("outbound TCP failed, but not with a sandbox denial: {addr}: {error}")]
    OutboundTcpNotDenied { addr: String, error: String },
    #[error("session already active: {0}")]
    SessionAlreadyActive(String),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("scheduler queue is full: {queued_requests}/{max_queue}")]
    SchedulerQueueFull {
        queued_requests: u32,
        max_queue: u32,
    },
    #[error(
        "sealed chunk hash mismatch at chunk {chunk_index}: expected {expected}, got {actual}"
    )]
    SealedChunkHashMismatch {
        chunk_index: u64,
        expected: String,
        actual: String,
    },
    #[error("crypto error: {0}")]
    Crypto(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeKeyContext {
    pub provider_id: String,
    pub enclave_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeKeypair {
    seed: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct RuntimeKeypairStoreOptions {
    pub path: PathBuf,
    pub context: RuntimeKeyContext,
    pub provider_secret: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeKeypairStore {
    pub schema_version: u32,
    pub alg: String,
    pub cipher: String,
    pub kdf: String,
    pub context: RuntimeKeyContext,
    pub public_key: String,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Clone, Debug)]
pub struct Tier1AttestationOptions {
    pub identity: CatalogEnclaveIdentity,
    pub runtime_keypair: RuntimeKeypair,
    pub provider_signing_seed: [u8; 32],
    pub binary_path: PathBuf,
    pub boot_epoch: u64,
    pub report_ts: u64,
    pub nonce_u: String,
}

#[derive(Clone, Debug)]
pub struct Tier1ExternalProviderAttestationOptions {
    pub identity: CatalogEnclaveIdentity,
    pub runtime_keypair: RuntimeKeypair,
    pub provider_pubkey: String,
    pub binary_path: PathBuf,
    pub boot_epoch: u64,
    pub report_ts: u64,
    pub nonce_u: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Tier1AttestationDraft {
    pub body: AttestationBody,
    pub sig_enclave: String,
    pub provider_signing_message_hex: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Tier1AttestationReport {
    pub report: AttestationReport,
    pub report_head: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxPlatform {
    LinuxSeccompBpf,
    MacosSandboxExec,
    WindowsAppContainer,
}

#[derive(Clone, Debug)]
pub struct SandboxConfig {
    pub sealed_store_dir: PathBuf,
    pub ipc_socket_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SandboxProfile {
    pub schema_version: u32,
    pub platform: SandboxPlatform,
    pub sealed_store_dir: PathBuf,
    pub ipc_socket_path: PathBuf,
    pub policy: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SandboxRunReport {
    pub platform: SandboxPlatform,
    pub command: Vec<String>,
    pub status_code: Option<i32>,
    pub success: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TcpProbeReport {
    pub addr: String,
    pub connected: bool,
    pub denied: bool,
    pub error_kind: Option<String>,
    pub raw_os_error: Option<i32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SchedulerConfig {
    pub max_sessions: u32,
    pub max_queue: u32,
    pub kv_cache_bytes_budget: u64,
    pub target_wait_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionUsage {
    pub session_id: String,
    pub in_tokens: u64,
    pub out_tokens: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MeterCheckpoint {
    pub session_id: String,
    pub seq: u64,
    pub final_checkpoint: bool,
    pub usage: SessionUsage,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScheduledRequest {
    pub request_id: String,
    pub session_id: String,
    pub queued_at_ms: u64,
    pub in_tokens_hint: u32,
    pub max_out_tokens_hint: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaturationInput {
    pub active_sessions: u32,
    pub max_sessions: u32,
    pub queued_requests: u32,
    pub max_queue: u32,
    pub kv_cache_bytes_used: u64,
    pub kv_cache_bytes_budget: u64,
    pub ewma_batch_wait_ms: f64,
    pub target_wait_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaturationBreakdown {
    pub slot_pressure: f64,
    pub queue_pressure: f64,
    pub memory_pressure: f64,
    pub latency_pressure: f64,
    pub saturation: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SchedulerSnapshot {
    pub active_sessions: u32,
    pub max_sessions: u32,
    pub queued_requests: u32,
    pub max_queue: u32,
    pub kv_cache_bytes_used: u64,
    pub kv_cache_bytes_budget: u64,
    pub ewma_batch_wait_ms: f64,
    pub target_wait_ms: u64,
    pub saturation: SaturationBreakdown,
    pub heartbeat_saturation: f64,
    pub should_answer_want: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AdmissionOutcome {
    Accepted {
        session_id: String,
        active_sessions: u32,
        max_sessions: u32,
    },
    RejectedBusy {
        session_id: String,
        active_sessions: u32,
        max_sessions: u32,
        queued_requests: u32,
        retry_after_ms: u64,
        alt_rooms: Vec<String>,
    },
}

#[derive(Clone, Debug)]
pub struct EnclaveScheduler {
    config: SchedulerConfig,
    sessions: BTreeMap<String, SessionState>,
    request_queues: BTreeMap<String, VecDeque<ScheduledRequest>>,
    ready_sessions: VecDeque<String>,
    kv_cache_bytes_used: u64,
    ewma_batch_wait_ms: f64,
}

#[derive(Clone, Debug)]
struct SessionState {
    usage: SessionUsage,
    next_checkpoint_seq: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DownloadSource {
    File(PathBuf),
    Http {
        url: String,
        bearer_token: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub struct DownloadRequest {
    pub source: DownloadSource,
    pub destination: PathBuf,
    pub chunk_size: usize,
    pub expected_merkle_root: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DownloadReport {
    pub destination: PathBuf,
    pub resumed_from: u64,
    pub bytes_written: u64,
    pub total_bytes: u64,
    pub merkle: MerkleManifest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MerkleManifest {
    pub kind: String,
    pub chunk_size: usize,
    pub total_bytes: u64,
    pub root: String,
    pub chunks: Vec<MerkleChunk>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MerkleChunk {
    pub index: u64,
    pub offset: u64,
    pub len: u64,
    pub blake3: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct KeyContext {
    pub provider_id: String,
    pub enclave_id: String,
    pub artifact_root: String,
    pub manifest_hash: String,
}

#[derive(Clone, Debug)]
pub struct SealOptions {
    pub plaintext_path: PathBuf,
    pub store_dir: PathBuf,
    pub key_context: KeyContext,
    pub provider_secret: Vec<u8>,
    pub chunk_size: usize,
    pub expected_merkle_root: Option<String>,
}

#[derive(Clone, Debug)]
pub struct BootOptions {
    pub store_dir: PathBuf,
    pub key_context: KeyContext,
    pub provider_secret: Vec<u8>,
    pub output_path: Option<PathBuf>,
    pub expected_merkle_root: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SealedStoreManifest {
    pub schema_version: u32,
    pub cipher: String,
    pub kdf: String,
    pub merkle: MerkleManifest,
    pub key_context: KeyContext,
    pub chunks: Vec<SealedChunk>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SealedChunk {
    pub index: u64,
    pub offset: u64,
    pub plain_len: u64,
    pub plain_blake3: String,
    pub sealed_path: String,
    pub nonce: String,
    pub sealed_len: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SealReport {
    pub store_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub merkle_root: String,
    pub total_bytes: u64,
    pub chunk_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BootReport {
    pub store_dir: PathBuf,
    pub output_path: Option<PathBuf>,
    pub merkle_root: String,
    pub total_bytes: u64,
    pub chunk_count: usize,
}

impl DownloadRequest {
    pub fn new(source: DownloadSource, destination: impl Into<PathBuf>) -> Self {
        Self {
            source,
            destination: destination.into(),
            chunk_size: DEFAULT_CHUNK_SIZE,
            expected_merkle_root: None,
        }
    }
}

impl RuntimeKeypair {
    pub fn generate() -> Result<Self> {
        Ok(Self {
            seed: random_bytes::<32>()?,
        })
    }

    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self { seed }
    }

    pub fn from_seed_hex(seed_hex: &str) -> Result<Self> {
        Ok(Self {
            seed: decode_fixed::<32>(seed_hex)?,
        })
    }

    pub fn public_key_hex(&self) -> String {
        hex::encode(self.signing_key().verifying_key().to_bytes())
    }

    fn signing_key(&self) -> SigningKey {
        SigningKey::from_bytes(&self.seed)
    }
}

impl RuntimeKeypairStoreOptions {
    pub fn new(
        path: impl Into<PathBuf>,
        context: RuntimeKeyContext,
        provider_secret: Vec<u8>,
    ) -> Self {
        Self {
            path: path.into(),
            context,
            provider_secret,
        }
    }
}

impl SandboxConfig {
    pub fn new(sealed_store_dir: impl Into<PathBuf>, ipc_socket_path: impl Into<PathBuf>) -> Self {
        Self {
            sealed_store_dir: sealed_store_dir.into(),
            ipc_socket_path: ipc_socket_path.into(),
        }
    }
}

impl SchedulerConfig {
    pub fn new(
        max_sessions: u32,
        max_queue: u32,
        kv_cache_bytes_budget: u64,
        target_wait_ms: u64,
    ) -> Self {
        Self {
            max_sessions,
            max_queue,
            kv_cache_bytes_budget,
            target_wait_ms,
        }
    }
}

impl ScheduledRequest {
    pub fn new(
        session_id: impl Into<String>,
        request_id: impl Into<String>,
        queued_at_ms: u64,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            session_id: session_id.into(),
            queued_at_ms,
            in_tokens_hint: 0,
            max_out_tokens_hint: 0,
        }
    }
}

impl EnclaveScheduler {
    pub fn new(config: SchedulerConfig) -> Result<Self> {
        validate_scheduler_config(&config)?;
        Ok(Self {
            config,
            sessions: BTreeMap::new(),
            request_queues: BTreeMap::new(),
            ready_sessions: VecDeque::new(),
            kv_cache_bytes_used: 0,
            ewma_batch_wait_ms: 0.0,
        })
    }

    pub fn config(&self) -> &SchedulerConfig {
        &self.config
    }

    pub fn open_session(
        &mut self,
        session_id: impl Into<String>,
        alt_rooms: Vec<String>,
    ) -> Result<AdmissionOutcome> {
        let session_id = session_id.into();
        if self.sessions.contains_key(&session_id) {
            return Err(EnclaveError::SessionAlreadyActive(session_id));
        }
        let active_sessions = self.active_sessions();
        if active_sessions >= self.config.max_sessions {
            return Ok(AdmissionOutcome::RejectedBusy {
                session_id,
                active_sessions,
                max_sessions: self.config.max_sessions,
                queued_requests: self.queued_requests(),
                retry_after_ms: self.retry_after_ms(),
                alt_rooms,
            });
        }

        self.sessions.insert(
            session_id.clone(),
            SessionState {
                usage: SessionUsage {
                    session_id: session_id.clone(),
                    in_tokens: 0,
                    out_tokens: 0,
                },
                next_checkpoint_seq: 1,
            },
        );
        Ok(AdmissionOutcome::Accepted {
            session_id,
            active_sessions: active_sessions + 1,
            max_sessions: self.config.max_sessions,
        })
    }

    pub fn close_session(&mut self, session_id: &str) -> Result<SessionUsage> {
        let state = self
            .sessions
            .remove(session_id)
            .ok_or_else(|| EnclaveError::SessionNotFound(session_id.to_owned()))?;
        self.request_queues.remove(session_id);
        self.ready_sessions.retain(|queued| queued != session_id);
        Ok(state.usage)
    }

    pub fn enqueue_request(&mut self, request: ScheduledRequest) -> Result<()> {
        if !self.sessions.contains_key(&request.session_id) {
            return Err(EnclaveError::SessionNotFound(request.session_id));
        }
        let queued_requests = self.queued_requests();
        if queued_requests >= self.config.max_queue {
            return Err(EnclaveError::SchedulerQueueFull {
                queued_requests,
                max_queue: self.config.max_queue,
            });
        }

        let queue = self
            .request_queues
            .entry(request.session_id.clone())
            .or_default();
        if queue.is_empty() && !self.ready_sessions.contains(&request.session_id) {
            self.ready_sessions.push_back(request.session_id.clone());
        }
        queue.push_back(request);
        Ok(())
    }

    pub fn next_batch(&mut self, max_items: usize, now_ms: u64) -> Vec<ScheduledRequest> {
        let mut batch = Vec::new();
        while batch.len() < max_items {
            let Some(session_id) = self.ready_sessions.pop_front() else {
                break;
            };
            if !self.sessions.contains_key(&session_id) {
                self.request_queues.remove(&session_id);
                continue;
            }

            let Some((request, has_more_for_session)) =
                self.request_queues.get_mut(&session_id).and_then(|queue| {
                    queue
                        .pop_front()
                        .map(|request| (request, !queue.is_empty()))
                })
            else {
                continue;
            };
            let observed_wait = now_ms.saturating_sub(request.queued_at_ms);
            self.record_batch_wait_ms(observed_wait as f64);
            batch.push(request);

            if has_more_for_session {
                self.ready_sessions.push_back(session_id);
            } else {
                self.request_queues.remove(&session_id);
            }
        }
        batch
    }

    pub fn record_tokens(
        &mut self,
        session_id: &str,
        in_tokens: u64,
        out_tokens: u64,
    ) -> Result<SessionUsage> {
        let state = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| EnclaveError::SessionNotFound(session_id.to_owned()))?;
        state.usage.in_tokens = state.usage.in_tokens.saturating_add(in_tokens);
        state.usage.out_tokens = state.usage.out_tokens.saturating_add(out_tokens);
        Ok(state.usage.clone())
    }

    pub fn checkpoint_session(
        &mut self,
        session_id: &str,
        final_checkpoint: bool,
    ) -> Result<MeterCheckpoint> {
        let state = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| EnclaveError::SessionNotFound(session_id.to_owned()))?;
        let checkpoint = MeterCheckpoint {
            session_id: session_id.to_owned(),
            seq: state.next_checkpoint_seq,
            final_checkpoint,
            usage: state.usage.clone(),
        };
        state.next_checkpoint_seq = state.next_checkpoint_seq.saturating_add(1);
        Ok(checkpoint)
    }

    pub fn update_kv_cache_bytes_used(&mut self, bytes: u64) {
        self.kv_cache_bytes_used = bytes;
    }

    pub fn set_ewma_batch_wait_ms(&mut self, wait_ms: f64) {
        self.ewma_batch_wait_ms = wait_ms.max(0.0);
    }

    pub fn active_sessions(&self) -> u32 {
        self.sessions.len() as u32
    }

    pub fn queued_requests(&self) -> u32 {
        self.request_queues
            .values()
            .map(VecDeque::len)
            .sum::<usize>() as u32
    }

    pub fn snapshot(&self) -> SchedulerSnapshot {
        let input = SaturationInput {
            active_sessions: self.active_sessions(),
            max_sessions: self.config.max_sessions,
            queued_requests: self.queued_requests(),
            max_queue: self.config.max_queue,
            kv_cache_bytes_used: self.kv_cache_bytes_used,
            kv_cache_bytes_budget: self.config.kv_cache_bytes_budget,
            ewma_batch_wait_ms: self.ewma_batch_wait_ms,
            target_wait_ms: self.config.target_wait_ms,
        };
        let saturation = calculate_saturation(&input);
        let shedding = saturation.saturation > SATURATION_SHED_THRESHOLD;
        SchedulerSnapshot {
            active_sessions: input.active_sessions,
            max_sessions: input.max_sessions,
            queued_requests: input.queued_requests,
            max_queue: input.max_queue,
            kv_cache_bytes_used: input.kv_cache_bytes_used,
            kv_cache_bytes_budget: input.kv_cache_bytes_budget,
            ewma_batch_wait_ms: input.ewma_batch_wait_ms,
            target_wait_ms: input.target_wait_ms,
            heartbeat_saturation: if shedding { 1.0 } else { saturation.saturation },
            should_answer_want: !shedding && input.active_sessions < input.max_sessions,
            saturation,
        }
    }

    fn retry_after_ms(&self) -> u64 {
        let queue_factor = u64::from(self.queued_requests() + 1);
        self.config
            .target_wait_ms
            .saturating_mul(queue_factor)
            .max(1_000)
    }

    fn record_batch_wait_ms(&mut self, observed_wait_ms: f64) {
        let observed = observed_wait_ms.max(0.0);
        self.ewma_batch_wait_ms = if self.ewma_batch_wait_ms == 0.0 {
            observed
        } else {
            (0.8 * self.ewma_batch_wait_ms) + (0.2 * observed)
        };
    }
}

impl SealOptions {
    pub fn new(
        plaintext_path: impl Into<PathBuf>,
        store_dir: impl Into<PathBuf>,
        key_context: KeyContext,
        provider_secret: Vec<u8>,
    ) -> Self {
        Self {
            plaintext_path: plaintext_path.into(),
            store_dir: store_dir.into(),
            key_context,
            provider_secret,
            chunk_size: DEFAULT_CHUNK_SIZE,
            expected_merkle_root: None,
        }
    }
}

impl BootOptions {
    pub fn new(
        store_dir: impl Into<PathBuf>,
        key_context: KeyContext,
        provider_secret: Vec<u8>,
    ) -> Self {
        Self {
            store_dir: store_dir.into(),
            key_context,
            provider_secret,
            output_path: None,
            expected_merkle_root: None,
        }
    }
}

impl fmt::Display for DownloadSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(path) => write!(f, "{}", path.display()),
            Self::Http { url, .. } => f.write_str(url),
        }
    }
}

pub fn calculate_saturation(input: &SaturationInput) -> SaturationBreakdown {
    let slot_pressure = bounded_ratio_u32(input.active_sessions, input.max_sessions);
    let queue_pressure = bounded_ratio_u32(input.queued_requests, input.max_queue);
    let memory_pressure = bounded_ratio_u64(input.kv_cache_bytes_used, input.kv_cache_bytes_budget);
    let latency_pressure = bounded_ratio_f64(input.ewma_batch_wait_ms, input.target_wait_ms as f64);
    let saturation = slot_pressure
        .max(queue_pressure)
        .max(memory_pressure)
        .max(latency_pressure)
        .clamp(0.0, 1.0);

    SaturationBreakdown {
        slot_pressure,
        queue_pressure,
        memory_pressure,
        latency_pressure,
        saturation,
    }
}

pub fn download_resumable(request: &DownloadRequest) -> Result<DownloadReport> {
    validate_chunk_size(request.chunk_size)?;
    if let Some(parent) = request.destination.parent() {
        fs::create_dir_all(parent)?;
    } else {
        return Err(EnclaveError::DestinationHasNoParent(
            request.destination.clone(),
        ));
    }

    let part_path = partial_path(&request.destination);
    let resumed_from = fs::metadata(&part_path).map_or(0, |meta| meta.len());

    match &request.source {
        DownloadSource::File(path) => {
            append_file_range(path, &part_path, resumed_from, request.chunk_size)?;
        }
        DownloadSource::Http { url, bearer_token } => {
            download_http_range(url, bearer_token.as_deref(), &part_path, resumed_from)?;
        }
    }

    fs::rename(&part_path, &request.destination)?;
    let merkle = build_merkle_manifest(&request.destination, request.chunk_size)?;
    if let Some(expected) = &request.expected_merkle_root {
        ensure_merkle_root(expected, &merkle.root)?;
    }

    Ok(DownloadReport {
        destination: request.destination.clone(),
        resumed_from,
        bytes_written: merkle.total_bytes.saturating_sub(resumed_from),
        total_bytes: merkle.total_bytes,
        merkle,
    })
}

pub fn build_merkle_manifest(path: &Path, chunk_size: usize) -> Result<MerkleManifest> {
    validate_chunk_size(chunk_size)?;
    let mut reader = BufReader::new(File::open(path)?);
    let mut buffer = vec![0_u8; chunk_size];
    let mut chunks = Vec::new();
    let mut total_bytes = 0_u64;

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let index = chunks.len() as u64;
        let hash = merkle_leaf_hash(index, read as u64, &buffer[..read]);
        chunks.push(MerkleChunk {
            index,
            offset: total_bytes,
            len: read as u64,
            blake3: hex::encode(hash),
        });
        total_bytes = total_bytes.saturating_add(read as u64);
    }

    let leaves = decode_chunk_hashes(&chunks)?;
    Ok(MerkleManifest {
        kind: MERKLE_KIND.to_owned(),
        chunk_size,
        total_bytes,
        root: hex::encode(merkle_root_from_leaves(&leaves)),
        chunks,
    })
}

pub fn verify_file_merkle(
    path: &Path,
    chunk_size: usize,
    expected_root: &str,
) -> Result<MerkleManifest> {
    let merkle = build_merkle_manifest(path, chunk_size)?;
    ensure_merkle_root(expected_root, &merkle.root)?;
    Ok(merkle)
}

pub fn seal_artifact(options: &SealOptions) -> Result<SealReport> {
    validate_chunk_size(options.chunk_size)?;
    validate_key_context(&options.key_context)?;
    validate_provider_secret(&options.provider_secret)?;
    ensure_empty_or_missing_dir(&options.store_dir)?;

    let merkle = build_merkle_manifest(&options.plaintext_path, options.chunk_size)?;
    if let Some(expected) = &options.expected_merkle_root {
        ensure_merkle_root(expected, &merkle.root)?;
    }

    let chunks_dir = options.store_dir.join("chunks");
    fs::create_dir_all(&chunks_dir)?;
    let key = derive_sealing_key(&options.provider_secret, &options.key_context)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|_| EnclaveError::Crypto("invalid AES key".into()))?;

    let mut reader = BufReader::new(File::open(&options.plaintext_path)?);
    let mut buffer = vec![0_u8; options.chunk_size];
    let mut sealed_chunks = Vec::with_capacity(merkle.chunks.len());

    for chunk in &merkle.chunks {
        let read = reader.read(&mut buffer)?;
        if read as u64 != chunk.len {
            return Err(EnclaveError::InvalidInput(format!(
                "artifact changed while sealing at chunk {}",
                chunk.index
            )));
        }

        let actual_hash = hex::encode(merkle_leaf_hash(chunk.index, chunk.len, &buffer[..read]));
        if actual_hash != chunk.blake3 {
            return Err(EnclaveError::InvalidInput(format!(
                "artifact changed while sealing at chunk {}",
                chunk.index
            )));
        }

        let nonce = random_nonce()?;
        let aad = chunk_aad(&options.key_context, &merkle.root, chunk)?;
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &buffer[..read],
                    aad: &aad,
                },
            )
            .map_err(|_| EnclaveError::Crypto("AES-GCM encryption failed".into()))?;
        let sealed_path = format!("chunks/{:08}.seal", chunk.index);
        let absolute_path = options.store_dir.join(&sealed_path);
        fs::write(&absolute_path, &ciphertext)?;
        sealed_chunks.push(SealedChunk {
            index: chunk.index,
            offset: chunk.offset,
            plain_len: chunk.len,
            plain_blake3: chunk.blake3.clone(),
            sealed_path,
            nonce: hex::encode(nonce),
            sealed_len: ciphertext.len() as u64,
        });
    }

    let manifest = SealedStoreManifest {
        schema_version: 1,
        cipher: "AES-256-GCM".to_owned(),
        kdf: "HKDF-SHA256".to_owned(),
        merkle: merkle.clone(),
        key_context: options.key_context.clone(),
        chunks: sealed_chunks,
    };
    let manifest_path = options.store_dir.join(SEALED_STORE_MANIFEST);
    write_json_pretty(&manifest_path, &manifest)?;

    Ok(SealReport {
        store_dir: options.store_dir.clone(),
        manifest_path,
        merkle_root: merkle.root,
        total_bytes: merkle.total_bytes,
        chunk_count: merkle.chunks.len(),
    })
}

pub fn boot_sealed_store(options: &BootOptions) -> Result<BootReport> {
    validate_key_context(&options.key_context)?;
    validate_provider_secret(&options.provider_secret)?;

    let manifest_path = options.store_dir.join(SEALED_STORE_MANIFEST);
    if !manifest_path.exists() {
        return Err(EnclaveError::SealedManifestMissing(manifest_path));
    }
    let manifest: SealedStoreManifest =
        serde_json::from_reader(BufReader::new(File::open(&manifest_path)?))?;
    validate_manifest_context(&options.key_context, &manifest.key_context)?;
    if let Some(expected) = &options.expected_merkle_root {
        ensure_merkle_root(expected, &manifest.merkle.root)?;
    }

    let key = derive_sealing_key(&options.provider_secret, &options.key_context)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|_| EnclaveError::Crypto("invalid AES key".into()))?;
    let mut output = if let Some(path) = &options.output_path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Some(BufWriter::new(File::create(path)?))
    } else {
        None
    };
    let mut leaves = Vec::with_capacity(manifest.chunks.len());
    let mut total_bytes = 0_u64;

    for (position, chunk) in manifest.chunks.iter().enumerate() {
        if chunk.index != position as u64 {
            return Err(EnclaveError::InvalidInput(format!(
                "sealed chunks are not contiguous at position {position}"
            )));
        }
        let merkle_chunk = MerkleChunk {
            index: chunk.index,
            offset: chunk.offset,
            len: chunk.plain_len,
            blake3: chunk.plain_blake3.clone(),
        };
        let aad = chunk_aad(&manifest.key_context, &manifest.merkle.root, &merkle_chunk)?;
        let nonce = decode_fixed::<12>(&chunk.nonce)?;
        let ciphertext = fs::read(options.store_dir.join(&chunk.sealed_path))?;
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: ciphertext.as_slice(),
                    aad: &aad,
                },
            )
            .map_err(|_| EnclaveError::SealedArtifactAuthenticationFailed {
                chunk_index: chunk.index,
            })?;
        let actual_hash = hex::encode(merkle_leaf_hash(chunk.index, chunk.plain_len, &plaintext));
        if actual_hash != chunk.plain_blake3 {
            return Err(EnclaveError::SealedChunkHashMismatch {
                chunk_index: chunk.index,
                expected: chunk.plain_blake3.clone(),
                actual: actual_hash,
            });
        }
        if let Some(writer) = output.as_mut() {
            writer.write_all(&plaintext)?;
        }
        leaves.push(decode_fixed::<32>(&chunk.plain_blake3)?);
        total_bytes = total_bytes.saturating_add(plaintext.len() as u64);
    }

    if let Some(mut writer) = output {
        writer.flush()?;
    }

    let actual_root = hex::encode(merkle_root_from_leaves(&leaves));
    ensure_merkle_root(&manifest.merkle.root, &actual_root)?;
    if total_bytes != manifest.merkle.total_bytes {
        return Err(EnclaveError::InvalidInput(format!(
            "sealed store byte count mismatch: expected {}, got {}",
            manifest.merkle.total_bytes, total_bytes
        )));
    }

    Ok(BootReport {
        store_dir: options.store_dir.clone(),
        output_path: options.output_path.clone(),
        merkle_root: actual_root,
        total_bytes,
        chunk_count: manifest.chunks.len(),
    })
}

pub fn read_sealed_manifest(store_dir: &Path) -> Result<SealedStoreManifest> {
    let manifest_path = store_dir.join(SEALED_STORE_MANIFEST);
    if !manifest_path.exists() {
        return Err(EnclaveError::SealedManifestMissing(manifest_path));
    }
    Ok(serde_json::from_reader(BufReader::new(File::open(
        manifest_path,
    )?))?)
}

pub fn hex_secret(secret_hex: &str) -> Result<Vec<u8>> {
    Ok(hex::decode(secret_hex.trim())?)
}

pub fn provider_signing_seed_from_hex(seed_hex: &str) -> Result<[u8; 32]> {
    decode_fixed::<32>(seed_hex)
}

pub fn measure_binary(path: &Path) -> Result<String> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

pub fn measure_current_binary() -> Result<String> {
    let current = std::env::current_exe()?;
    measure_binary(&current)
}

pub fn unix_timestamp_now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| EnclaveError::InvalidInput(err.to_string()))?
        .as_secs())
}

pub fn create_runtime_keypair_store(
    options: &RuntimeKeypairStoreOptions,
) -> Result<RuntimeKeypair> {
    validate_runtime_key_context(&options.context)?;
    validate_provider_secret(&options.provider_secret)?;
    if options.path.exists() {
        return Err(EnclaveError::RuntimeKeypairStoreAlreadyExists(
            options.path.clone(),
        ));
    }
    if let Some(parent) = options.path.parent() {
        fs::create_dir_all(parent)?;
    }

    let keypair = RuntimeKeypair::generate()?;
    write_runtime_keypair_store(options, &keypair)?;
    Ok(keypair)
}

pub fn load_or_create_runtime_keypair_store(
    options: &RuntimeKeypairStoreOptions,
) -> Result<RuntimeKeypair> {
    if options.path.exists() {
        read_runtime_keypair_store(options)
    } else {
        create_runtime_keypair_store(options)
    }
}

pub fn read_runtime_keypair_store(options: &RuntimeKeypairStoreOptions) -> Result<RuntimeKeypair> {
    validate_runtime_key_context(&options.context)?;
    validate_provider_secret(&options.provider_secret)?;
    let store: RuntimeKeypairStore =
        serde_json::from_reader(BufReader::new(File::open(&options.path)?))?;

    if store.schema_version != 1 {
        return Err(EnclaveError::InvalidInput(format!(
            "runtime keypair store schema_version must be 1, got {}",
            store.schema_version
        )));
    }
    if store.alg != ATTESTATION_ALG {
        return Err(EnclaveError::InvalidInput(format!(
            "runtime keypair alg must be {ATTESTATION_ALG}, got {}",
            store.alg
        )));
    }
    if store.cipher != "AES-256-GCM" || store.kdf != "HKDF-SHA256" {
        return Err(EnclaveError::InvalidInput(
            "runtime keypair store must use AES-256-GCM with HKDF-SHA256".to_owned(),
        ));
    }
    validate_runtime_manifest_context(&options.context, &store.context)?;

    let key = derive_runtime_keypair_store_key(&options.provider_secret, &options.context)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|_| EnclaveError::Crypto("invalid AES key".into()))?;
    let nonce = decode_fixed::<12>(&store.nonce)?;
    let ciphertext = hex::decode(&store.ciphertext)?;
    let aad = runtime_keypair_aad(&store.context, &store.public_key)?;
    let seed = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: ciphertext.as_slice(),
                aad: &aad,
            },
        )
        .map_err(|_| EnclaveError::RuntimeKeypairAuthenticationFailed)?;
    let seed: [u8; 32] = seed
        .try_into()
        .map_err(|_| EnclaveError::InvalidInput("runtime keypair seed must be 32 bytes".into()))?;
    let keypair = RuntimeKeypair::from_seed(seed);
    let actual_public_key = keypair.public_key_hex();
    if actual_public_key != store.public_key {
        return Err(EnclaveError::InvalidInput(format!(
            "runtime keypair public key mismatch: expected {}, got {}",
            store.public_key, actual_public_key
        )));
    }
    Ok(keypair)
}

pub fn write_runtime_keypair_store(
    options: &RuntimeKeypairStoreOptions,
    keypair: &RuntimeKeypair,
) -> Result<RuntimeKeypairStore> {
    validate_runtime_key_context(&options.context)?;
    validate_provider_secret(&options.provider_secret)?;
    if let Some(parent) = options.path.parent() {
        fs::create_dir_all(parent)?;
    }

    let public_key = keypair.public_key_hex();
    let key = derive_runtime_keypair_store_key(&options.provider_secret, &options.context)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|_| EnclaveError::Crypto("invalid AES key".into()))?;
    let nonce = random_nonce()?;
    let aad = runtime_keypair_aad(&options.context, &public_key)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &keypair.seed,
                aad: &aad,
            },
        )
        .map_err(|_| EnclaveError::Crypto("runtime keypair encryption failed".into()))?;

    let store = RuntimeKeypairStore {
        schema_version: 1,
        alg: ATTESTATION_ALG.to_owned(),
        cipher: "AES-256-GCM".to_owned(),
        kdf: "HKDF-SHA256".to_owned(),
        context: options.context.clone(),
        public_key,
        nonce: hex::encode(nonce),
        ciphertext: hex::encode(ciphertext),
    };
    write_json_pretty(&options.path, &store)?;
    Ok(store)
}

pub fn build_tier1_attestation_report(
    options: &Tier1AttestationOptions,
) -> Result<Tier1AttestationReport> {
    validate_identity(&options.identity)?;
    validate_hex_field("nonce_u", &options.nonce_u, 32)?;

    let binary_hash = measure_binary(&options.binary_path)?;
    if !options.identity.binary_hash.is_empty() && options.identity.binary_hash != binary_hash {
        return Err(EnclaveError::InvalidInput(format!(
            "identity binary_hash {} does not match measured binary hash {}",
            options.identity.binary_hash, binary_hash
        )));
    }
    let identity = CatalogEnclaveIdentity {
        binary_hash: binary_hash.clone(),
        ..options.identity.clone()
    };
    let enclave_id = catalog_enclave_id(&identity);
    let provider_signing_key = SigningKey::from_bytes(&options.provider_signing_seed);
    let provider_pubkey = hex::encode(provider_signing_key.verifying_key().to_bytes());
    let body = AttestationBody {
        schema_version: ATTESTATION_SCHEMA_VERSION,
        alg: ATTESTATION_ALG.to_owned(),
        enclave_id,
        enclave_pubkey: options.runtime_keypair.public_key_hex(),
        provider_pubkey,
        manifest_hash: identity.manifest_hash,
        binary_hash,
        att_tier: TIER1_ATTESTATION_TIER,
        hw_quote: None,
        boot_epoch: options.boot_epoch,
        report_ts: options.report_ts,
        nonce_u: options.nonce_u.clone(),
    };

    let enclave_signing_key = options.runtime_keypair.signing_key();
    let sig_enclave =
        sign_attestation_body(&enclave_signing_key, &body, AttestationSigner::Enclave)?;
    let sig_provider =
        sign_attestation_body(&provider_signing_key, &body, AttestationSigner::Provider)?;
    let report = AttestationReport {
        schema_version: body.schema_version,
        alg: body.alg,
        enclave_id: body.enclave_id,
        enclave_pubkey: body.enclave_pubkey,
        provider_pubkey: body.provider_pubkey,
        manifest_hash: body.manifest_hash,
        binary_hash: body.binary_hash,
        att_tier: body.att_tier,
        hw_quote: body.hw_quote,
        boot_epoch: body.boot_epoch,
        report_ts: body.report_ts,
        nonce_u: body.nonce_u,
        sig_enclave,
        sig_provider,
    };
    let report_head =
        attestation_report_head(&report).map_err(|err| EnclaveError::Crypto(err.to_string()))?;
    Ok(Tier1AttestationReport {
        report,
        report_head,
    })
}

pub fn prepare_tier1_attestation_report(
    options: &Tier1ExternalProviderAttestationOptions,
) -> Result<Tier1AttestationDraft> {
    validate_identity(&options.identity)?;
    validate_hex_field("provider_pubkey", &options.provider_pubkey, 32)?;
    validate_hex_field("nonce_u", &options.nonce_u, 32)?;

    let binary_hash = measure_binary(&options.binary_path)?;
    if !options.identity.binary_hash.is_empty() && options.identity.binary_hash != binary_hash {
        return Err(EnclaveError::InvalidInput(format!(
            "identity binary_hash {} does not match measured binary hash {}",
            options.identity.binary_hash, binary_hash
        )));
    }
    let identity = CatalogEnclaveIdentity {
        binary_hash: binary_hash.clone(),
        ..options.identity.clone()
    };
    let body = AttestationBody {
        schema_version: ATTESTATION_SCHEMA_VERSION,
        alg: ATTESTATION_ALG.to_owned(),
        enclave_id: catalog_enclave_id(&identity),
        enclave_pubkey: options.runtime_keypair.public_key_hex(),
        provider_pubkey: options.provider_pubkey.clone(),
        manifest_hash: identity.manifest_hash,
        binary_hash,
        att_tier: TIER1_ATTESTATION_TIER,
        hw_quote: None,
        boot_epoch: options.boot_epoch,
        report_ts: options.report_ts,
        nonce_u: options.nonce_u.clone(),
    };

    let enclave_signing_key = options.runtime_keypair.signing_key();
    let sig_enclave =
        sign_attestation_body(&enclave_signing_key, &body, AttestationSigner::Enclave)?;
    let provider_signing_message_hex = hex::encode(
        attestation_signing_bytes(&body, AttestationSigner::Provider)
            .map_err(|err| EnclaveError::Crypto(err.to_string()))?,
    );

    Ok(Tier1AttestationDraft {
        body,
        sig_enclave,
        provider_signing_message_hex,
    })
}

pub fn finalize_tier1_attestation_report(
    draft: Tier1AttestationDraft,
    sig_provider: impl Into<String>,
) -> Result<Tier1AttestationReport> {
    let sig_provider = sig_provider.into();
    validate_hex_field("sig_provider", &sig_provider, 64)?;
    let report = AttestationReport {
        schema_version: draft.body.schema_version,
        alg: draft.body.alg,
        enclave_id: draft.body.enclave_id,
        enclave_pubkey: draft.body.enclave_pubkey,
        provider_pubkey: draft.body.provider_pubkey,
        manifest_hash: draft.body.manifest_hash,
        binary_hash: draft.body.binary_hash,
        att_tier: draft.body.att_tier,
        hw_quote: draft.body.hw_quote,
        boot_epoch: draft.body.boot_epoch,
        report_ts: draft.body.report_ts,
        nonce_u: draft.body.nonce_u,
        sig_enclave: draft.sig_enclave,
        sig_provider,
    };
    let report_head =
        attestation_report_head(&report).map_err(|err| EnclaveError::Crypto(err.to_string()))?;
    Ok(Tier1AttestationReport {
        report,
        report_head,
    })
}

pub fn current_sandbox_platform() -> Result<SandboxPlatform> {
    if cfg!(target_os = "linux") {
        Ok(SandboxPlatform::LinuxSeccompBpf)
    } else if cfg!(target_os = "macos") {
        Ok(SandboxPlatform::MacosSandboxExec)
    } else if cfg!(target_os = "windows") {
        Ok(SandboxPlatform::WindowsAppContainer)
    } else {
        Err(EnclaveError::SandboxUnsupported(
            std::env::consts::OS.to_owned(),
        ))
    }
}

pub fn build_sandbox_profile(
    config: &SandboxConfig,
    platform: SandboxPlatform,
) -> Result<SandboxProfile> {
    validate_sandbox_config(config)?;
    let policy = match platform {
        SandboxPlatform::LinuxSeccompBpf => linux_seccomp_policy_document(),
        SandboxPlatform::MacosSandboxExec => macos_sandbox_exec_profile(config),
        SandboxPlatform::WindowsAppContainer => windows_appcontainer_policy_document(config)?,
    };
    Ok(SandboxProfile {
        schema_version: SANDBOX_SCHEMA_VERSION,
        platform,
        sealed_store_dir: config.sealed_store_dir.clone(),
        ipc_socket_path: config.ipc_socket_path.clone(),
        policy,
    })
}

pub fn run_sandboxed_command(
    config: &SandboxConfig,
    command: &[String],
) -> Result<SandboxRunReport> {
    validate_sandbox_config(config)?;
    if command.is_empty() {
        return Err(EnclaveError::SandboxCommandEmpty);
    }
    let platform = current_sandbox_platform()?;
    let status = run_platform_sandbox(config, command)?;
    Ok(SandboxRunReport {
        platform,
        command: command.to_vec(),
        status_code: status.code(),
        success: status.success(),
    })
}

pub fn apply_current_process_sandbox(config: &SandboxConfig) -> Result<()> {
    validate_sandbox_config(config)?;
    apply_platform_sandbox(config)
}

pub fn probe_outbound_tcp(addr: &str, timeout: Duration) -> TcpProbeReport {
    let socket_addr = match addr
        .to_socket_addrs()
        .ok()
        .and_then(|mut addrs| addrs.next())
    {
        Some(addr) => addr,
        None => {
            return TcpProbeReport {
                addr: addr.to_owned(),
                connected: false,
                denied: false,
                error_kind: Some("invalid-address".to_owned()),
                raw_os_error: None,
            };
        }
    };

    match TcpStream::connect_timeout(&socket_addr, timeout) {
        Ok(_) => TcpProbeReport {
            addr: addr.to_owned(),
            connected: true,
            denied: false,
            error_kind: None,
            raw_os_error: None,
        },
        Err(err) => {
            let denied = is_tcp_denied_error(&err);
            TcpProbeReport {
                addr: addr.to_owned(),
                connected: false,
                denied,
                error_kind: Some(format!("{:?}", err.kind())),
                raw_os_error: err.raw_os_error(),
            }
        }
    }
}

pub fn expect_outbound_tcp_denied(addr: &str, timeout: Duration) -> Result<TcpProbeReport> {
    let report = probe_outbound_tcp(addr, timeout);
    if report.denied {
        Ok(report)
    } else if report.connected {
        Err(EnclaveError::OutboundTcpUnexpectedlySucceeded {
            addr: addr.to_owned(),
        })
    } else {
        Err(EnclaveError::OutboundTcpNotDenied {
            addr: addr.to_owned(),
            error: format!(
                "kind={:?} raw_os_error={:?}",
                report.error_kind, report.raw_os_error
            ),
        })
    }
}

fn append_file_range(
    source: &Path,
    part_path: &Path,
    start: u64,
    buffer_size: usize,
) -> Result<()> {
    let mut source_file = BufReader::new(File::open(source)?);
    source_file.seek(SeekFrom::Start(start))?;
    let mut destination = OpenOptions::new()
        .create(true)
        .append(true)
        .open(part_path)?;
    let mut buffer = vec![0_u8; buffer_size];
    loop {
        let read = source_file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        destination.write_all(&buffer[..read])?;
    }
    destination.flush()?;
    Ok(())
}

fn download_http_range(
    url: &str,
    bearer_token: Option<&str>,
    part_path: &Path,
    start: u64,
) -> Result<()> {
    let client = reqwest::blocking::Client::new();
    let mut request = client.get(url);
    if let Some(token) = bearer_token {
        request = request.bearer_auth(token);
    }
    if start > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={start}-"));
    }

    let mut response = request.send()?;
    let status = response.status();
    let append = if start > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
        true
    } else if status == reqwest::StatusCode::OK {
        false
    } else {
        return Err(EnclaveError::DownloadStatus {
            url: url.to_owned(),
            status,
        });
    };

    let mut destination = OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(part_path)?;
    std::io::copy(&mut response, &mut destination)?;
    destination.flush()?;
    Ok(())
}

fn partial_path(destination: &Path) -> PathBuf {
    let mut path = destination.to_path_buf();
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("artifact");
    path.set_file_name(format!("{file_name}.part"));
    path
}

fn ensure_empty_or_missing_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if path.read_dir()?.next().is_none() {
        return Ok(());
    }
    Err(EnclaveError::StoreAlreadyExists(path.to_path_buf()))
}

fn write_json_pretty<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut file = BufWriter::new(File::create(path)?);
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

fn validate_chunk_size(chunk_size: usize) -> Result<()> {
    if chunk_size == 0 {
        return Err(EnclaveError::InvalidInput(
            "chunk_size must be greater than zero".to_owned(),
        ));
    }
    Ok(())
}

fn validate_provider_secret(secret: &[u8]) -> Result<()> {
    if secret.len() < 32 {
        return Err(EnclaveError::InvalidInput(
            "provider_secret must contain at least 32 bytes".to_owned(),
        ));
    }
    Ok(())
}

fn validate_key_context(context: &KeyContext) -> Result<()> {
    for (field, value) in [
        ("provider_id", &context.provider_id),
        ("enclave_id", &context.enclave_id),
        ("artifact_root", &context.artifact_root),
        ("manifest_hash", &context.manifest_hash),
    ] {
        if value.trim().is_empty() {
            return Err(EnclaveError::InvalidInput(format!(
                "{field} cannot be empty"
            )));
        }
    }
    Ok(())
}

fn validate_runtime_key_context(context: &RuntimeKeyContext) -> Result<()> {
    for (field, value) in [
        ("provider_id", &context.provider_id),
        ("enclave_id", &context.enclave_id),
    ] {
        if value.trim().is_empty() {
            return Err(EnclaveError::InvalidInput(format!(
                "{field} cannot be empty"
            )));
        }
    }
    Ok(())
}

fn validate_identity(identity: &CatalogEnclaveIdentity) -> Result<()> {
    for (field, value) in [
        ("admin_pubkey", &identity.admin_pubkey),
        ("model_id", &identity.model_id),
        ("artifact_root", &identity.artifact_root),
        ("manifest_hash", &identity.manifest_hash),
    ] {
        if value.trim().is_empty() {
            return Err(EnclaveError::InvalidInput(format!(
                "{field} cannot be empty"
            )));
        }
    }
    Ok(())
}

fn validate_sandbox_config(config: &SandboxConfig) -> Result<()> {
    if config.sealed_store_dir.as_os_str().is_empty() {
        return Err(EnclaveError::InvalidInput(
            "sealed_store_dir cannot be empty".to_owned(),
        ));
    }
    if config.ipc_socket_path.as_os_str().is_empty() {
        return Err(EnclaveError::InvalidInput(
            "ipc_socket_path cannot be empty".to_owned(),
        ));
    }
    Ok(())
}

fn validate_scheduler_config(config: &SchedulerConfig) -> Result<()> {
    if config.max_sessions == 0 {
        return Err(EnclaveError::InvalidInput(
            "max_sessions must be greater than zero".to_owned(),
        ));
    }
    if config.target_wait_ms == 0 {
        return Err(EnclaveError::InvalidInput(
            "target_wait_ms must be greater than zero".to_owned(),
        ));
    }
    Ok(())
}

fn validate_hex_field(field: &str, value: &str, bytes: usize) -> Result<()> {
    if value.len() != bytes * 2 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(EnclaveError::InvalidInput(format!(
            "{field} must be {bytes} bytes of hex"
        )));
    }
    Ok(())
}

fn bounded_ratio_u32(numerator: u32, denominator: u32) -> f64 {
    if denominator == 0 {
        if numerator == 0 {
            0.0
        } else {
            1.0
        }
    } else {
        (f64::from(numerator) / f64::from(denominator)).clamp(0.0, 1.0)
    }
}

fn bounded_ratio_u64(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        if numerator == 0 {
            0.0
        } else {
            1.0
        }
    } else {
        (numerator as f64 / denominator as f64).clamp(0.0, 1.0)
    }
}

fn bounded_ratio_f64(numerator: f64, denominator: f64) -> f64 {
    if denominator <= 0.0 {
        if numerator <= 0.0 {
            0.0
        } else {
            1.0
        }
    } else {
        (numerator / denominator).clamp(0.0, 1.0)
    }
}

fn macos_sandbox_exec_profile(config: &SandboxConfig) -> String {
    let sealed_store = sandbox_quote_path(&config.sealed_store_dir);
    let ipc_socket = sandbox_quote_path(&config.ipc_socket_path);
    let ipc_parent = config
        .ipc_socket_path
        .parent()
        .map(sandbox_quote_path)
        .unwrap_or_else(|| "\"/tmp\"".to_owned());

    format!(
        r#"(version 1)
(allow default)
(deny network*)
(allow file-read* (subpath {sealed_store}))
(deny file-write* (subpath {sealed_store}))
(allow file-read* file-write* (literal {ipc_socket}))
(allow file-read* file-write* (subpath {ipc_parent}))
"#
    )
}

fn linux_seccomp_policy_document() -> String {
    serde_json::json!({
        "schema_version": SANDBOX_SCHEMA_VERSION,
        "kind": "seccomp-bpf",
        "default_action": "allow",
        "match_action": "errno(EPERM)",
        "blocked_syscalls": [
            { "syscall": "socket", "arg": "domain", "values": ["AF_INET", "AF_INET6", "AF_PACKET", "AF_NETLINK"] }
        ],
        "fs": {
            "sealed_store": "read-only by mount/permissions before applying seccomp",
            "ipc_socket": "AF_UNIX only"
        }
    })
    .to_string()
}

fn windows_appcontainer_policy_document(config: &SandboxConfig) -> Result<String> {
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "schema_version": SANDBOX_SCHEMA_VERSION,
        "kind": "appcontainer",
        "capabilities": [],
        "network": "no internetClient/privateNetworkClientServer capability",
        "sealed_store": {
            "path": config.sealed_store_dir,
            "access": "read-only"
        },
        "ipc_socket": {
            "path": config.ipc_socket_path,
            "access": "only named-pipe/AF_UNIX IPC endpoint granted to the container"
        }
    }))?)
}

fn sandbox_quote_path(path: &Path) -> String {
    format!(
        "\"{}\"",
        path.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    )
}

fn run_platform_sandbox(config: &SandboxConfig, command: &[String]) -> Result<ExitStatus> {
    #[cfg(target_os = "macos")]
    {
        let profile = build_sandbox_profile(config, SandboxPlatform::MacosSandboxExec)?;
        Command::new("sandbox-exec")
            .arg("-p")
            .arg(profile.policy)
            .args(command)
            .status()
            .map_err(EnclaveError::Io)
    }

    #[cfg(target_os = "linux")]
    {
        apply_current_process_sandbox(config)?;
        Command::new(&command[0])
            .args(&command[1..])
            .status()
            .map_err(EnclaveError::Io)
    }

    #[cfg(target_os = "windows")]
    {
        let _ = (config, command);
        Err(EnclaveError::SandboxUnsupported(
            "AppContainer process launch is not implemented in this build".to_owned(),
        ))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (config, command);
        Err(EnclaveError::SandboxUnsupported(
            std::env::consts::OS.to_owned(),
        ))
    }
}

fn apply_platform_sandbox(config: &SandboxConfig) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let _ = config;
        linux::apply_network_deny_seccomp()
    }

    #[cfg(target_os = "macos")]
    {
        let _ = config;
        Err(EnclaveError::SandboxUnsupported(
            "macOS sandbox-exec must spawn a sandboxed child process".to_owned(),
        ))
    }

    #[cfg(target_os = "windows")]
    {
        let _ = config;
        Err(EnclaveError::SandboxUnsupported(
            "Windows AppContainer process launch is not implemented in this build".to_owned(),
        ))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = config;
        Err(EnclaveError::SandboxUnsupported(
            std::env::consts::OS.to_owned(),
        ))
    }
}

fn is_tcp_denied_error(err: &std::io::Error) -> bool {
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        return true;
    }
    matches!(err.raw_os_error(), Some(1 | 13))
}

fn validate_manifest_context(expected: &KeyContext, actual: &KeyContext) -> Result<()> {
    compare_context_field("provider_id", &expected.provider_id, &actual.provider_id)?;
    compare_context_field("enclave_id", &expected.enclave_id, &actual.enclave_id)?;
    compare_context_field(
        "artifact_root",
        &expected.artifact_root,
        &actual.artifact_root,
    )?;
    compare_context_field(
        "manifest_hash",
        &expected.manifest_hash,
        &actual.manifest_hash,
    )?;
    Ok(())
}

fn validate_runtime_manifest_context(
    expected: &RuntimeKeyContext,
    actual: &RuntimeKeyContext,
) -> Result<()> {
    compare_context_field("provider_id", &expected.provider_id, &actual.provider_id)?;
    compare_context_field("enclave_id", &expected.enclave_id, &actual.enclave_id)?;
    Ok(())
}

fn compare_context_field(field: &'static str, expected: &str, actual: &str) -> Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(EnclaveError::ContextMismatch {
            field,
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        })
    }
}

fn ensure_merkle_root(expected: &str, actual: &str) -> Result<()> {
    if expected.eq_ignore_ascii_case(actual) {
        Ok(())
    } else {
        Err(EnclaveError::MerkleMismatch {
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        })
    }
}

fn decode_chunk_hashes(chunks: &[MerkleChunk]) -> Result<Vec<[u8; 32]>> {
    chunks
        .iter()
        .map(|chunk| decode_fixed::<32>(&chunk.blake3))
        .collect()
}

fn decode_fixed<const N: usize>(value: &str) -> Result<[u8; N]> {
    let bytes = hex::decode(value)?;
    bytes
        .try_into()
        .map_err(|_| EnclaveError::InvalidInput(format!("expected {N} bytes of hex")))
}

fn random_nonce() -> Result<[u8; 12]> {
    let mut nonce = [0_u8; 12];
    getrandom::fill(&mut nonce).map_err(|err| EnclaveError::Crypto(err.to_string()))?;
    Ok(nonce)
}

fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).map_err(|err| EnclaveError::Crypto(err.to_string()))?;
    Ok(bytes)
}

fn derive_sealing_key(secret: &[u8], context: &KeyContext) -> Result<[u8; 32]> {
    let context_bytes = serde_json::to_vec(context)?;
    let mut salt_hasher = blake3::Hasher::new();
    salt_hasher.update(b"mayhem-enclave-sealing-salt-v1");
    salt_hasher.update(&context_bytes);
    let salt = salt_hasher.finalize();

    let hkdf = Hkdf::<Sha256>::new(Some(salt.as_bytes()), secret);
    let mut key = [0_u8; 32];
    hkdf.expand(b"mayhem-enclave-sealed-weights-v1", &mut key)
        .map_err(|_| EnclaveError::Crypto("HKDF expand failed".to_owned()))?;
    Ok(key)
}

fn derive_runtime_keypair_store_key(
    secret: &[u8],
    context: &RuntimeKeyContext,
) -> Result<[u8; 32]> {
    let context_bytes = serde_json::to_vec(context)?;
    let mut salt_hasher = blake3::Hasher::new();
    salt_hasher.update(b"mayhem-runtime-keypair-store-salt-v1");
    salt_hasher.update(&context_bytes);
    let salt = salt_hasher.finalize();

    let hkdf = Hkdf::<Sha256>::new(Some(salt.as_bytes()), secret);
    let mut key = [0_u8; 32];
    hkdf.expand(b"mayhem-enclave-runtime-keypair-v1", &mut key)
        .map_err(|_| EnclaveError::Crypto("HKDF expand failed".to_owned()))?;
    Ok(key)
}

fn chunk_aad(context: &KeyContext, merkle_root: &str, chunk: &MerkleChunk) -> Result<Vec<u8>> {
    #[derive(Serialize)]
    struct ChunkAad<'a> {
        schema: &'static str,
        context: &'a KeyContext,
        merkle_root: &'a str,
        chunk_index: u64,
        chunk_offset: u64,
        chunk_len: u64,
        chunk_blake3: &'a str,
    }

    Ok(serde_json::to_vec(&ChunkAad {
        schema: "mayhem-sealed-chunk-aad-v1",
        context,
        merkle_root,
        chunk_index: chunk.index,
        chunk_offset: chunk.offset,
        chunk_len: chunk.len,
        chunk_blake3: &chunk.blake3,
    })?)
}

fn runtime_keypair_aad(context: &RuntimeKeyContext, public_key: &str) -> Result<Vec<u8>> {
    #[derive(Serialize)]
    struct RuntimeKeypairAad<'a> {
        schema: &'static str,
        context: &'a RuntimeKeyContext,
        public_key: &'a str,
    }

    Ok(serde_json::to_vec(&RuntimeKeypairAad {
        schema: "mayhem-runtime-keypair-aad-v1",
        context,
        public_key,
    })?)
}

fn sign_attestation_body(
    signing_key: &SigningKey,
    body: &AttestationBody,
    signer: AttestationSigner,
) -> Result<String> {
    let bytes = attestation_signing_bytes(body, signer)
        .map_err(|err| EnclaveError::Crypto(err.to_string()))?;
    Ok(hex::encode(signing_key.sign(&bytes).to_bytes()))
}

fn merkle_leaf_hash(index: u64, len: u64, data: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mayhem-blake3-merkle-v1:leaf");
    hasher.update(&index.to_le_bytes());
    hasher.update(&len.to_le_bytes());
    hasher.update(data);
    *hasher.finalize().as_bytes()
}

fn merkle_parent_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mayhem-blake3-merkle-v1:node");
    hasher.update(left);
    hasher.update(right);
    *hasher.finalize().as_bytes()
}

fn merkle_empty_hash() -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mayhem-blake3-merkle-v1:empty");
    *hasher.finalize().as_bytes()
}

fn merkle_root_from_leaves(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        return merkle_empty_hash();
    }

    let mut level = leaves.to_vec();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        for pair in level.chunks(2) {
            let left = pair[0];
            let right = if pair.len() == 2 { pair[1] } else { pair[0] };
            next.push(merkle_parent_hash(&left, &right));
        }
        level = next;
    }
    level[0]
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{EnclaveError, Result};
    use seccompiler::{
        BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
        SeccompRule,
    };
    use std::collections::BTreeMap;
    use std::convert::TryInto;

    pub fn apply_network_deny_seccomp() -> Result<()> {
        let forbidden_domains = [
            libc::AF_INET,
            libc::AF_INET6,
            libc::AF_PACKET,
            libc::AF_NETLINK,
        ];
        let mut socket_rules = Vec::with_capacity(forbidden_domains.len());
        for domain in forbidden_domains {
            socket_rules.push(
                SeccompRule::new(vec![SeccompCondition::new(
                    0,
                    SeccompCmpArgLen::Dword,
                    SeccompCmpOp::Eq,
                    domain as u64,
                )
                .map_err(seccomp_error)?])
                .map_err(seccomp_error)?,
            );
        }

        let rules = BTreeMap::from([(libc::SYS_socket, socket_rules)]);
        let filter = SeccompFilter::new(
            rules,
            SeccompAction::Allow,
            SeccompAction::Errno(libc::EPERM as u32),
            std::env::consts::ARCH.try_into().map_err(seccomp_error)?,
        )
        .map_err(seccomp_error)?;
        let bpf: BpfProgram = filter.try_into().map_err(seccomp_error)?;
        seccompiler::apply_filter(&bpf).map_err(seccomp_error)
    }

    fn seccomp_error<E: std::fmt::Display>(err: E) -> EnclaveError {
        EnclaveError::SandboxUnsupported(format!("seccomp-bpf setup failed: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn test_context(root: String) -> KeyContext {
        KeyContext {
            provider_id: "provider-test".to_owned(),
            enclave_id: "enclave-test".to_owned(),
            artifact_root: root,
            manifest_hash: "manifest-test".to_owned(),
        }
    }

    fn test_secret() -> Vec<u8> {
        (0_u8..32).collect()
    }

    fn runtime_context() -> RuntimeKeyContext {
        RuntimeKeyContext {
            provider_id: "provider-test".to_owned(),
            enclave_id: "enclave-test".to_owned(),
        }
    }

    fn sandbox_config() -> SandboxConfig {
        SandboxConfig::new("/sealed/store", "/tmp/mayhem-ipc.sock")
    }

    fn scheduler_config() -> SchedulerConfig {
        SchedulerConfig::new(4, 8, 1_000, 100)
    }

    #[test]
    fn merkle_manifest_changes_when_chunk_changes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("artifact.bin");
        fs::write(&path, b"abcdefghij")?;

        let before = build_merkle_manifest(&path, 4)?;
        fs::write(&path, b"abcdxfghij")?;
        let after = build_merkle_manifest(&path, 4)?;

        assert_ne!(before.root, after.root);
        assert_eq!(before.chunks.len(), 3);
        Ok(())
    }

    #[test]
    fn local_download_resumes_from_partial_file_and_verifies_merkle() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("download.bin");
        let payload = b"0123456789abcdefghijklmnopqrstuvwxyz";
        fs::write(&source, payload)?;
        fs::write(partial_path(&destination), &payload[..11])?;
        let expected = build_merkle_manifest(&source, 7)?;

        let mut request =
            DownloadRequest::new(DownloadSource::File(source.clone()), destination.clone());
        request.chunk_size = 7;
        request.expected_merkle_root = Some(expected.root.clone());
        let report = download_resumable(&request)?;

        assert_eq!(report.resumed_from, 11);
        assert_eq!(fs::read(&destination)?, payload);
        assert_eq!(report.merkle.root, expected.root);
        Ok(())
    }

    #[test]
    fn bit_flipped_sealed_chunk_fails_boot_with_clear_error() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let artifact = temp.path().join("artifact.bin");
        let store = temp.path().join("sealed");
        let output = temp.path().join("booted.bin");
        let payload = b"model-weights-but-small-for-the-test";
        fs::write(&artifact, payload)?;
        let merkle = build_merkle_manifest(&artifact, 8)?;
        let context = test_context(merkle.root.clone());
        let mut seal = SealOptions::new(&artifact, &store, context.clone(), test_secret());
        seal.chunk_size = 8;
        seal.expected_merkle_root = Some(merkle.root.clone());
        seal_artifact(&seal)?;

        let first_chunk = store.join("chunks/00000000.seal");
        let mut bytes = fs::read(&first_chunk)?;
        bytes[0] ^= 0x80;
        fs::write(&first_chunk, bytes)?;

        let mut boot = BootOptions::new(&store, context, test_secret());
        boot.output_path = Some(output);
        let err = boot_sealed_store(&boot).expect_err("tamper must fail");

        assert!(matches!(
            err,
            EnclaveError::SealedArtifactAuthenticationFailed { chunk_index: 0 }
        ));
        assert!(err.to_string().contains("authentication failed"));
        Ok(())
    }

    #[test]
    fn sealed_store_round_trips_and_rejects_wrong_provider_secret() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let artifact = temp.path().join("artifact.bin");
        let store = temp.path().join("sealed");
        let output = temp.path().join("booted.bin");
        fs::write(&artifact, b"roundtrip artifact data")?;
        let merkle = build_merkle_manifest(&artifact, 5)?;
        let context = test_context(merkle.root.clone());
        let mut seal = SealOptions::new(&artifact, &store, context.clone(), test_secret());
        seal.chunk_size = 5;
        seal_artifact(&seal)?;

        let mut boot = BootOptions::new(&store, context.clone(), test_secret());
        boot.output_path = Some(output.clone());
        let report = boot_sealed_store(&boot)?;
        assert_eq!(report.merkle_root, merkle.root);
        assert_eq!(fs::read(&output)?, b"roundtrip artifact data");

        let wrong_secret = vec![7_u8; 32];
        let err = boot_sealed_store(&BootOptions::new(&store, context, wrong_secret))
            .expect_err("wrong provider secret must not decrypt");
        assert!(matches!(
            err,
            EnclaveError::SealedArtifactAuthenticationFailed { chunk_index: 0 }
        ));
        Ok(())
    }

    #[test]
    fn boot_rejects_expected_root_mismatch_before_decrypting() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let artifact = temp.path().join("artifact.bin");
        let store = temp.path().join("sealed");
        fs::write(&artifact, b"root mismatch")?;
        let merkle = build_merkle_manifest(&artifact, 4)?;
        let context = test_context(merkle.root);
        let mut seal = SealOptions::new(&artifact, &store, context.clone(), test_secret());
        seal.chunk_size = 4;
        seal_artifact(&seal)?;

        let mut boot = BootOptions::new(&store, context, test_secret());
        boot.expected_merkle_root = Some("00".repeat(32));
        let err = boot_sealed_store(&boot).expect_err("wrong expected root must fail");
        assert!(matches!(err, EnclaveError::MerkleMismatch { .. }));
        Ok(())
    }

    #[test]
    fn empty_file_has_stable_merkle_root() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let artifact = temp.path().join("empty.bin");
        File::create(&artifact)?.flush()?;

        let first = build_merkle_manifest(&artifact, 16)?;
        let second = build_merkle_manifest(&artifact, 16)?;

        assert_eq!(first.root, second.root);
        assert_eq!(first.total_bytes, 0);
        assert!(first.chunks.is_empty());
        Ok(())
    }

    #[test]
    fn runtime_keypair_store_round_trips_and_rejects_wrong_secret() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join(RUNTIME_KEYPAIR_STORE);
        let options = RuntimeKeypairStoreOptions::new(&path, runtime_context(), test_secret());
        let keypair = RuntimeKeypair::from_seed([9_u8; 32]);
        let store = write_runtime_keypair_store(&options, &keypair)?;

        let loaded = read_runtime_keypair_store(&options)?;
        assert_eq!(loaded.public_key_hex(), keypair.public_key_hex());
        assert_eq!(store.public_key, keypair.public_key_hex());

        let wrong_secret_options =
            RuntimeKeypairStoreOptions::new(&path, runtime_context(), vec![7_u8; 32]);
        let err = read_runtime_keypair_store(&wrong_secret_options)
            .expect_err("wrong secret must not decrypt runtime keypair");
        assert!(matches!(
            err,
            EnclaveError::RuntimeKeypairAuthenticationFailed
        ));
        Ok(())
    }

    #[test]
    fn tier1_report_self_measures_binary_and_rejects_mismatched_identity_hash() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let binary = temp.path().join("mayhem-enclave-test-bin");
        fs::write(&binary, b"test binary bytes")?;
        let binary_hash = measure_binary(&binary)?;
        let identity = CatalogEnclaveIdentity {
            admin_pubkey: "admin".to_owned(),
            model_id: "model".to_owned(),
            artifact_root: "artifact".to_owned(),
            manifest_hash: "manifest".to_owned(),
            binary_hash: binary_hash.clone(),
        };
        let report = build_tier1_attestation_report(&Tier1AttestationOptions {
            identity: identity.clone(),
            runtime_keypair: RuntimeKeypair::from_seed([9_u8; 32]),
            provider_signing_seed: [7_u8; 32],
            binary_path: binary.clone(),
            boot_epoch: 100,
            report_ts: 200,
            nonce_u: "aa".repeat(32),
        })?;
        assert_eq!(report.report.binary_hash, binary_hash);
        assert_eq!(report.report.manifest_hash, identity.manifest_hash);
        assert_eq!(report.report.att_tier, TIER1_ATTESTATION_TIER);

        let mut wrong_identity = identity;
        wrong_identity.binary_hash = "00".repeat(32);
        let err = build_tier1_attestation_report(&Tier1AttestationOptions {
            identity: wrong_identity,
            runtime_keypair: RuntimeKeypair::from_seed([9_u8; 32]),
            provider_signing_seed: [7_u8; 32],
            binary_path: binary,
            boot_epoch: 100,
            report_ts: 200,
            nonce_u: "aa".repeat(32),
        })
        .expect_err("wrong identity binary hash must fail before signing");
        assert!(err.to_string().contains("does not match measured"));
        Ok(())
    }

    #[test]
    fn tier1_external_provider_signature_keeps_provider_pubkey() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let binary = temp.path().join("mayhem-enclave-test-bin");
        fs::write(&binary, b"test binary bytes")?;
        let binary_hash = measure_binary(&binary)?;
        let provider_key = SigningKey::from_bytes(&[11_u8; 32]);
        let provider_pubkey = hex::encode(provider_key.verifying_key().to_bytes());
        let identity = CatalogEnclaveIdentity {
            admin_pubkey: "admin".to_owned(),
            model_id: "model".to_owned(),
            artifact_root: "artifact".to_owned(),
            manifest_hash: "manifest".to_owned(),
            binary_hash,
        };
        let draft = prepare_tier1_attestation_report(&Tier1ExternalProviderAttestationOptions {
            identity,
            runtime_keypair: RuntimeKeypair::from_seed([9_u8; 32]),
            provider_pubkey: provider_pubkey.clone(),
            binary_path: binary,
            boot_epoch: 100,
            report_ts: 200,
            nonce_u: "aa".repeat(32),
        })?;
        let provider_message = hex::decode(&draft.provider_signing_message_hex)?;
        let sig_provider = provider_key.sign(&provider_message);
        let report =
            finalize_tier1_attestation_report(draft, hex::encode(sig_provider.to_bytes()))?;

        assert_eq!(report.report.provider_pubkey, provider_pubkey);
        assert_eq!(
            report.report.enclave_id,
            catalog_enclave_id(&CatalogEnclaveIdentity {
                admin_pubkey: "admin".to_owned(),
                model_id: "model".to_owned(),
                artifact_root: "artifact".to_owned(),
                manifest_hash: "manifest".to_owned(),
                binary_hash: report.report.binary_hash.clone(),
            })
        );
        Ok(())
    }

    #[test]
    fn macos_sandbox_profile_denies_network_and_sealed_store_writes() -> Result<()> {
        let profile = build_sandbox_profile(&sandbox_config(), SandboxPlatform::MacosSandboxExec)?;

        assert_eq!(profile.schema_version, SANDBOX_SCHEMA_VERSION);
        assert!(profile.policy.contains("(deny network*)"));
        assert!(profile.policy.contains("(deny file-write*"));
        assert!(profile.policy.contains("/sealed/store"));
        assert!(profile.policy.contains("/tmp/mayhem-ipc.sock"));
        Ok(())
    }

    #[test]
    fn linux_sandbox_profile_documents_seccomp_network_deny() -> Result<()> {
        let profile = build_sandbox_profile(&sandbox_config(), SandboxPlatform::LinuxSeccompBpf)?;

        assert!(profile.policy.contains("seccomp-bpf"));
        assert!(profile.policy.contains("AF_INET"));
        assert!(profile.policy.contains("AF_INET6"));
        assert!(profile.policy.contains("AF_UNIX only"));
        Ok(())
    }

    #[test]
    fn windows_sandbox_profile_uses_appcontainer_without_network_capabilities() -> Result<()> {
        let profile =
            build_sandbox_profile(&sandbox_config(), SandboxPlatform::WindowsAppContainer)?;

        assert!(profile.policy.contains("appcontainer"));
        assert!(profile.policy.contains("\"capabilities\": []"));
        assert!(profile.policy.contains("no internetClient"));
        assert!(profile.policy.contains("read-only"));
        Ok(())
    }

    #[test]
    fn saturation_formula_uses_max_pressure_and_caps_at_one() {
        let input = SaturationInput {
            active_sessions: 1,
            max_sessions: 4,
            queued_requests: 2,
            max_queue: 8,
            kv_cache_bytes_used: 900,
            kv_cache_bytes_budget: 1_000,
            ewma_batch_wait_ms: 250.0,
            target_wait_ms: 100,
        };

        let sat = calculate_saturation(&input);

        assert_eq!(sat.slot_pressure, 0.25);
        assert_eq!(sat.queue_pressure, 0.25);
        assert_eq!(sat.memory_pressure, 0.9);
        assert_eq!(sat.latency_pressure, 1.0);
        assert_eq!(sat.saturation, 1.0);
    }

    #[test]
    fn synthetic_slot_load_produces_expected_saturation_curve() -> Result<()> {
        let mut scheduler = EnclaveScheduler::new(scheduler_config())?;
        let mut curve = vec![scheduler.snapshot().saturation.saturation];

        for session in ["s1", "s2", "s3", "s4"] {
            assert!(matches!(
                scheduler.open_session(session, vec![])?,
                AdmissionOutcome::Accepted { .. }
            ));
            curve.push(scheduler.snapshot().saturation.saturation);
        }

        assert_eq!(curve, vec![0.0, 0.25, 0.5, 0.75, 1.0]);
        assert_eq!(scheduler.snapshot().heartbeat_saturation, 1.0);
        assert!(!scheduler.snapshot().should_answer_want);
        Ok(())
    }

    #[test]
    fn max_sessions_hard_cap_is_enforced() -> Result<()> {
        let mut scheduler = EnclaveScheduler::new(SchedulerConfig::new(2, 4, 1_000, 100))?;
        scheduler.open_session("s1", vec![])?;
        scheduler.open_session("s2", vec![])?;

        let outcome = scheduler.open_session("s3", vec!["room-b".to_owned()])?;

        assert!(matches!(
            outcome,
            AdmissionOutcome::RejectedBusy {
                active_sessions: 2,
                max_sessions: 2,
                ..
            }
        ));
        assert_eq!(scheduler.active_sessions(), 2);
        Ok(())
    }

    #[test]
    fn round_robin_batch_scheduler_is_fair_across_sessions() -> Result<()> {
        let mut scheduler = EnclaveScheduler::new(scheduler_config())?;
        scheduler.open_session("a", vec![])?;
        scheduler.open_session("b", vec![])?;
        for request in ["a1", "a2", "a3"] {
            scheduler.enqueue_request(ScheduledRequest::new("a", request, 0))?;
        }
        for request in ["b1", "b2"] {
            scheduler.enqueue_request(ScheduledRequest::new("b", request, 0))?;
        }

        let batch = scheduler.next_batch(4, 50);
        let ids: Vec<_> = batch
            .iter()
            .map(|request| request.request_id.as_str())
            .collect();

        assert_eq!(ids, vec!["a1", "b1", "a2", "b2"]);
        assert_eq!(scheduler.queued_requests(), 1);
        assert_eq!(scheduler.snapshot().ewma_batch_wait_ms, 50.0);
        Ok(())
    }

    #[test]
    fn queue_and_latency_pressure_drive_synthetic_load_curve() -> Result<()> {
        let mut scheduler = EnclaveScheduler::new(SchedulerConfig::new(4, 4, 1_000, 100))?;
        scheduler.open_session("a", vec![])?;
        scheduler.enqueue_request(ScheduledRequest::new("a", "a1", 0))?;
        scheduler.enqueue_request(ScheduledRequest::new("a", "a2", 0))?;

        let queued = scheduler.snapshot().saturation;
        assert_eq!(queued.slot_pressure, 0.25);
        assert_eq!(queued.queue_pressure, 0.5);
        assert_eq!(queued.saturation, 0.5);

        let batch = scheduler.next_batch(1, 150);
        assert_eq!(batch.len(), 1);
        let waited = scheduler.snapshot().saturation;
        assert_eq!(waited.latency_pressure, 1.0);
        assert_eq!(waited.saturation, 1.0);
        Ok(())
    }

    #[test]
    fn per_session_meter_counters_and_checkpoints_are_isolated() -> Result<()> {
        let mut scheduler = EnclaveScheduler::new(scheduler_config())?;
        scheduler.open_session("a", vec![])?;
        scheduler.open_session("b", vec![])?;

        let usage_a = scheduler.record_tokens("a", 11, 29)?;
        let usage_b = scheduler.record_tokens("b", 3, 5)?;
        scheduler.record_tokens("a", 7, 13)?;
        let checkpoint_a1 = scheduler.checkpoint_session("a", false)?;
        let checkpoint_a2 = scheduler.checkpoint_session("a", true)?;
        let checkpoint_b1 = scheduler.checkpoint_session("b", true)?;

        assert_eq!(
            usage_a,
            SessionUsage {
                session_id: "a".to_owned(),
                in_tokens: 11,
                out_tokens: 29,
            }
        );
        assert_eq!(usage_b.in_tokens, 3);
        assert_eq!(checkpoint_a1.seq, 1);
        assert_eq!(checkpoint_a1.usage.in_tokens, 18);
        assert_eq!(checkpoint_a1.usage.out_tokens, 42);
        assert_eq!(checkpoint_a2.seq, 2);
        assert!(checkpoint_a2.final_checkpoint);
        assert_eq!(checkpoint_b1.seq, 1);
        assert_eq!(checkpoint_b1.usage.out_tokens, 5);
        Ok(())
    }
}
