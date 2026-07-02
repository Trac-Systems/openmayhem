#![forbid(unsafe_code)]

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;

pub const DEFAULT_CHUNK_SIZE: usize = 8 * 1024 * 1024;
pub const MERKLE_KIND: &str = "blake3_merkle_v1";
pub const SEALED_STORE_MANIFEST: &str = "sealed-manifest.json";

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
}
