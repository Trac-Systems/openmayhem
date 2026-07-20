use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write as _};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use ed25519_dalek::{Signature, VerifyingKey};
use flate2::read::{DeflateDecoder, MultiGzDecoder};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const INTERCOM_BUNDLE_ASSET_PREFIX: &str = "share/mayhem/intercom/";
pub const MAYHEM_ASSET_ROOT: &str = "share/mayhem";
pub const RELEASE_BIN_ROOT: &str = "bin";
pub const RELEASE_MANIFEST_PATH: &str = "manifest.json";
pub const RELEASE_CHECKSUMS_PATH: &str = "SHA256SUMS";
pub const RELEASE_STAGE_METADATA_PATH: &str = "stage.json";
pub const RELEASE_STAGE_PAYLOAD_ROOT: &str = "release";
pub const RELEASE_STAGE_SIGNATURE_PATH: &str = "manifest.json.sig";

const RELEASE_MANIFEST_SIGNATURE_DOMAIN: &[u8] = b"mayhem.release-manifest.v1\n";
const INTERCOM_TREE_DIGEST_DOMAIN: &[u8] = b"mayhem-intercom-runtime-tree-v1\0";
const RELEASE_TREE_DIGEST_DOMAIN: &[u8] = b"mayhem-release-payload-tree-v1\0";
const RELEASE_STAGE_SCHEMA: u32 = 1;
const RELEASE_ANTI_ROLLBACK_FLOOR_SCHEMA: u32 = 1;
const REQUIRED_RELEASE_BINARY_BASE_NAMES: &[&str] = &[
    "mayhem",
    "mayhem-gateway",
    "mayhem-pay",
    "mayhemd",
    "mayhem-enclave",
    "mayhem-paygate",
    "mayhem-attestation-verifier",
];
const ZIP_END_OF_CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0605_4b50;
const ZIP_CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0201_4b50;
const ZIP_LOCAL_FILE_HEADER_SIGNATURE: u32 = 0x0403_4b50;
const ZIP_MAX_COMMENT_SIZE: u64 = u16::MAX as u64;
const ACTIVATION_JOURNAL_SCHEMA: u32 = 1;
const ACTIVATION_JOURNAL_FILE: &str = ".mayhem-activation-journal";
const ACTIVATION_JOURNAL_RECORD: &str = "journal.json";
const ACTIVATION_PHASE_READY: &str = "00-ready";
const ACTIVATION_PHASE_ACTIVATING: &str = "10-activating";
const ACTIVATION_PHASE_HEALTH_GATE: &str = "20-health-gate";
const ACTIVATION_PHASE_COMMITTING: &str = "30-committing";
const ACTIVATION_PHASE_ROLLING_BACK: &str = "30-rolling-back";
const MAX_JAVASCRIPT_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const COPY_BUFFER_SIZE: usize = 64 * 1024;

static NEXT_ACTIVATION_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReleaseExtractionLimits {
    pub max_archive_bytes: u64,
    pub max_archive_metadata_bytes: u64,
    pub max_expanded_archive_bytes: u64,
    pub max_file_bytes: u64,
    pub max_total_file_bytes: u64,
    pub max_entries: usize,
    pub max_path_bytes: usize,
}

impl Default for ReleaseExtractionLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: 2 * 1024 * 1024 * 1024,
            max_archive_metadata_bytes: 64 * 1024 * 1024,
            max_expanded_archive_bytes: 8 * 1024 * 1024 * 1024,
            max_file_bytes: 2 * 1024 * 1024 * 1024,
            max_total_file_bytes: 8 * 1024 * 1024 * 1024,
            max_entries: 250_000,
            max_path_bytes: 1024,
        }
    }
}

impl ReleaseExtractionLimits {
    fn validate(&self) -> Result<()> {
        if self.max_archive_bytes == 0
            || self.max_archive_metadata_bytes == 0
            || self.max_expanded_archive_bytes == 0
            || self.max_file_bytes == 0
            || self.max_total_file_bytes == 0
            || self.max_entries == 0
            || self.max_path_bytes == 0
        {
            bail!("release extraction limits must all be positive");
        }
        if self.max_file_bytes > self.max_total_file_bytes {
            bail!("release max_file_bytes must not exceed max_total_file_bytes");
        }
        if self.max_archive_metadata_bytes > self.max_archive_bytes {
            bail!("release max_archive_metadata_bytes must not exceed max_archive_bytes");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseDetachedSignature {
    pub schema_version: u32,
    pub alg: String,
    pub signed_path: String,
    pub key_id: String,
    pub public_key: String,
    pub sha256: String,
    pub sig: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustedReleaseKey {
    pub key_id: String,
    pub public_key: String,
}

impl TrustedReleaseKey {
    pub fn new(key_id: impl Into<String>, public_key: impl Into<String>) -> Self {
        Self {
            key_id: key_id.into(),
            public_key: public_key.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseStageMetadata {
    pub schema: u32,
    pub source_git_sha: String,
    pub manifest_sha256: String,
    pub signature_sha256: String,
    pub payload_tree_sha256: String,
    pub payload_file_count: u64,
    pub staged_at_unix: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct ReleaseAntiRollbackFloor {
    schema: u32,
    version: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseBundleManifest {
    pub schema: u32,
    pub name: String,
    pub version: String,
    pub target: String,
    pub built_at_utc: String,
    pub source_git_sha: String,
    pub binaries: Vec<ReleaseBundleBinary>,
    pub assets: Vec<ReleaseBundleAsset>,
    pub intercom: IntercomBundleManifest,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseBundleBinary {
    pub name: String,
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseBundleAsset {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IntercomBundleManifest {
    pub schema: u32,
    pub release_version: String,
    pub contract_version: u64,
    pub contract_code_sha256: String,
    pub assets: Vec<IntercomBundleAsset>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IntercomBundleAsset {
    pub path: String,
    pub sha256: String,
}

impl ReleaseBundleManifest {
    pub fn validate(&self) -> Result<()> {
        if self.schema != 1 {
            bail!("release bundle manifest schema must be 1");
        }
        if self.name != "mayhem" {
            bail!("release bundle manifest name must be mayhem");
        }
        normalized_release_version(&self.version)?;
        if self.target.is_empty()
            || !self.target.is_ascii()
            || self
                .target
                .bytes()
                .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'-' | b'_' | b'.'))
        {
            bail!("release bundle target is invalid: {:?}", self.target);
        }
        if self.built_at_utc.trim().is_empty()
            || !self.built_at_utc.is_ascii()
            || self
                .built_at_utc
                .bytes()
                .any(|byte| byte.is_ascii_control())
        {
            bail!("release bundle built_at_utc is invalid");
        }
        if !is_lower_hex_len(&self.source_git_sha, 40) {
            bail!(
                "release bundle source_git_sha must be exactly 40 lowercase hexadecimal characters"
            );
        }
        if self.assets.is_empty() {
            bail!("release bundle assets must be a non-empty array");
        }

        let mut assets_by_path = BTreeMap::new();
        let mut portable_asset_paths = BTreeSet::new();
        let mut previous: Option<&str> = None;
        for (index, asset) in self.assets.iter().enumerate() {
            validate_bundle_relative_path(
                &asset.path,
                &format!("release bundle assets[{index}].path"),
            )?;
            if matches!(
                asset.path.as_str(),
                RELEASE_MANIFEST_PATH | RELEASE_CHECKSUMS_PATH
            ) {
                bail!(
                    "release bundle assets must exclude verified metadata path: {}",
                    asset.path
                );
            }
            if !is_lower_sha256(&asset.sha256) {
                bail!("release bundle assets[{index}].sha256 must be lowercase SHA-256 hex");
            }
            if previous.is_some_and(|previous| previous >= asset.path.as_str()) {
                bail!(
                    "release bundle asset paths must be unique and sorted in ascending byte order"
                );
            }
            if !portable_asset_paths.insert(portable_path_key(&asset.path)) {
                bail!(
                    "release bundle asset paths must be unique under portable case folding: {}",
                    asset.path
                );
            }
            if previous.is_some_and(|previous| {
                asset.path.starts_with(previous)
                    && asset.path.as_bytes().get(previous.len()) == Some(&b'/')
            }) {
                bail!(
                    "release bundle asset paths must not contain a file beneath another file: {}",
                    asset.path
                );
            }
            assets_by_path.insert(asset.path.as_str(), asset);
            previous = Some(asset.path.as_str());
        }
        for portable_path in &portable_asset_paths {
            let mut prefix = String::new();
            for (index, segment) in portable_path.split('/').enumerate() {
                if index != 0 {
                    if portable_asset_paths.contains(&prefix) {
                        bail!(
                            "release bundle asset paths must not contain a file beneath another file"
                        );
                    }
                    prefix.push('/');
                }
                prefix.push_str(segment);
            }
        }

        if self.binaries.is_empty() {
            bail!("release bundle must list binaries");
        }
        let mut binary_names = BTreeSet::new();
        let mut binary_paths = BTreeSet::new();
        let mut portable_binary_names = BTreeSet::new();
        let mut portable_binary_paths = BTreeSet::new();
        for (index, binary) in self.binaries.iter().enumerate() {
            validate_portable_file_name(
                &binary.name,
                &format!("release bundle binaries[{index}].name"),
            )?;
            validate_bundle_relative_path(
                &binary.path,
                &format!("release bundle binaries[{index}].path"),
            )?;
            let relative_binary = binary
                .path
                .strip_prefix(&format!("{RELEASE_BIN_ROOT}/"))
                .with_context(|| {
                    format!(
                        "release bundle binaries[{index}].path must be directly under \
                         {RELEASE_BIN_ROOT}/"
                    )
                })?;
            if relative_binary.contains('/') {
                bail!(
                    "release bundle binaries[{index}].path must be directly under \
                     {RELEASE_BIN_ROOT}/"
                );
            }
            if !is_lower_sha256(&binary.sha256) {
                bail!("release bundle binaries[{index}].sha256 must be lowercase SHA-256 hex");
            }
            if !binary_names.insert(binary.name.as_str()) {
                bail!(
                    "release bundle contains duplicate binary name: {}",
                    binary.name
                );
            }
            if !portable_binary_names.insert(portable_path_key(&binary.name)) {
                bail!(
                    "release bundle contains case-insensitive duplicate binary name: {}",
                    binary.name
                );
            }
            if !binary_paths.insert(binary.path.as_str()) {
                bail!(
                    "release bundle contains duplicate binary path: {}",
                    binary.path
                );
            }
            if !portable_binary_paths.insert(portable_path_key(&binary.path)) {
                bail!(
                    "release bundle contains case-insensitive duplicate binary path: {}",
                    binary.path
                );
            }
            let file_name = Path::new(&binary.path)
                .file_name()
                .and_then(|name| name.to_str())
                .context("release bundle binary path must have a UTF-8 file name")?;
            if file_name != binary.name {
                bail!(
                    "release bundle binary name {} does not match path {}",
                    binary.name,
                    binary.path
                );
            }
            let asset = assets_by_path.get(binary.path.as_str()).with_context(|| {
                format!(
                    "release bundle binary path is missing from payload assets: {}",
                    binary.path
                )
            })?;
            if asset.sha256 != binary.sha256 {
                bail!(
                    "release bundle binary {} hash does not match its payload asset",
                    binary.name
                );
            }
        }
        for asset in self.assets.iter().filter(|asset| {
            asset.path == RELEASE_BIN_ROOT
                || asset.path.starts_with(&format!("{RELEASE_BIN_ROOT}/"))
        }) {
            if !binary_paths.contains(asset.path.as_str()) {
                bail!(
                    "signed bin payload file is not listed as a release binary: {}",
                    asset.path
                );
            }
        }
        let primary_name = primary_binary_name(&self.target);
        if !self
            .binaries
            .iter()
            .any(|binary| binary.name == primary_name)
        {
            bail!("release bundle does not include primary binary {primary_name}");
        }
        for required in required_release_binary_names(&self.target) {
            if !self.binaries.iter().any(|binary| binary.name == required) {
                bail!("release bundle does not include required sibling binary {required}");
            }
        }

        self.intercom.validate()?;
        verify_release_version_binding(&self.version, &self.intercom)?;
        for intercom_asset in &self.intercom.assets {
            let outer_asset = assets_by_path
                .get(intercom_asset.path.as_str())
                .with_context(|| {
                    format!(
                        "Intercom asset is missing from outer release assets: {}",
                        intercom_asset.path
                    )
                })?;
            if outer_asset.sha256 != intercom_asset.sha256 {
                bail!(
                    "Intercom asset {} hash does not match outer release asset",
                    intercom_asset.path
                );
            }
        }
        if !self
            .assets
            .iter()
            .any(|asset| asset.path.starts_with(&format!("{MAYHEM_ASSET_ROOT}/")))
        {
            bail!("release bundle must contain the complete {MAYHEM_ASSET_ROOT} asset root");
        }
        Ok(())
    }
}

impl IntercomBundleManifest {
    pub fn validate(&self) -> Result<()> {
        if self.schema != 1 {
            bail!("Intercom bundle manifest schema must be 1");
        }
        if !is_explicit_semantic_version(&self.release_version) {
            bail!("Intercom bundle release_version must be an explicit semantic version");
        }
        if self.contract_version == 0 || self.contract_version > MAX_JAVASCRIPT_SAFE_INTEGER {
            bail!("Intercom bundle contract_version must be a positive safe integer");
        }
        if !is_lower_sha256(&self.contract_code_sha256) {
            bail!("Intercom bundle contract_code_sha256 must be lowercase SHA-256 hex");
        }
        if self.assets.is_empty() {
            bail!("Intercom bundle assets must be a non-empty array");
        }

        let mut seen = BTreeSet::new();
        let mut previous: Option<&str> = None;
        for (index, asset) in self.assets.iter().enumerate() {
            let relative = asset
                .path
                .strip_prefix(INTERCOM_BUNDLE_ASSET_PREFIX)
                .with_context(|| {
                    format!(
                        "Intercom bundle assets[{index}].path must start with \
                         {INTERCOM_BUNDLE_ASSET_PREFIX}"
                    )
                })?;
            validate_bundle_relative_path(
                relative,
                &format!("Intercom bundle assets[{index}].path"),
            )?;
            if !seen.insert(asset.path.as_str()) {
                bail!(
                    "Intercom bundle assets contains duplicate path: {}",
                    asset.path
                );
            }
            if previous.is_some_and(|previous| previous > asset.path.as_str()) {
                bail!("Intercom bundle asset paths must be sorted in ascending byte order");
            }
            if !is_lower_sha256(&asset.sha256) {
                bail!("Intercom bundle assets[{index}].sha256 must be lowercase SHA-256 hex");
            }
            previous = Some(asset.path.as_str());
        }
        Ok(())
    }
}

pub fn verify_release_version_binding(
    release_version: &str,
    intercom: &IntercomBundleManifest,
) -> Result<()> {
    let normalized = normalized_release_version(release_version)?;
    if normalized != intercom.release_version {
        bail!(
            "outer release version {release_version} does not bind to Intercom release version {}",
            intercom.release_version
        );
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedReleaseTree {
    release_root: PathBuf,
    bin_root: PathBuf,
    primary_binary: PathBuf,
    asset_root: PathBuf,
    file_count: usize,
    payload_tree_sha256: String,
}

impl VerifiedReleaseTree {
    pub fn release_root(&self) -> &Path {
        &self.release_root
    }

    pub fn bin_root(&self) -> &Path {
        &self.bin_root
    }

    pub fn primary_binary(&self) -> &Path {
        &self.primary_binary
    }

    pub fn primary_binary_in(&self, bin_root: &Path) -> PathBuf {
        bin_root.join(
            self.primary_binary
                .file_name()
                .expect("verified primary binary has a file name"),
        )
    }

    pub fn asset_root(&self) -> &Path {
        &self.asset_root
    }

    pub fn file_count(&self) -> usize {
        self.file_count
    }

    pub fn payload_tree_sha256(&self) -> &str {
        &self.payload_tree_sha256
    }
}

pub fn verify_staged_release_tree(
    manifest: &ReleaseBundleManifest,
    signed_manifest_bytes: &[u8],
    stage_root: &Path,
) -> Result<VerifiedReleaseTree> {
    manifest.validate()?;
    ensure_real_directory(stage_root, "staged release root")?;

    let signed_manifest: ReleaseBundleManifest = serde_json::from_slice(signed_manifest_bytes)
        .context("parsing signed release manifest bytes")?;
    if &signed_manifest != manifest {
        bail!("parsed release manifest does not match the signed manifest bytes");
    }
    let archived_manifest = read_real_file(&stage_root.join(RELEASE_MANIFEST_PATH))?;
    if archived_manifest != signed_manifest_bytes {
        bail!("staged manifest.json does not exactly match the signed manifest bytes");
    }

    let actual = inventory_tree(stage_root, "staged release")?;
    if !actual.contains_key(RELEASE_MANIFEST_PATH) {
        bail!("staged release is missing verified metadata: {RELEASE_MANIFEST_PATH}");
    }
    if !actual.contains_key(RELEASE_CHECKSUMS_PATH) {
        bail!("staged release is missing verified metadata: {RELEASE_CHECKSUMS_PATH}");
    }

    let expected = manifest
        .assets
        .iter()
        .map(|asset| (asset.path.as_str(), asset))
        .collect::<BTreeMap<_, _>>();
    let expected_directories = directory_ancestors(
        expected
            .keys()
            .copied()
            .chain([RELEASE_MANIFEST_PATH, RELEASE_CHECKSUMS_PATH]),
    );
    verify_directory_inventory(stage_root, &expected_directories, "staged release")?;
    for relative in expected.keys() {
        if !actual.contains_key(*relative) {
            bail!("listed release payload file is missing: {relative}");
        }
    }
    for relative in actual.keys() {
        if !matches!(
            relative.as_str(),
            RELEASE_MANIFEST_PATH | RELEASE_CHECKSUMS_PATH
        ) && !expected.contains_key(relative.as_str())
        {
            bail!("staged release contains unlisted extra payload file: {relative}");
        }
    }
    for (relative, asset) in &expected {
        let actual_sha256 = actual
            .get(*relative)
            .expect("missing and extra release files checked above");
        if actual_sha256 != &asset.sha256 {
            bail!(
                "release payload file {relative} SHA-256 mismatch \
                 (expected {}, actual {actual_sha256})",
                asset.sha256
            );
        }
    }

    verify_checksum_metadata(stage_root, &actual)?;

    let primary_binary = manifest
        .binaries
        .iter()
        .find(|binary| binary.name == primary_binary_name(&manifest.target))
        .expect("primary binary checked by manifest validation");
    let bin_root = stage_root.join(RELEASE_BIN_ROOT);
    ensure_real_directory(&bin_root, "staged release bin root")?;
    let primary_binary = bin_root.join(&primary_binary.name);
    ensure_real_file(&primary_binary, "staged primary binary")?;
    let asset_root = stage_root.join(MAYHEM_ASSET_ROOT);
    ensure_real_directory(&asset_root, "staged Mayhem asset root")?;

    Ok(VerifiedReleaseTree {
        release_root: stage_root.to_path_buf(),
        bin_root,
        primary_binary,
        asset_root,
        file_count: expected.len(),
        payload_tree_sha256: release_tree_digest(&manifest.assets),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedReleaseStage {
    stage_root: PathBuf,
    metadata: ReleaseStageMetadata,
}

impl PreparedReleaseStage {
    pub fn stage_root(&self) -> &Path {
        &self.stage_root
    }

    pub fn payload_root(&self) -> PathBuf {
        self.stage_root.join(RELEASE_STAGE_PAYLOAD_ROOT)
    }

    pub fn metadata(&self) -> &ReleaseStageMetadata {
        &self.metadata
    }

    pub fn source_git_sha(&self) -> &str {
        &self.metadata.source_git_sha
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct AuthenticatedReleaseStage {
    stage_root: PathBuf,
    metadata: ReleaseStageMetadata,
    manifest: ReleaseBundleManifest,
    signed_manifest_bytes: Vec<u8>,
    signature: ReleaseDetachedSignature,
    verified: VerifiedReleaseTree,
    anti_rollback_floor_path: PathBuf,
    anti_rollback_floor: ReleaseAntiRollbackFloor,
}

impl AuthenticatedReleaseStage {
    pub fn stage_root(&self) -> &Path {
        &self.stage_root
    }

    pub fn metadata(&self) -> &ReleaseStageMetadata {
        &self.metadata
    }

    pub fn manifest(&self) -> &ReleaseBundleManifest {
        &self.manifest
    }

    pub fn signature(&self) -> &ReleaseDetachedSignature {
        &self.signature
    }

    pub fn verified_release(&self) -> &VerifiedReleaseTree {
        &self.verified
    }

    pub fn anti_rollback_floor_path(&self) -> &Path {
        &self.anti_rollback_floor_path
    }

    pub fn authenticated_floor_version(&self) -> &str {
        &self.anti_rollback_floor.version
    }

    pub fn source_git_sha(&self) -> &str {
        &self.manifest.source_git_sha
    }
}

/// Safely extracts and atomically publishes a release stage.
///
/// This validates the detached signature envelope but deliberately does not
/// establish trust in its key. `reauthenticate_release_stage` must be called
/// with the live trust set and anti-rollback floor immediately before apply.
pub fn stage_release_archive(
    archive_path: &Path,
    signed_manifest_bytes: &[u8],
    detached_signature_bytes: &[u8],
    stage_root: &Path,
    staged_at_unix: u64,
    limits: &ReleaseExtractionLimits,
) -> Result<PreparedReleaseStage> {
    limits.validate()?;
    validate_absolute_path(stage_root, "release stage root")?;
    if staged_at_unix > MAX_JAVASCRIPT_SAFE_INTEGER {
        bail!("release staged_at_unix must be a JavaScript-safe integer");
    }

    let manifest: ReleaseBundleManifest = serde_json::from_slice(signed_manifest_bytes)
        .context("parsing signed release manifest bytes for staging")?;
    manifest.validate()?;
    let signature: ReleaseDetachedSignature = serde_json::from_slice(detached_signature_bytes)
        .context("parsing detached release signature for staging")?;
    validate_release_signature_envelope(&signature, &manifest, signed_manifest_bytes)?;

    ensure_real_file(archive_path, "release archive")?;
    let archive_size = fs::metadata(archive_path)
        .with_context(|| format!("inspecting release archive {}", archive_path.display()))?
        .len();
    if archive_size > limits.max_archive_bytes {
        bail!(
            "release archive exceeds max_archive_bytes ({} > {})",
            archive_size,
            limits.max_archive_bytes
        );
    }
    let archive_format = release_archive_format(archive_path, &manifest.target)?;

    let stage_parent = stage_root
        .parent()
        .context("release stage root must have a parent")?;
    ensure_real_directory(stage_parent, "release stage parent")?;
    ensure_path_absent(stage_root, "release stage root")?;
    let temporary_stage = stage_parent.join(format!(".mayhem-stage-{}", new_activation_id()));
    ensure_path_absent(&temporary_stage, "temporary release stage")?;
    fs::create_dir(&temporary_stage).with_context(|| {
        format!(
            "creating temporary release stage {}",
            temporary_stage.display()
        )
    })?;
    sync_directory(stage_parent)?;

    let stage_result = (|| -> Result<ReleaseStageMetadata> {
        let payload_root = temporary_stage.join(RELEASE_STAGE_PAYLOAD_ROOT);
        fs::create_dir(&payload_root)
            .with_context(|| format!("creating staged payload root {}", payload_root.display()))?;
        extract_release_archive_bounded(
            archive_path,
            archive_format,
            &manifest,
            &payload_root,
            limits,
        )?;
        let verified = verify_staged_release_tree(&manifest, signed_manifest_bytes, &payload_root)?;

        create_new_synced_file(
            &temporary_stage.join(RELEASE_STAGE_SIGNATURE_PATH),
            detached_signature_bytes,
            "staged detached release signature",
        )?;
        let metadata = ReleaseStageMetadata {
            schema: RELEASE_STAGE_SCHEMA,
            source_git_sha: manifest.source_git_sha.clone(),
            manifest_sha256: sha256_bytes_hex(signed_manifest_bytes),
            signature_sha256: sha256_bytes_hex(detached_signature_bytes),
            payload_tree_sha256: verified.payload_tree_sha256().to_owned(),
            payload_file_count: u64::try_from(verified.file_count())
                .context("release payload file count does not fit u64")?,
            staged_at_unix,
        };
        let mut metadata_bytes =
            serde_json::to_vec_pretty(&metadata).context("serializing release stage metadata")?;
        metadata_bytes.push(b'\n');
        create_new_synced_file(
            &temporary_stage.join(RELEASE_STAGE_METADATA_PATH),
            &metadata_bytes,
            "release stage metadata",
        )?;
        sync_directory(&payload_root)?;
        sync_directory(&temporary_stage)?;
        Ok(metadata)
    })();

    let metadata = match stage_result {
        Ok(metadata) => metadata,
        Err(error) => {
            let _ = fs::remove_dir_all(&temporary_stage);
            let _ = sync_directory(stage_parent);
            return Err(error);
        }
    };
    if let Err(error) = fs::rename(&temporary_stage, stage_root).with_context(|| {
        format!(
            "publishing release stage {} at {}",
            temporary_stage.display(),
            stage_root.display()
        )
    }) {
        let _ = fs::remove_dir_all(&temporary_stage);
        let _ = sync_directory(stage_parent);
        return Err(error);
    }
    sync_directory(stage_parent)?;

    Ok(PreparedReleaseStage {
        stage_root: stage_root.to_path_buf(),
        metadata,
    })
}

/// Strictly parses and authenticates a detached release manifest.
pub(crate) fn authenticate_release_manifest(
    signed_manifest_bytes: &[u8],
    detached_signature_bytes: &[u8],
    trusted_keys: &BTreeMap<String, String>,
    expected_key_id: Option<&str>,
) -> Result<(ReleaseBundleManifest, ReleaseDetachedSignature)> {
    let manifest: ReleaseBundleManifest =
        serde_json::from_slice(signed_manifest_bytes).context("parsing signed release manifest")?;
    manifest.validate()?;
    let signature: ReleaseDetachedSignature = serde_json::from_slice(detached_signature_bytes)
        .context("parsing detached release signature")?;
    verify_release_manifest_signature(
        &signature,
        &manifest,
        signed_manifest_bytes,
        trusted_keys,
        expected_key_id,
    )?;
    Ok((manifest, signature))
}

fn trusted_release_key_map(trusted_keys: &[TrustedReleaseKey]) -> Result<BTreeMap<String, String>> {
    let mut trusted_by_id = BTreeMap::new();
    for trusted in trusted_keys {
        validate_release_key_id(&trusted.key_id)?;
        if !is_lower_hex_len(&trusted.public_key, 64) {
            bail!(
                "trusted release key {} must be lowercase 32-byte hex",
                trusted.key_id
            );
        }
        if trusted_by_id
            .insert(trusted.key_id.clone(), trusted.public_key.clone())
            .is_some()
        {
            bail!(
                "trusted release key set contains duplicate id {}",
                trusted.key_id
            );
        }
    }
    Ok(trusted_by_id)
}

/// Reopens a stage using live trust and rollback state.
///
/// The trusted key set and anti-rollback floor must come from protected state
/// read at apply time. No identity, target, key, hash, or version from
/// `stage.json` is trusted without deriving it again from the signed manifest
/// and payload.
pub fn reauthenticate_release_stage(
    stage_root: &Path,
    expected_target: &str,
    anti_rollback_floor_path: &Path,
    trusted_keys: &[TrustedReleaseKey],
    expected_key_id: Option<&str>,
) -> Result<AuthenticatedReleaseStage> {
    validate_absolute_path(stage_root, "release stage root")?;
    validate_release_stage_envelope(stage_root)?;
    let anti_rollback_floor = read_release_anti_rollback_floor_record(anti_rollback_floor_path)?;

    let metadata_bytes = read_real_file(&stage_root.join(RELEASE_STAGE_METADATA_PATH))?;
    let metadata: ReleaseStageMetadata =
        serde_json::from_slice(&metadata_bytes).context("parsing release stage metadata")?;
    validate_release_stage_metadata(&metadata)?;

    let payload_root = stage_root.join(RELEASE_STAGE_PAYLOAD_ROOT);
    let signed_manifest_bytes = read_real_file(&payload_root.join(RELEASE_MANIFEST_PATH))?;
    let detached_signature_bytes = read_real_file(&stage_root.join(RELEASE_STAGE_SIGNATURE_PATH))?;
    let actual_manifest_sha256 = sha256_bytes_hex(&signed_manifest_bytes);
    if metadata.manifest_sha256 != actual_manifest_sha256 {
        bail!(
            "release stage manifest hash mismatch (metadata {}, actual {actual_manifest_sha256})",
            metadata.manifest_sha256
        );
    }
    let actual_signature_sha256 = sha256_bytes_hex(&detached_signature_bytes);
    if metadata.signature_sha256 != actual_signature_sha256 {
        bail!(
            "release stage signature hash mismatch (metadata {}, actual {actual_signature_sha256})",
            metadata.signature_sha256
        );
    }

    let trusted_keys = trusted_release_key_map(trusted_keys)?;
    let (manifest, signature) = authenticate_release_manifest(
        &signed_manifest_bytes,
        &detached_signature_bytes,
        &trusted_keys,
        expected_key_id,
    )?;
    if metadata.source_git_sha != manifest.source_git_sha {
        bail!(
            "release stage source_git_sha {} does not match signed manifest {}",
            metadata.source_git_sha,
            manifest.source_git_sha
        );
    }

    if manifest.target != expected_target {
        bail!(
            "signed release target {} does not match apply target {expected_target}",
            manifest.target
        );
    }
    verify_release_anti_rollback(&manifest.version, &anti_rollback_floor.version)?;
    let verified = verify_staged_release_tree(&manifest, &signed_manifest_bytes, &payload_root)?;
    if metadata.payload_tree_sha256 != verified.payload_tree_sha256() {
        bail!(
            "release stage payload tree hash does not match the signed manifest \
             (metadata {}, signed {})",
            metadata.payload_tree_sha256,
            verified.payload_tree_sha256()
        );
    }
    if metadata.payload_file_count
        != u64::try_from(verified.file_count()).context("payload file count does not fit u64")?
    {
        bail!("release stage payload file count does not match the signed manifest");
    }

    Ok(AuthenticatedReleaseStage {
        stage_root: stage_root.to_path_buf(),
        metadata,
        manifest,
        signed_manifest_bytes,
        signature,
        verified,
        anti_rollback_floor_path: anti_rollback_floor_path.to_path_buf(),
        anti_rollback_floor,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedIntercomTree {
    pub intercom_dir: PathBuf,
    pub file_count: usize,
    pub tree_sha256: String,
}

pub fn verify_staged_intercom_tree(
    manifest: &IntercomBundleManifest,
    stage_root: &Path,
) -> Result<VerifiedIntercomTree> {
    manifest.validate()?;
    ensure_real_directory(stage_root, "staged release root")?;

    let mut intercom_dir = stage_root.to_path_buf();
    for segment in ["share", "mayhem", "intercom"] {
        intercom_dir.push(segment);
        ensure_real_directory(&intercom_dir, "staged Intercom runtime directory")?;
    }

    let actual = inventory_tree(&intercom_dir, "staged Intercom runtime")?;
    let expected = manifest
        .assets
        .iter()
        .map(|asset| {
            let relative = asset
                .path
                .strip_prefix(INTERCOM_BUNDLE_ASSET_PREFIX)
                .expect("validated Intercom asset prefix");
            (relative, asset)
        })
        .collect::<BTreeMap<_, _>>();
    let expected_directories = directory_ancestors(expected.keys().copied());
    verify_directory_inventory(
        &intercom_dir,
        &expected_directories,
        "staged Intercom runtime",
    )?;

    for relative in expected.keys() {
        if !actual.contains_key(*relative) {
            bail!("listed Intercom runtime file is missing: {relative}");
        }
    }
    for relative in actual.keys() {
        if !expected.contains_key(relative.as_str()) {
            bail!("staged Intercom tree contains unlisted extra file: {relative}");
        }
    }
    for (relative, asset) in &expected {
        let actual_sha256 = actual
            .get(*relative)
            .expect("missing and extra Intercom files checked above");
        if actual_sha256 != &asset.sha256 {
            bail!(
                "Intercom runtime file {relative} SHA-256 mismatch \
                 (expected {}, actual {actual_sha256})",
                asset.sha256
            );
        }
    }

    Ok(VerifiedIntercomTree {
        intercom_dir,
        file_count: actual.len(),
        tree_sha256: intercom_tree_digest(manifest),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReleaseArchiveFormat {
    TarGz,
    Zip,
}

#[derive(Debug)]
struct ReleaseArchiveLayout {
    archive_root: String,
    expected_files: BTreeSet<String>,
    expected_directories: BTreeSet<String>,
}

impl ReleaseArchiveLayout {
    fn new(manifest: &ReleaseBundleManifest) -> Result<Self> {
        let archive_root = format!("mayhem-{}-{}", manifest.version, manifest.target);
        validate_bundle_relative_path(&archive_root, "release archive root")?;

        let mut expected_files = BTreeSet::new();
        for relative in manifest
            .assets
            .iter()
            .map(|asset| asset.path.as_str())
            .chain([RELEASE_MANIFEST_PATH, RELEASE_CHECKSUMS_PATH])
        {
            expected_files.insert(format!("{archive_root}/{relative}"));
        }
        // Build ancestor paths without relying on host path semantics.
        let mut expected_directories = BTreeSet::new();
        expected_directories.insert(archive_root.clone());
        for file in &expected_files {
            let segments = file.split('/').collect::<Vec<_>>();
            for end in 1..segments.len() {
                expected_directories.insert(segments[..end].join("/"));
            }
        }
        Ok(Self {
            archive_root,
            expected_files,
            expected_directories,
        })
    }

    fn payload_relative<'a>(&self, archive_path: &'a str) -> Option<&'a str> {
        archive_path
            .strip_prefix(&self.archive_root)
            .and_then(|suffix| suffix.strip_prefix('/'))
    }

    fn validate_entry(&self, archive_path: &str, is_directory: bool) -> Result<()> {
        if is_directory {
            if !self.expected_directories.contains(archive_path) {
                bail!("release archive contains unlisted extra directory: {archive_path}");
            }
        } else if !self.expected_files.contains(archive_path) {
            bail!("release archive contains unlisted extra file: {archive_path}");
        }
        Ok(())
    }
}

fn release_archive_format(archive_path: &Path, target: &str) -> Result<ReleaseArchiveFormat> {
    let name = archive_path
        .file_name()
        .and_then(|name| name.to_str())
        .context("release archive path must have a UTF-8 file name")?;
    if target.contains("windows") {
        if !name.ends_with(".zip") {
            bail!("Windows release archive must use the .zip format: {name}");
        }
        Ok(ReleaseArchiveFormat::Zip)
    } else {
        if !name.ends_with(".tar.gz") {
            bail!("non-Windows release archive must use the .tar.gz format: {name}");
        }
        Ok(ReleaseArchiveFormat::TarGz)
    }
}

fn extract_release_archive_bounded(
    archive_path: &Path,
    format: ReleaseArchiveFormat,
    manifest: &ReleaseBundleManifest,
    payload_root: &Path,
    limits: &ReleaseExtractionLimits,
) -> Result<()> {
    ensure_real_directory(payload_root, "staged release payload root")?;
    if fs::read_dir(payload_root)
        .with_context(|| format!("reading staged payload root {}", payload_root.display()))?
        .next()
        .is_some()
    {
        bail!(
            "staged release payload root must be empty before extraction: {}",
            payload_root.display()
        );
    }
    let layout = ReleaseArchiveLayout::new(manifest)?;
    match format {
        ReleaseArchiveFormat::TarGz => {
            extract_tar_gz_release(archive_path, payload_root, &layout, limits)
        }
        ReleaseArchiveFormat::Zip => {
            extract_zip_release(archive_path, payload_root, &layout, limits)
        }
    }
}

struct ExpandedArchiveReader<R> {
    inner: R,
    bytes_read: u64,
    limit: u64,
}

impl<R> ExpandedArchiveReader<R> {
    fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            bytes_read: 0,
            limit,
        }
    }
}

impl<R: Read> Read for ExpandedArchiveReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.bytes_read == self.limit {
            let mut probe = [0u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "expanded release archive exceeds max_expanded_archive_bytes",
                )),
            };
        }
        let remaining = self.limit - self.bytes_read;
        let allowed = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("read size is bounded by usize");
        let read = self.inner.read(&mut buffer[..allowed])?;
        self.bytes_read += read as u64;
        Ok(read)
    }
}

fn extract_tar_gz_release(
    archive_path: &Path,
    payload_root: &Path,
    layout: &ReleaseArchiveLayout,
    limits: &ReleaseExtractionLimits,
) -> Result<()> {
    let file = fs::File::open(archive_path)
        .with_context(|| format!("opening release archive {}", archive_path.display()))?;
    let decoder = MultiGzDecoder::new(file);
    let bounded = ExpandedArchiveReader::new(decoder, limits.max_expanded_archive_bytes);
    let mut archive = tar::Archive::new(bounded);
    let entries = archive
        .entries()
        .with_context(|| format!("reading release tar {}", archive_path.display()))?;
    let mut seen = BTreeSet::new();
    let mut seen_portable = BTreeSet::new();
    let mut total_file_bytes = 0u64;
    let mut entry_count = 0usize;

    for entry in entries {
        entry_count = entry_count
            .checked_add(1)
            .context("release archive entry count overflow")?;
        if entry_count > limits.max_entries {
            bail!("release archive exceeds max_entries");
        }
        let mut entry =
            entry.with_context(|| format!("reading release tar {}", archive_path.display()))?;
        let entry_type = entry.header().entry_type();
        let is_directory = entry_type.is_dir();
        if !is_directory && !entry_type.is_file() {
            bail!("release tar contains a link or special file");
        }
        let raw_path = entry.path_bytes();
        let archive_entry =
            normalized_archive_entry_path(raw_path.as_ref(), is_directory, limits.max_path_bytes)?;
        register_archive_entry(&archive_entry, &mut seen, &mut seen_portable)?;
        layout.validate_entry(&archive_entry, is_directory)?;

        if is_directory {
            if let Some(relative) = layout.payload_relative(&archive_entry) {
                create_extraction_directory(payload_root, relative)?;
            }
            continue;
        }
        let relative = layout
            .payload_relative(&archive_entry)
            .context("release tar file must be below its archive root")?;
        let declared_size = entry
            .header()
            .size()
            .context("reading release tar entry size")?;
        reserve_extracted_file_bytes(declared_size, &mut total_file_bytes, limits)?;
        write_extracted_file(
            &mut entry,
            payload_root,
            relative,
            declared_size,
            "release tar entry",
        )?;
    }
    verify_archive_file_inventory(layout, &seen)?;

    let mut trailing = archive.into_inner();
    let mut buffer = [0u8; COPY_BUFFER_SIZE];
    loop {
        let read = trailing
            .read(&mut buffer)
            .context("reading trailing release tar data")?;
        if read == 0 {
            break;
        }
        if buffer[..read].iter().any(|byte| *byte != 0) {
            bail!("release tar contains non-padding data after its end marker");
        }
    }
    Ok(())
}

#[derive(Debug)]
struct ZipEntryPlan {
    archive_path: String,
    payload_relative: Option<String>,
    is_directory: bool,
    flags: u16,
    compression: u16,
    compressed_size: u64,
    uncompressed_size: u64,
    crc32: u32,
    local_header_offset: u64,
    raw_name: Vec<u8>,
}

fn extract_zip_release(
    archive_path: &Path,
    payload_root: &Path,
    layout: &ReleaseArchiveLayout,
    limits: &ReleaseExtractionLimits,
) -> Result<()> {
    let mut file = fs::File::open(archive_path)
        .with_context(|| format!("opening release ZIP {}", archive_path.display()))?;
    let archive_size = file
        .metadata()
        .with_context(|| format!("inspecting release ZIP {}", archive_path.display()))?
        .len();
    let (central_offset, central_size, entry_total) =
        read_zip_end_of_central_directory(&mut file, archive_size)?;
    if entry_total > limits.max_entries {
        bail!("release ZIP exceeds max_entries");
    }
    if central_size > limits.max_archive_metadata_bytes {
        bail!("release ZIP central directory exceeds max_archive_metadata_bytes");
    }
    let central_end = central_offset
        .checked_add(central_size)
        .context("release ZIP central directory offset overflow")?;
    if central_end > archive_size {
        bail!("release ZIP central directory escapes the archive");
    }
    let central_size_usize = usize::try_from(central_size)
        .context("release ZIP central directory is too large for this platform")?;
    let mut central = vec![0u8; central_size_usize];
    file.seek(SeekFrom::Start(central_offset))
        .context("seeking to release ZIP central directory")?;
    file.read_exact(&mut central)
        .context("reading release ZIP central directory")?;

    let mut cursor = 0usize;
    let mut plans = Vec::with_capacity(entry_total);
    let mut seen = BTreeSet::new();
    let mut seen_portable = BTreeSet::new();
    let mut total_file_bytes = 0u64;
    for _ in 0..entry_total {
        let fixed = central
            .get(cursor..cursor.saturating_add(46))
            .context("release ZIP central directory entry is truncated")?;
        if zip_u32(fixed, 0)? != ZIP_CENTRAL_DIRECTORY_SIGNATURE {
            bail!("release ZIP central directory signature is invalid");
        }
        let version_made_by = zip_u16(fixed, 4)?;
        let version_needed = zip_u16(fixed, 6)?;
        let flags = zip_u16(fixed, 8)?;
        let compression = zip_u16(fixed, 10)?;
        let crc32 = zip_u32(fixed, 16)?;
        let compressed_size = zip_u32(fixed, 20)? as u64;
        let uncompressed_size = zip_u32(fixed, 24)? as u64;
        let name_length = zip_u16(fixed, 28)? as usize;
        let extra_length = zip_u16(fixed, 30)? as usize;
        let comment_length = zip_u16(fixed, 32)? as usize;
        let disk_start = zip_u16(fixed, 34)?;
        let external_attributes = zip_u32(fixed, 38)?;
        let local_header_offset = zip_u32(fixed, 42)? as u64;
        if version_needed > 20 {
            bail!("release ZIP requires unsupported ZIP features");
        }
        if disk_start != 0 {
            bail!("release ZIP must not span multiple disks");
        }
        if comment_length != 0 {
            bail!("release ZIP entries must not contain comments");
        }
        validate_zip_flags(flags, compression)?;
        if !matches!(compression, 0 | 8) {
            bail!("release ZIP uses unsupported compression method {compression}");
        }
        let variable_length = name_length
            .checked_add(extra_length)
            .and_then(|length| length.checked_add(comment_length))
            .context("release ZIP central entry length overflow")?;
        let entry_end = cursor
            .checked_add(46)
            .and_then(|position| position.checked_add(variable_length))
            .context("release ZIP central entry offset overflow")?;
        let variable = central
            .get(cursor + 46..entry_end)
            .context("release ZIP central directory entry is truncated")?;
        let raw_name = variable[..name_length].to_vec();
        reject_zip64_extra(&variable[name_length..name_length + extra_length])?;
        let name_marks_directory = raw_name.ends_with(b"/");
        let is_directory =
            zip_entry_is_directory(version_made_by, external_attributes, name_marks_directory)?;
        let archive_entry =
            normalized_archive_entry_path(&raw_name, is_directory, limits.max_path_bytes)?;
        register_archive_entry(&archive_entry, &mut seen, &mut seen_portable)?;
        layout.validate_entry(&archive_entry, is_directory)?;
        if is_directory {
            if compressed_size != 0 || uncompressed_size != 0 {
                bail!("release ZIP directory entry must have zero size: {archive_entry}");
            }
        } else {
            reserve_extracted_file_bytes(uncompressed_size, &mut total_file_bytes, limits)?;
        }
        plans.push(ZipEntryPlan {
            payload_relative: layout.payload_relative(&archive_entry).map(str::to_owned),
            archive_path: archive_entry,
            is_directory,
            flags,
            compression,
            compressed_size,
            uncompressed_size,
            crc32,
            local_header_offset,
            raw_name,
        });
        cursor = entry_end;
    }
    if cursor != central.len() {
        bail!("release ZIP central directory contains trailing records");
    }
    verify_archive_file_inventory(layout, &seen)?;

    let mut occupied_ranges = Vec::with_capacity(plans.len());
    for plan in &plans {
        let data_offset = validate_zip_local_header(&mut file, plan, central_offset)?;
        let data_end = data_offset
            .checked_add(plan.compressed_size)
            .context("release ZIP entry data offset overflow")?;
        if data_end > central_offset {
            bail!(
                "release ZIP entry data overlaps its central directory: {}",
                plan.archive_path
            );
        }
        occupied_ranges.push((
            plan.local_header_offset,
            data_end,
            plan.archive_path.as_str(),
        ));
    }
    occupied_ranges.sort_by_key(|range| range.0);
    for pair in occupied_ranges.windows(2) {
        if pair[0].1 > pair[1].0 {
            bail!(
                "release ZIP entries overlap: {} and {}",
                pair[0].2,
                pair[1].2
            );
        }
    }

    for plan in &plans {
        let Some(relative) = plan.payload_relative.as_deref() else {
            continue;
        };
        if plan.is_directory {
            create_extraction_directory(payload_root, relative)?;
            continue;
        }
        let data_offset = validate_zip_local_header(&mut file, plan, central_offset)?;
        file.seek(SeekFrom::Start(data_offset))
            .with_context(|| format!("seeking to release ZIP entry {}", plan.archive_path))?;
        let mut compressed = (&mut file).take(plan.compressed_size);
        match plan.compression {
            0 => {
                if plan.compressed_size != plan.uncompressed_size {
                    bail!(
                        "stored release ZIP entry has mismatched sizes: {}",
                        plan.archive_path
                    );
                }
                let actual_crc32 = write_extracted_file(
                    &mut compressed,
                    payload_root,
                    relative,
                    plan.uncompressed_size,
                    "stored release ZIP entry",
                )?;
                if actual_crc32 != plan.crc32 {
                    bail!("release ZIP CRC-32 mismatch: {}", plan.archive_path);
                }
            }
            8 => {
                let mut decoder = DeflateDecoder::new(compressed);
                let actual_crc32 = write_extracted_file(
                    &mut decoder,
                    payload_root,
                    relative,
                    plan.uncompressed_size,
                    "deflated release ZIP entry",
                )?;
                if actual_crc32 != plan.crc32 {
                    bail!("release ZIP CRC-32 mismatch: {}", plan.archive_path);
                }
                compressed = decoder.into_inner();
                if compressed.limit() != 0 {
                    bail!(
                        "deflated release ZIP entry did not consume its declared data: {}",
                        plan.archive_path
                    );
                }
            }
            _ => unreachable!("ZIP compression method validated"),
        }
    }
    Ok(())
}

fn read_zip_end_of_central_directory(
    file: &mut fs::File,
    archive_size: u64,
) -> Result<(u64, u64, usize)> {
    if archive_size < 22 {
        bail!("release ZIP is too short");
    }
    let search_size = archive_size.min(22 + ZIP_MAX_COMMENT_SIZE);
    let search_size_usize =
        usize::try_from(search_size).context("release ZIP tail is too large for this platform")?;
    let mut tail = vec![0u8; search_size_usize];
    file.seek(SeekFrom::End(-(search_size as i64)))
        .context("seeking to release ZIP end record")?;
    file.read_exact(&mut tail)
        .context("reading release ZIP end record")?;

    let mut found = None;
    for offset in (0..=tail.len() - 22).rev() {
        if zip_u32(&tail, offset)? != ZIP_END_OF_CENTRAL_DIRECTORY_SIGNATURE {
            continue;
        }
        let comment_length = zip_u16(&tail, offset + 20)? as usize;
        if offset + 22 + comment_length == tail.len() {
            found = Some(offset);
            break;
        }
    }
    let offset = found.context("release ZIP end-of-central-directory record is missing")?;
    let disk = zip_u16(&tail, offset + 4)?;
    let central_disk = zip_u16(&tail, offset + 6)?;
    let entries_on_disk = zip_u16(&tail, offset + 8)?;
    let entries_total = zip_u16(&tail, offset + 10)?;
    let central_size = zip_u32(&tail, offset + 12)?;
    let central_offset = zip_u32(&tail, offset + 16)?;
    if disk != 0 || central_disk != 0 || entries_on_disk != entries_total {
        bail!("release ZIP must not span multiple disks");
    }
    if entries_total == u16::MAX || central_size == u32::MAX || central_offset == u32::MAX {
        bail!("ZIP64 release archives are not supported by the bounded updater");
    }
    let eocd_offset = archive_size - search_size + offset as u64;
    if central_offset as u64 + central_size as u64 != eocd_offset {
        bail!("release ZIP central directory bounds are inconsistent");
    }
    Ok((
        central_offset as u64,
        central_size as u64,
        entries_total as usize,
    ))
}

fn validate_zip_flags(flags: u16, compression: u16) -> Result<()> {
    const UTF8: u16 = 1 << 11;
    const DEFLATE_OPTIONS: u16 = (1 << 1) | (1 << 2);
    let allowed = UTF8 | if compression == 8 { DEFLATE_OPTIONS } else { 0 };
    if flags & !allowed != 0 {
        bail!("release ZIP uses encryption, data descriptors, or unsupported flags");
    }
    Ok(())
}

fn reject_zip64_extra(mut extra: &[u8]) -> Result<()> {
    while !extra.is_empty() {
        if extra.len() < 4 {
            bail!("release ZIP extra field is truncated");
        }
        let kind = u16::from_le_bytes([extra[0], extra[1]]);
        let size = u16::from_le_bytes([extra[2], extra[3]]) as usize;
        extra = extra
            .get(4..)
            .and_then(|remaining| remaining.get(size..))
            .context("release ZIP extra field is truncated")?;
        if kind == 0x0001 {
            bail!("ZIP64 release archives are not supported by the bounded updater");
        }
    }
    Ok(())
}

fn zip_entry_is_directory(
    version_made_by: u16,
    external_attributes: u32,
    name_marks_directory: bool,
) -> Result<bool> {
    let host = version_made_by >> 8;
    let dos_directory = external_attributes & 0x10 != 0;
    if host == 3 {
        let mode = external_attributes >> 16;
        let file_type = mode & 0o170000;
        match file_type {
            0 | 0o100000 => {
                if name_marks_directory || dos_directory {
                    bail!("release ZIP file type conflicts with its path");
                }
                Ok(false)
            }
            0o040000 => {
                if !name_marks_directory {
                    bail!("release ZIP directory path must end with slash");
                }
                Ok(true)
            }
            _ => bail!("release ZIP contains a link or special file"),
        }
    } else {
        if name_marks_directory != dos_directory {
            bail!("release ZIP entry has ambiguous file type");
        }
        Ok(name_marks_directory)
    }
}

fn validate_zip_local_header(
    file: &mut fs::File,
    plan: &ZipEntryPlan,
    central_offset: u64,
) -> Result<u64> {
    let header_end = plan
        .local_header_offset
        .checked_add(30)
        .context("release ZIP local header offset overflow")?;
    if header_end > central_offset {
        bail!("release ZIP local header overlaps its central directory");
    }
    file.seek(SeekFrom::Start(plan.local_header_offset))
        .with_context(|| format!("seeking to release ZIP entry {}", plan.archive_path))?;
    let mut fixed = [0u8; 30];
    file.read_exact(&mut fixed)
        .with_context(|| format!("reading release ZIP entry {}", plan.archive_path))?;
    if zip_u32(&fixed, 0)? != ZIP_LOCAL_FILE_HEADER_SIGNATURE {
        bail!(
            "release ZIP local header signature is invalid: {}",
            plan.archive_path
        );
    }
    if zip_u16(&fixed, 6)? != plan.flags
        || zip_u16(&fixed, 8)? != plan.compression
        || zip_u32(&fixed, 14)? != plan.crc32
        || zip_u32(&fixed, 18)? as u64 != plan.compressed_size
        || zip_u32(&fixed, 22)? as u64 != plan.uncompressed_size
    {
        bail!(
            "release ZIP local and central metadata disagree: {}",
            plan.archive_path
        );
    }
    let name_length = zip_u16(&fixed, 26)? as usize;
    let extra_length = zip_u16(&fixed, 28)? as usize;
    let variable_length = name_length
        .checked_add(extra_length)
        .context("release ZIP local header length overflow")?;
    let mut variable = vec![0u8; variable_length];
    file.read_exact(&mut variable)
        .with_context(|| format!("reading release ZIP entry name {}", plan.archive_path))?;
    if variable[..name_length] != plan.raw_name {
        bail!(
            "release ZIP local and central paths disagree: {}",
            plan.archive_path
        );
    }
    reject_zip64_extra(&variable[name_length..])?;
    header_end
        .checked_add(variable_length as u64)
        .context("release ZIP entry data offset overflow")
}

fn zip_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let value = bytes
        .get(offset..offset.saturating_add(2))
        .context("release ZIP metadata is truncated")?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn zip_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let value = bytes
        .get(offset..offset.saturating_add(4))
        .context("release ZIP metadata is truncated")?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn normalized_archive_entry_path(
    raw_path: &[u8],
    is_directory: bool,
    max_path_bytes: usize,
) -> Result<String> {
    if raw_path.is_empty() || raw_path.len() > max_path_bytes || raw_path.contains(&0) {
        bail!("release archive entry path is empty or exceeds max_path_bytes");
    }
    let raw = std::str::from_utf8(raw_path).context("release archive path must be valid UTF-8")?;
    let path = if is_directory {
        raw.strip_suffix('/').unwrap_or(raw)
    } else {
        if raw.ends_with('/') {
            bail!("release archive regular file path must not end with slash");
        }
        raw
    };
    validate_bundle_relative_path(path, "release archive entry path")?;
    Ok(path.to_owned())
}

fn register_archive_entry(
    archive_path: &str,
    seen: &mut BTreeSet<String>,
    seen_portable: &mut BTreeSet<String>,
) -> Result<()> {
    if !seen.insert(archive_path.to_owned()) {
        bail!("release archive contains duplicate path: {archive_path}");
    }
    let portable = portable_path_key(archive_path);
    if !seen_portable.insert(portable) {
        bail!("release archive contains case-insensitive duplicate path: {archive_path}");
    }
    Ok(())
}

fn reserve_extracted_file_bytes(
    file_size: u64,
    total_file_bytes: &mut u64,
    limits: &ReleaseExtractionLimits,
) -> Result<()> {
    if file_size > limits.max_file_bytes {
        bail!("release archive file exceeds max_file_bytes");
    }
    *total_file_bytes = total_file_bytes
        .checked_add(file_size)
        .context("release archive total file size overflow")?;
    if *total_file_bytes > limits.max_total_file_bytes {
        bail!("release archive exceeds max_total_file_bytes");
    }
    if *total_file_bytes > limits.max_expanded_archive_bytes {
        bail!("release archive exceeds max_expanded_archive_bytes");
    }
    Ok(())
}

fn verify_archive_file_inventory(
    layout: &ReleaseArchiveLayout,
    seen: &BTreeSet<String>,
) -> Result<()> {
    for expected in &layout.expected_files {
        if !seen.contains(expected) {
            bail!("release archive is missing listed file: {expected}");
        }
    }
    Ok(())
}

fn create_extraction_directory(payload_root: &Path, relative: &str) -> Result<()> {
    let mut current = payload_root.to_path_buf();
    for segment in relative.split('/') {
        current.push(segment);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    bail!(
                        "release extraction ancestor must be a real directory: {}",
                        current.display()
                    );
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current)
                    .with_context(|| format!("creating release directory {}", current.display()))?;
                set_release_directory_permissions(&current)?;
                sync_parent(&current)?;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspecting release path {}", current.display()));
            }
        }
    }
    Ok(())
}

fn write_extracted_file(
    reader: &mut impl Read,
    payload_root: &Path,
    relative: &str,
    expected_size: u64,
    label: &str,
) -> Result<u32> {
    let parent = Path::new(relative)
        .parent()
        .context("release archive file must have a parent")?;
    if !parent.as_os_str().is_empty() {
        let parent = parent
            .to_str()
            .context("release archive parent path must be valid UTF-8")?;
        create_extraction_directory(payload_root, parent)?;
    }
    let destination = payload_root.join(relative);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .with_context(|| format!("creating extracted release file {}", destination.display()))?;
    let copy_result = (|| -> Result<u32> {
        let mut remaining = expected_size;
        let mut buffer = [0u8; COPY_BUFFER_SIZE];
        let mut crc32 = u32::MAX;
        while remaining != 0 {
            let allowed = usize::try_from(remaining.min(buffer.len() as u64))
                .expect("copy size is bounded by usize");
            let read = reader
                .read(&mut buffer[..allowed])
                .with_context(|| format!("reading {label} {relative}"))?;
            if read == 0 {
                bail!("{label} ended before its declared size: {relative}");
            }
            output
                .write_all(&buffer[..read])
                .with_context(|| format!("writing extracted release file {relative}"))?;
            crc32 = crc32_update(crc32, &buffer[..read]);
            remaining -= read as u64;
        }
        let mut probe = [0u8; 1];
        if reader
            .read(&mut probe)
            .with_context(|| format!("checking {label} size {relative}"))?
            != 0
        {
            bail!("{label} exceeds its declared size: {relative}");
        }
        set_release_file_permissions(&output, relative)?;
        output
            .sync_all()
            .with_context(|| format!("syncing extracted release file {relative}"))?;
        sync_parent(&destination)?;
        Ok(!crc32)
    })();
    if copy_result.is_err() {
        drop(output);
        let _ = fs::remove_file(&destination);
    }
    copy_result
}

fn crc32_update(mut crc32: u32, bytes: &[u8]) -> u32 {
    for byte in bytes {
        crc32 ^= *byte as u32;
        for _ in 0..8 {
            crc32 = (crc32 >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc32 & 1)));
        }
    }
    crc32
}

#[cfg(unix)]
fn set_release_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("setting release directory permissions {}", path.display()))
}

#[cfg(not(unix))]
fn set_release_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_release_file_permissions(file: &fs::File, relative: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = if relative.starts_with(&format!("{RELEASE_BIN_ROOT}/")) {
        0o755
    } else {
        0o644
    };
    file.set_permissions(fs::Permissions::from_mode(mode))
        .with_context(|| format!("setting extracted release permissions for {relative}"))
}

#[cfg(not(unix))]
fn set_release_file_permissions(_file: &fs::File, _relative: &str) -> Result<()> {
    Ok(())
}

fn validate_release_stage_envelope(stage_root: &Path) -> Result<()> {
    ensure_real_directory(stage_root, "release stage root")?;
    let expected = BTreeMap::from([
        (RELEASE_STAGE_METADATA_PATH, ActivationTargetKind::File),
        (RELEASE_STAGE_PAYLOAD_ROOT, ActivationTargetKind::Directory),
        (RELEASE_STAGE_SIGNATURE_PATH, ActivationTargetKind::File),
    ]);
    let mut seen = BTreeSet::new();
    for entry in fs::read_dir(stage_root)
        .with_context(|| format!("reading release stage {}", stage_root.display()))?
    {
        let entry =
            entry.with_context(|| format!("reading release stage {}", stage_root.display()))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("release stage contains a non-UTF-8 entry"))?;
        let kind = expected
            .get(name.as_str())
            .with_context(|| format!("release stage contains unlisted extra entry: {name}"))?;
        ensure_path_kind(&entry.path(), *kind, "release stage entry")?;
        seen.insert(name);
    }
    for name in expected.keys() {
        if !seen.contains(*name) {
            bail!("release stage is missing required entry: {name}");
        }
    }
    Ok(())
}

fn validate_release_stage_metadata(metadata: &ReleaseStageMetadata) -> Result<()> {
    if metadata.schema != RELEASE_STAGE_SCHEMA {
        bail!("release stage metadata schema must be {RELEASE_STAGE_SCHEMA}");
    }
    if !is_lower_hex_len(&metadata.source_git_sha, 40) {
        bail!(
            "release stage metadata source_git_sha must be exactly 40 lowercase hexadecimal characters"
        );
    }
    for (label, value) in [
        ("manifest_sha256", metadata.manifest_sha256.as_str()),
        ("signature_sha256", metadata.signature_sha256.as_str()),
        ("payload_tree_sha256", metadata.payload_tree_sha256.as_str()),
    ] {
        if !is_lower_sha256(value) {
            bail!("release stage metadata {label} must be lowercase SHA-256 hex");
        }
    }
    if metadata.payload_file_count == 0 || metadata.payload_file_count > MAX_JAVASCRIPT_SAFE_INTEGER
    {
        bail!("release stage metadata payload_file_count must be a positive safe integer");
    }
    if metadata.staged_at_unix > MAX_JAVASCRIPT_SAFE_INTEGER {
        bail!("release stage metadata staged_at_unix must be a safe integer");
    }
    Ok(())
}

fn validate_release_signature_envelope(
    signature: &ReleaseDetachedSignature,
    manifest: &ReleaseBundleManifest,
    signed_manifest_bytes: &[u8],
) -> Result<()> {
    if signature.schema_version != 1 {
        bail!("release manifest signature schema_version must be 1");
    }
    if signature.alg != "ed25519" {
        bail!("release manifest signature alg must be ed25519");
    }
    validate_release_key_id(&signature.key_id)?;
    if !is_lower_hex_len(&signature.public_key, 64) {
        bail!("release manifest signature public_key must be lowercase 32-byte hex");
    }
    if !is_lower_sha256(&signature.sha256) {
        bail!("release manifest signature sha256 must be lowercase SHA-256 hex");
    }
    if !is_lower_hex_len(&signature.sig, 128) {
        bail!("release manifest signature sig must be lowercase 64-byte hex");
    }
    let expected_signed_path = format!(
        "mayhem-{}-{}.manifest.json",
        manifest.version, manifest.target
    );
    if signature.signed_path != expected_signed_path {
        bail!(
            "release manifest signature signed_path {} does not match {expected_signed_path}",
            signature.signed_path
        );
    }
    let actual_sha256 = sha256_bytes_hex(signed_manifest_bytes);
    if signature.sha256 != actual_sha256 {
        bail!(
            "release manifest signature hash mismatch (signature {}, actual {actual_sha256})",
            signature.sha256
        );
    }
    Ok(())
}

fn verify_release_manifest_signature(
    signature: &ReleaseDetachedSignature,
    manifest: &ReleaseBundleManifest,
    signed_manifest_bytes: &[u8],
    trusted_keys: &BTreeMap<String, String>,
    expected_key_id: Option<&str>,
) -> Result<()> {
    validate_release_signature_envelope(signature, manifest, signed_manifest_bytes)?;
    if let Some(expected_key_id) = expected_key_id {
        validate_release_key_id(expected_key_id)?;
        if signature.key_id != expected_key_id {
            bail!(
                "release manifest signed by key id {}, expected {expected_key_id}",
                signature.key_id
            );
        }
    }

    for (key_id, public_key) in trusted_keys {
        validate_release_key_id(key_id)?;
        if !is_lower_hex_len(public_key, 64) {
            bail!(
                "trusted release key {} must be lowercase 32-byte hex",
                key_id
            );
        }
    }
    let trusted_public_key = trusted_keys
        .get(&signature.key_id)
        .with_context(|| format!("release signing key {} is not trusted", signature.key_id))?;
    if trusted_public_key != &signature.public_key {
        bail!("release manifest signature public key does not match the live trusted key");
    }

    let public_key = decode_lower_hex_array::<32>(&signature.public_key, "release public key")?;
    let signature_bytes =
        decode_lower_hex_array::<64>(&signature.sig, "release manifest signature")?;
    let verifying_key =
        VerifyingKey::from_bytes(&public_key).context("release public key is invalid")?;
    let mut signing_bytes =
        Vec::with_capacity(RELEASE_MANIFEST_SIGNATURE_DOMAIN.len() + signed_manifest_bytes.len());
    signing_bytes.extend_from_slice(RELEASE_MANIFEST_SIGNATURE_DOMAIN);
    signing_bytes.extend_from_slice(signed_manifest_bytes);
    verifying_key
        .verify_strict(&signing_bytes, &Signature::from_bytes(&signature_bytes))
        .context("release manifest signature verification failed")
}

fn verify_release_anti_rollback(candidate: &str, minimum: &str) -> Result<()> {
    let candidate = Version::parse(normalized_release_version(candidate)?)
        .context("parsing signed release version for anti-rollback")?;
    let minimum = Version::parse(normalized_release_version(minimum)?)
        .context("parsing protected release version floor")?;
    if candidate < minimum {
        bail!(
            "signed release version {candidate} is below protected anti-rollback floor {minimum}"
        );
    }
    Ok(())
}

pub fn initialize_release_anti_rollback_floor(path: &Path, version: &str) -> Result<()> {
    write_release_anti_rollback_floor(path, version)
}

fn write_release_anti_rollback_floor(path: &Path, version: &str) -> Result<()> {
    validate_absolute_path(path, "release anti-rollback floor")?;
    let parent = path
        .parent()
        .context("release anti-rollback floor must have a parent")?;
    ensure_real_directory(parent, "release anti-rollback floor directory")?;
    let floor = ReleaseAntiRollbackFloor {
        schema: RELEASE_ANTI_ROLLBACK_FLOOR_SCHEMA,
        version: normalized_release_version(version)?.to_owned(),
    };
    let mut bytes =
        serde_json::to_vec_pretty(&floor).context("serializing release anti-rollback floor")?;
    bytes.push(b'\n');
    create_new_synced_file(path, &bytes, "release anti-rollback floor")
}

pub fn read_release_anti_rollback_floor(path: &Path) -> Result<String> {
    Ok(read_release_anti_rollback_floor_record(path)?.version)
}

fn read_release_anti_rollback_floor_record(path: &Path) -> Result<ReleaseAntiRollbackFloor> {
    validate_absolute_path(path, "release anti-rollback floor")?;
    let bytes = read_real_file(path)
        .with_context(|| format!("reading release anti-rollback floor {}", path.display()))?;
    let floor: ReleaseAntiRollbackFloor =
        serde_json::from_slice(&bytes).context("parsing release anti-rollback floor")?;
    if floor.schema != RELEASE_ANTI_ROLLBACK_FLOOR_SCHEMA {
        bail!("release anti-rollback floor schema must be {RELEASE_ANTI_ROLLBACK_FLOOR_SCHEMA}");
    }
    if !is_explicit_semantic_version(&floor.version) {
        bail!("release anti-rollback floor version must be an explicit semantic version");
    }
    Ok(floor)
}

fn validate_release_key_id(key_id: &str) -> Result<()> {
    if key_id.is_empty()
        || key_id.len() > 128
        || !key_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        bail!("release key id is invalid");
    }
    Ok(())
}

fn is_lower_hex_len(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn decode_lower_hex_array<const N: usize>(value: &str, label: &str) -> Result<[u8; N]> {
    if !is_lower_hex_len(value, N * 2) {
        bail!("{label} must be lowercase {}-byte hex", N);
    }
    let mut output = [0u8; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Ok(output)
}

fn hex_nibble(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => bail!("invalid lowercase hexadecimal digit"),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ActivationTarget {
    staged: PathBuf,
    destination: PathBuf,
}

impl ActivationTarget {
    fn new(staged: impl Into<PathBuf>, destination: impl Into<PathBuf>) -> Self {
        Self {
            staged: staged.into(),
            destination: destination.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivationRecovery {
    NoJournal,
    RolledBack,
    CommitCompleted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReleaseActivationRequirement {
    InProcessSafe,
    DetachedHelperRequired { running_executable: PathBuf },
}

/// Determines whether the current process may replace `active_bin_root`.
///
/// A Windows caller receiving `DetachedHelperRequired` must start a verified
/// helper executable outside both activation targets, exit the process running
/// from `active_bin_root`, quiesce sibling services executing any binary from
/// that root, and have the helper wait for every such process to exit before it
/// recovers or starts the transaction. The helper invocation must carry the
/// expected process identities plus the transaction, staged release,
/// destination, and protected-store paths. The helper then invokes
/// `activate_authenticated_release`, runs health through
/// `AuthenticatedReleaseStage::verified_release().primary_binary_in(
/// active_bin_root)`, and commits or rolls back before reporting completion.
pub fn release_activation_requirement(
    active_bin_root: &Path,
) -> Result<ReleaseActivationRequirement> {
    validate_absolute_path(active_bin_root, "active release bin root")?;
    let running_executable =
        std::env::current_exe().context("locating executable performing release activation")?;
    let running_executable = fs::canonicalize(&running_executable).unwrap_or(running_executable);
    let active_bin_root =
        fs::canonicalize(active_bin_root).unwrap_or_else(|_| active_bin_root.to_path_buf());
    Ok(release_activation_requirement_for_platform(
        cfg!(windows),
        &running_executable,
        &active_bin_root,
    ))
}

#[derive(Debug)]
pub struct ActivationTransaction {
    journal_path: PathBuf,
    journal: ActivationJournal,
}

impl ActivationTransaction {
    pub fn journal_path(&self) -> &Path {
        &self.journal_path
    }

    pub fn destinations(&self) -> impl Iterator<Item = &Path> {
        self.journal
            .entries
            .iter()
            .map(|entry| Path::new(entry.destination.as_str()))
    }

    pub fn backup_path(&self, index: usize) -> Option<PathBuf> {
        self.journal
            .entries
            .get(index)
            .map(|entry| activation_scratch_path(&self.journal.id, index, entry, "old"))
    }

    pub fn commit(self) -> Result<()> {
        mark_activation_phase(&self.journal_path, ActivationPhase::Committing)?;
        finish_commit(&self.journal_path, &self.journal)
    }

    pub fn rollback(self) -> Result<()> {
        mark_activation_phase(&self.journal_path, ActivationPhase::RollingBack)?;
        finish_rollback(&self.journal_path, &self.journal)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActivationPhase {
    Preparing,
    Activating,
    HealthGate,
    Committing,
    RollingBack,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ActivationTargetKind {
    File,
    Directory,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ActivationTargetRole {
    BinRoot,
    AssetRoot,
    AntiRollbackFloor,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct ActivationJournalEntry {
    role: ActivationTargetRole,
    destination: String,
    kind: ActivationTargetKind,
    had_active: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct ActivationJournal {
    schema: u32,
    id: String,
    manifest_sha256: String,
    release_version: String,
    previous_floor_version: String,
    entries: Vec<ActivationJournalEntry>,
}

pub fn activation_journal_path(transaction_dir: &Path) -> PathBuf {
    transaction_dir.join(ACTIVATION_JOURNAL_FILE)
}

pub fn activate_authenticated_release(
    transaction_dir: &Path,
    authenticated: AuthenticatedReleaseStage,
    active_bin_root: &Path,
    active_asset_root: &Path,
    protected_store_paths: &[PathBuf],
) -> Result<ActivationTransaction> {
    if let ReleaseActivationRequirement::DetachedHelperRequired { running_executable } =
        release_activation_requirement(active_bin_root)?
    {
        bail!(
            "Windows bin-root activation requires a detached helper outside {} after {} exits",
            active_bin_root.display(),
            running_executable.display()
        );
    }
    activate_release_transaction(
        transaction_dir,
        &authenticated,
        &[
            ActivationTarget::new(authenticated.verified.bin_root(), active_bin_root),
            ActivationTarget::new(authenticated.verified.asset_root(), active_asset_root),
        ],
        protected_store_paths,
    )
}

fn release_activation_requirement_for_platform(
    windows: bool,
    running_executable: &Path,
    active_bin_root: &Path,
) -> ReleaseActivationRequirement {
    if windows && running_executable.starts_with(active_bin_root) {
        ReleaseActivationRequirement::DetachedHelperRequired {
            running_executable: running_executable.to_path_buf(),
        }
    } else {
        ReleaseActivationRequirement::InProcessSafe
    }
}

fn activate_release_transaction(
    transaction_dir: &Path,
    authenticated: &AuthenticatedReleaseStage,
    targets: &[ActivationTarget],
    protected_paths: &[PathBuf],
) -> Result<ActivationTransaction> {
    validate_absolute_path(transaction_dir, "activation transaction directory")?;
    fs::create_dir_all(transaction_dir)
        .with_context(|| format!("creating {}", transaction_dir.display()))?;
    ensure_real_directory(transaction_dir, "activation transaction directory")?;
    if targets.len() != 2 {
        bail!("release activation must contain coordinated bin and asset targets");
    }
    let current_floor =
        read_release_anti_rollback_floor_record(&authenticated.anti_rollback_floor_path)?;
    if current_floor != authenticated.anti_rollback_floor {
        bail!(
            "release anti-rollback floor changed after stage authentication \
             (authenticated {}, current {})",
            authenticated.anti_rollback_floor.version,
            current_floor.version
        );
    }

    let journal_path = activation_journal_path(transaction_dir);
    ensure_path_absent(&journal_path, "activation journal")?;
    let id = new_activation_id();
    let mut entries = validate_activation_targets(
        transaction_dir,
        targets,
        &[
            ActivationTargetRole::BinRoot,
            ActivationTargetRole::AssetRoot,
        ],
        protected_paths,
        Some(id.as_str()),
    )?;
    entries.push(validate_anti_rollback_floor_target(
        transaction_dir,
        &authenticated.anti_rollback_floor_path,
        targets,
        &entries,
        protected_paths,
        &id,
    )?);
    let release_version = normalized_release_version(&authenticated.manifest.version)?.to_owned();
    let journal = ActivationJournal {
        schema: ACTIVATION_JOURNAL_SCHEMA,
        id,
        manifest_sha256: sha256_bytes_hex(&authenticated.signed_manifest_bytes),
        release_version,
        previous_floor_version: authenticated.anti_rollback_floor.version.clone(),
        entries,
    };
    create_activation_journal(&journal_path, &journal)?;

    let prepare_result = (|| -> Result<()> {
        for (index, target) in targets.iter().enumerate() {
            let incoming =
                activation_scratch_path(&journal.id, index, &journal.entries[index], "new");
            copy_path_durable(&target.staged, &incoming, journal.entries[index].kind)?;
        }

        let floor_index = journal.entries.len() - 1;
        let floor_incoming = activation_scratch_path(
            &journal.id,
            floor_index,
            &journal.entries[floor_index],
            "new",
        );
        write_release_anti_rollback_floor(&floor_incoming, &journal.release_version)?;

        verify_authenticated_release_stage_unchanged(authenticated)?;
        verify_destination_local_release_copies(&authenticated.manifest, &journal)?;
        let current_floor =
            read_release_anti_rollback_floor_record(&authenticated.anti_rollback_floor_path)?;
        if current_floor != authenticated.anti_rollback_floor {
            bail!(
                "release anti-rollback floor changed while preparing activation \
                 (authenticated {}, current {})",
                authenticated.anti_rollback_floor.version,
                current_floor.version
            );
        }
        verify_incoming_anti_rollback_floor(&journal)
    })();
    if let Err(error) = prepare_result {
        return rollback_after_activation_error(&journal_path, &journal, error);
    }

    mark_activation_phase(&journal_path, ActivationPhase::Activating)?;
    let activation_result = journal
        .entries
        .iter()
        .enumerate()
        .try_for_each(|(index, entry)| {
            let destination = Path::new(&entry.destination);
            let incoming = activation_scratch_path(&journal.id, index, entry, "new");
            let backup = activation_scratch_path(&journal.id, index, entry, "old");
            if entry.had_active {
                fs::rename(destination, &backup).with_context(|| {
                    format!(
                        "moving active path {} to destination-local backup {}",
                        destination.display(),
                        backup.display()
                    )
                })?;
                sync_parent(destination)?;
            }
            fs::rename(&incoming, destination).with_context(|| {
                format!(
                    "activating destination-local copy {} at {}",
                    incoming.display(),
                    destination.display()
                )
            })?;
            sync_parent(destination)
        });
    if let Err(error) = activation_result {
        return rollback_after_activation_error(&journal_path, &journal, error);
    }

    mark_activation_phase(&journal_path, ActivationPhase::HealthGate)?;
    Ok(ActivationTransaction {
        journal_path,
        journal,
    })
}

fn verify_authenticated_release_stage_unchanged(
    authenticated: &AuthenticatedReleaseStage,
) -> Result<()> {
    validate_release_stage_envelope(&authenticated.stage_root)?;
    let metadata_bytes =
        read_real_file(&authenticated.stage_root.join(RELEASE_STAGE_METADATA_PATH))?;
    let metadata: ReleaseStageMetadata =
        serde_json::from_slice(&metadata_bytes).context("reparsing release stage metadata")?;
    if metadata != authenticated.metadata {
        bail!("release stage metadata changed after authentication");
    }

    let payload_root = authenticated.stage_root.join(RELEASE_STAGE_PAYLOAD_ROOT);
    let signed_manifest_bytes = read_real_file(&payload_root.join(RELEASE_MANIFEST_PATH))?;
    if signed_manifest_bytes != authenticated.signed_manifest_bytes {
        bail!("signed release manifest changed after stage authentication");
    }
    let signature_bytes =
        read_real_file(&authenticated.stage_root.join(RELEASE_STAGE_SIGNATURE_PATH))?;
    if sha256_bytes_hex(&signature_bytes) != authenticated.metadata.signature_sha256 {
        bail!("detached release signature changed after stage authentication");
    }
    let signature: ReleaseDetachedSignature =
        serde_json::from_slice(&signature_bytes).context("reparsing detached release signature")?;
    if signature != authenticated.signature {
        bail!("detached release signature changed after stage authentication");
    }

    let verified = verify_staged_release_tree(
        &authenticated.manifest,
        &authenticated.signed_manifest_bytes,
        &payload_root,
    )?;
    if verified != authenticated.verified
        || verified.payload_tree_sha256() != authenticated.metadata.payload_tree_sha256
        || u64::try_from(verified.file_count()).context("payload file count does not fit u64")?
            != authenticated.metadata.payload_file_count
    {
        bail!("release payload changed after stage authentication");
    }
    Ok(())
}

pub fn recover_release_activation(
    transaction_dir: &Path,
    active_bin_root: &Path,
    active_asset_root: &Path,
    anti_rollback_floor_path: &Path,
    protected_paths: &[PathBuf],
) -> Result<ActivationRecovery> {
    validate_absolute_path(transaction_dir, "activation transaction directory")?;
    ensure_real_directory(transaction_dir, "activation transaction directory")?;
    let journal_path = activation_journal_path(transaction_dir);
    let metadata = match fs::symlink_metadata(&journal_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ActivationRecovery::NoJournal);
        }
        Err(error) => {
            return Err(error).with_context(|| format!("inspecting {}", journal_path.display()));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "activation journal must be a real directory: {}",
            journal_path.display()
        );
    }

    let Some(phase) = read_activation_phase(&journal_path)? else {
        remove_activation_journal(&journal_path)?;
        return Ok(ActivationRecovery::RolledBack);
    };
    let record_path = journal_path.join(ACTIVATION_JOURNAL_RECORD);
    let bytes = read_real_file(&record_path)
        .with_context(|| format!("reading activation journal {}", record_path.display()))?;
    let journal: ActivationJournal =
        serde_json::from_slice(&bytes).context("parsing activation recovery journal")?;
    validate_recovery_journal(
        transaction_dir,
        active_bin_root,
        active_asset_root,
        anti_rollback_floor_path,
        protected_paths,
        &journal,
    )?;

    if phase == ActivationPhase::Committing {
        finish_commit(&journal_path, &journal)?;
        Ok(ActivationRecovery::CommitCompleted)
    } else {
        mark_activation_phase(&journal_path, ActivationPhase::RollingBack)?;
        finish_rollback(&journal_path, &journal)?;
        Ok(ActivationRecovery::RolledBack)
    }
}

fn rollback_after_activation_error(
    journal_path: &Path,
    journal: &ActivationJournal,
    activation_error: anyhow::Error,
) -> Result<ActivationTransaction> {
    let journal_result = mark_activation_phase(journal_path, ActivationPhase::RollingBack);
    let rollback_result = journal_result.and_then(|()| finish_rollback(journal_path, journal));
    match rollback_result {
        Ok(()) => Err(activation_error.context("activation transaction rolled back")),
        Err(rollback_error) => bail!(
            "activation transaction failed: {activation_error:#}; \
             durable rollback also failed: {rollback_error:#}"
        ),
    }
}

fn validate_activation_targets(
    transaction_dir: &Path,
    targets: &[ActivationTarget],
    roles: &[ActivationTargetRole],
    protected_paths: &[PathBuf],
    activation_id: Option<&str>,
) -> Result<Vec<ActivationJournalEntry>> {
    if targets.len() != roles.len() {
        bail!("activation targets must have one journal role each");
    }
    for protected in protected_paths {
        validate_absolute_path(protected, "protected store path")?;
        ensure_optional_real_directory(protected, "protected store path")?;
    }

    let mut entries = Vec::with_capacity(targets.len());
    for (index, target) in targets.iter().enumerate() {
        validate_absolute_path(&target.staged, "staged activation source")?;
        validate_absolute_path(&target.destination, "activation destination")?;
        if !target.destination.starts_with(transaction_dir) {
            bail!(
                "activation destination must be inside destination-local transaction directory {}: {}",
                transaction_dir.display(),
                target.destination.display()
            );
        }
        if paths_overlap(
            &target.destination,
            &activation_journal_path(transaction_dir),
        ) {
            bail!("activation destination must not overlap the activation journal");
        }
        for protected in protected_paths {
            if paths_overlap(&target.staged, protected)
                || paths_overlap(&target.destination, protected)
            {
                bail!(
                    "activation source/destination must remain outside protected store path {}",
                    protected.display()
                );
            }
        }
        for previous in targets.iter().take(index) {
            if paths_overlap(&target.destination, &previous.destination) {
                bail!(
                    "activation destinations must not overlap: {} and {}",
                    previous.destination.display(),
                    target.destination.display()
                );
            }
        }

        let kind = real_path_kind(&target.staged, "staged activation source")?;
        if kind != ActivationTargetKind::Directory {
            bail!("release bin and asset activation sources must be directories");
        }
        let parent = target
            .destination
            .parent()
            .context("activation destination must have a parent")?;
        ensure_descendant_directory(transaction_dir, parent)?;
        let had_active = match fs::symlink_metadata(&target.destination) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !kind_matches_metadata(kind, &metadata) {
                    bail!(
                        "active destination has the wrong type or is a symbolic link: {}",
                        target.destination.display()
                    );
                }
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "inspecting activation destination {}",
                        target.destination.display()
                    )
                });
            }
        };

        let destination = target
            .destination
            .to_str()
            .context("activation destination must be valid UTF-8")?
            .to_owned();
        let entry = ActivationJournalEntry {
            role: roles[index],
            destination,
            kind,
            had_active,
        };
        if let Some(id) = activation_id {
            for suffix in ["new", "old"] {
                let scratch = activation_scratch_path(id, index, &entry, suffix);
                for protected in protected_paths {
                    if paths_overlap(&scratch, protected) {
                        bail!(
                            "activation scratch path must remain outside protected store path {}",
                            protected.display()
                        );
                    }
                }
                ensure_path_absent(&scratch, "destination-local activation scratch path")?;
            }
        }
        entries.push(entry);
    }
    Ok(entries)
}

fn validate_anti_rollback_floor_target(
    transaction_dir: &Path,
    floor_path: &Path,
    release_targets: &[ActivationTarget],
    entries: &[ActivationJournalEntry],
    protected_paths: &[PathBuf],
    activation_id: &str,
) -> Result<ActivationJournalEntry> {
    validate_absolute_path(floor_path, "release anti-rollback floor")?;
    if !floor_path.starts_with(transaction_dir) {
        bail!(
            "release anti-rollback floor must be inside destination-local transaction directory {}: {}",
            transaction_dir.display(),
            floor_path.display()
        );
    }
    if paths_overlap(floor_path, &activation_journal_path(transaction_dir)) {
        bail!("release anti-rollback floor must not overlap the activation journal");
    }
    let parent = floor_path
        .parent()
        .context("release anti-rollback floor must have a parent")?;
    ensure_descendant_directory(transaction_dir, parent)?;
    ensure_real_file(floor_path, "release anti-rollback floor")?;

    for protected in protected_paths {
        if paths_overlap(floor_path, protected) {
            bail!(
                "release anti-rollback floor must remain outside protected store path {}",
                protected.display()
            );
        }
    }
    for target in release_targets {
        if paths_overlap(floor_path, &target.staged)
            || paths_overlap(floor_path, &target.destination)
        {
            bail!("release anti-rollback floor must not overlap release activation paths");
        }
    }
    for entry in entries {
        if paths_overlap(floor_path, Path::new(&entry.destination)) {
            bail!("release anti-rollback floor must not overlap activation destinations");
        }
    }

    let entry = ActivationJournalEntry {
        role: ActivationTargetRole::AntiRollbackFloor,
        destination: floor_path
            .to_str()
            .context("release anti-rollback floor path must be valid UTF-8")?
            .to_owned(),
        kind: ActivationTargetKind::File,
        had_active: true,
    };
    for suffix in ["new", "old"] {
        let scratch = activation_scratch_path(activation_id, entries.len(), &entry, suffix);
        for protected in protected_paths {
            if paths_overlap(&scratch, protected) {
                bail!(
                    "anti-rollback floor scratch path must remain outside protected store path {}",
                    protected.display()
                );
            }
        }
        ensure_path_absent(
            &scratch,
            "destination-local anti-rollback floor scratch path",
        )?;
    }
    Ok(entry)
}

fn validate_recovery_journal(
    transaction_dir: &Path,
    active_bin_root: &Path,
    active_asset_root: &Path,
    anti_rollback_floor_path: &Path,
    protected_paths: &[PathBuf],
    journal: &ActivationJournal,
) -> Result<()> {
    if journal.schema != ACTIVATION_JOURNAL_SCHEMA {
        bail!("activation journal schema must be {ACTIVATION_JOURNAL_SCHEMA}");
    }
    validate_activation_id(&journal.id)?;
    if !is_lower_sha256(&journal.manifest_sha256) {
        bail!("activation journal manifest_sha256 must be lowercase SHA-256 hex");
    }
    if !is_explicit_semantic_version(&journal.release_version)
        || !is_explicit_semantic_version(&journal.previous_floor_version)
    {
        bail!("activation journal release versions must be explicit semantic versions");
    }
    verify_release_anti_rollback(&journal.release_version, &journal.previous_floor_version)?;
    let expected = [
        (
            ActivationTargetRole::BinRoot,
            active_bin_root,
            ActivationTargetKind::Directory,
        ),
        (
            ActivationTargetRole::AssetRoot,
            active_asset_root,
            ActivationTargetKind::Directory,
        ),
        (
            ActivationTargetRole::AntiRollbackFloor,
            anti_rollback_floor_path,
            ActivationTargetKind::File,
        ),
    ];
    if journal.entries.len() != expected.len() {
        bail!("activation journal targets do not match expected destinations");
    }

    for protected in protected_paths {
        validate_absolute_path(protected, "protected store path")?;
        ensure_optional_real_directory(protected, "protected store path")?;
    }
    for (index, ((expected_role, expected_destination, expected_kind), entry)) in
        expected.iter().zip(&journal.entries).enumerate()
    {
        validate_absolute_path(expected_destination, "expected activation destination")?;
        if !expected_destination.starts_with(transaction_dir)
            || expected_destination.to_str() != Some(entry.destination.as_str())
            || entry.role != *expected_role
            || entry.kind != *expected_kind
        {
            bail!("activation journal destination {index} does not match expected destination");
        }
        if entry.role == ActivationTargetRole::AntiRollbackFloor && !entry.had_active {
            bail!("activation journal anti-rollback floor must have an active predecessor");
        }
        ensure_descendant_directory(
            transaction_dir,
            expected_destination
                .parent()
                .context("expected activation destination must have a parent")?,
        )?;
        for protected in protected_paths {
            if paths_overlap(expected_destination, protected) {
                bail!(
                    "activation recovery destination must remain outside protected store path {}",
                    protected.display()
                );
            }
            for suffix in ["new", "old"] {
                if paths_overlap(
                    &activation_scratch_path(&journal.id, index, entry, suffix),
                    protected,
                ) {
                    bail!(
                        "activation recovery scratch path must remain outside protected store path {}",
                        protected.display()
                    );
                }
            }
        }
        for (_, previous, _) in expected.iter().take(index) {
            if paths_overlap(expected_destination, previous) {
                bail!("activation recovery destinations must not overlap");
            }
        }
    }
    Ok(())
}

fn finish_commit(journal_path: &Path, journal: &ActivationJournal) -> Result<()> {
    for entry in &journal.entries {
        let destination = Path::new(&entry.destination);
        ensure_path_kind(destination, entry.kind, "activated destination")?;
    }
    verify_active_anti_rollback_floor(journal, &journal.release_version)?;
    for (index, entry) in journal.entries.iter().enumerate() {
        let destination = Path::new(&entry.destination);
        for suffix in ["old", "new"] {
            let scratch = activation_scratch_path(&journal.id, index, entry, suffix);
            remove_path_if_present(&scratch)?;
        }
        sync_parent(destination)?;
    }
    remove_activation_journal(journal_path)
}

fn finish_rollback(journal_path: &Path, journal: &ActivationJournal) -> Result<()> {
    for (index, entry) in journal.entries.iter().enumerate().rev() {
        let destination = Path::new(&entry.destination);
        let incoming = activation_scratch_path(&journal.id, index, entry, "new");
        let backup = activation_scratch_path(&journal.id, index, entry, "old");
        if entry.had_active {
            if path_exists(&backup)? {
                ensure_path_kind(&backup, entry.kind, "activation backup")?;
                if path_exists(destination)? {
                    ensure_path_kind(destination, entry.kind, "newly activated destination")?;
                    remove_path_if_present(&incoming)?;
                    fs::rename(destination, &incoming).with_context(|| {
                        format!(
                            "preserving newly activated path {} during rollback",
                            destination.display()
                        )
                    })?;
                    sync_parent(destination)?;
                }
                fs::rename(&backup, destination).with_context(|| {
                    format!(
                        "restoring activation backup {} to {}",
                        backup.display(),
                        destination.display()
                    )
                })?;
                sync_parent(destination)?;
            } else {
                ensure_path_kind(destination, entry.kind, "original activation destination")?;
            }
            remove_path_if_present(&incoming)?;
        } else {
            remove_path_if_present(destination)?;
            remove_path_if_present(&incoming)?;
            remove_path_if_present(&backup)?;
            sync_parent(destination)?;
        }
    }
    verify_active_anti_rollback_floor(journal, &journal.previous_floor_version)?;
    remove_activation_journal(journal_path)
}

fn verify_destination_local_release_copies(
    manifest: &ReleaseBundleManifest,
    journal: &ActivationJournal,
) -> Result<()> {
    manifest.validate()?;
    let bin_index = activation_entry_index(journal, ActivationTargetRole::BinRoot)?;
    let asset_index = activation_entry_index(journal, ActivationTargetRole::AssetRoot)?;
    let incoming_bin =
        activation_scratch_path(&journal.id, bin_index, &journal.entries[bin_index], "new");
    let incoming_assets = activation_scratch_path(
        &journal.id,
        asset_index,
        &journal.entries[asset_index],
        "new",
    );
    verify_manifest_subtree(
        manifest,
        &incoming_bin,
        RELEASE_BIN_ROOT,
        "destination-local incoming bin root",
    )?;
    verify_manifest_subtree(
        manifest,
        &incoming_assets,
        MAYHEM_ASSET_ROOT,
        "destination-local incoming Mayhem asset root",
    )
}

fn verify_manifest_subtree(
    manifest: &ReleaseBundleManifest,
    root: &Path,
    manifest_prefix: &str,
    label: &str,
) -> Result<()> {
    let prefix = format!("{manifest_prefix}/");
    let expected = manifest
        .assets
        .iter()
        .filter_map(|asset| {
            asset
                .path
                .strip_prefix(&prefix)
                .map(|relative| (relative, asset.sha256.as_str()))
        })
        .collect::<BTreeMap<_, _>>();
    if expected.is_empty() {
        bail!("signed release manifest contains no files beneath {manifest_prefix}");
    }

    let actual = inventory_tree(root, label)?;
    let expected_directories = directory_ancestors(expected.keys().copied());
    verify_directory_inventory(root, &expected_directories, label)?;
    for (relative, expected_sha256) in &expected {
        let actual_sha256 = actual
            .get(*relative)
            .with_context(|| format!("{label} is missing signed file {relative}"))?;
        if actual_sha256 != expected_sha256 {
            bail!(
                "{label} file {relative} SHA-256 mismatch \
                 (expected {expected_sha256}, actual {actual_sha256})"
            );
        }
    }
    for relative in actual.keys() {
        if !expected.contains_key(relative.as_str()) {
            bail!("{label} contains unlisted extra file {relative}");
        }
    }
    Ok(())
}

fn activation_entry_index(
    journal: &ActivationJournal,
    role: ActivationTargetRole,
) -> Result<usize> {
    let mut matching = journal
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.role == role);
    let (index, _) = matching
        .next()
        .with_context(|| format!("activation journal is missing {role:?} target"))?;
    if matching.next().is_some() {
        bail!("activation journal contains duplicate {role:?} targets");
    }
    Ok(index)
}

fn verify_incoming_anti_rollback_floor(journal: &ActivationJournal) -> Result<()> {
    let index = activation_entry_index(journal, ActivationTargetRole::AntiRollbackFloor)?;
    let incoming = activation_scratch_path(&journal.id, index, &journal.entries[index], "new");
    let floor = read_release_anti_rollback_floor_record(&incoming)?;
    if floor.version != journal.release_version {
        bail!(
            "incoming release anti-rollback floor version {} does not match activated release {}",
            floor.version,
            journal.release_version
        );
    }
    Ok(())
}

fn verify_active_anti_rollback_floor(
    journal: &ActivationJournal,
    expected_version: &str,
) -> Result<()> {
    let index = activation_entry_index(journal, ActivationTargetRole::AntiRollbackFloor)?;
    let floor =
        read_release_anti_rollback_floor_record(Path::new(&journal.entries[index].destination))?;
    if floor.version != expected_version {
        bail!(
            "active release anti-rollback floor version {} does not match transaction version {}",
            floor.version,
            expected_version
        );
    }
    Ok(())
}

fn activation_scratch_path(
    activation_id: &str,
    index: usize,
    entry: &ActivationJournalEntry,
    suffix: &str,
) -> PathBuf {
    let destination = Path::new(&entry.destination);
    destination
        .parent()
        .expect("validated activation destination parent")
        .join(format!(".mayhem-activate-{activation_id}-{index}-{suffix}"))
}

fn create_activation_journal(path: &Path, journal: &ActivationJournal) -> Result<()> {
    let parent = path
        .parent()
        .context("activation journal must have a parent")?;
    ensure_real_directory(parent, "activation journal directory")?;
    fs::create_dir(path)
        .with_context(|| format!("creating activation journal directory {}", path.display()))?;
    sync_directory(parent)?;

    let mut bytes = serde_json::to_vec(journal).context("serializing activation journal")?;
    bytes.push(b'\n');
    let create_result = create_new_synced_file(
        &path.join(ACTIVATION_JOURNAL_RECORD),
        &bytes,
        "activation journal record",
    )
    .and_then(|()| mark_activation_phase(path, ActivationPhase::Preparing));
    if create_result.is_err() {
        let _ = fs::remove_dir_all(path);
        let _ = sync_directory(parent);
    }
    create_result
}

fn mark_activation_phase(path: &Path, phase: ActivationPhase) -> Result<()> {
    ensure_real_directory(path, "activation journal")?;
    let current = read_activation_phase(path)?;
    let transition_allowed = matches!(
        (current, phase),
        (None, ActivationPhase::Preparing)
            | (Some(ActivationPhase::Preparing), ActivationPhase::Preparing)
            | (
                Some(ActivationPhase::Preparing),
                ActivationPhase::Activating
            )
            | (
                Some(ActivationPhase::Activating),
                ActivationPhase::Activating
            )
            | (
                Some(ActivationPhase::Activating),
                ActivationPhase::HealthGate
            )
            | (
                Some(ActivationPhase::HealthGate),
                ActivationPhase::HealthGate
            )
            | (
                Some(ActivationPhase::HealthGate),
                ActivationPhase::Committing
            )
            | (
                Some(ActivationPhase::Committing),
                ActivationPhase::Committing
            )
            | (
                Some(
                    ActivationPhase::Preparing
                        | ActivationPhase::Activating
                        | ActivationPhase::HealthGate
                        | ActivationPhase::RollingBack,
                ),
                ActivationPhase::RollingBack,
            )
    );
    if !transition_allowed {
        bail!(
            "invalid activation journal phase transition from {:?} to {phase:?}",
            current
        );
    }

    let marker = path.join(activation_phase_marker(phase));
    match create_new_synced_file(&marker, b"", "activation phase marker") {
        Ok(()) => Ok(()),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists) =>
        {
            ensure_real_file(&marker, "activation phase marker")
        }
        Err(error) => Err(error),
    }
}

fn read_activation_phase(path: &Path) -> Result<Option<ActivationPhase>> {
    ensure_real_directory(path, "activation journal")?;
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(path).with_context(|| format!("reading {}", path.display()))? {
        let entry = entry.with_context(|| format!("reading {}", path.display()))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("activation journal contains a non-UTF-8 entry"))?;
        let metadata = entry
            .file_type()
            .with_context(|| format!("inspecting {}", entry.path().display()))?;
        if metadata.is_symlink() || !metadata.is_file() {
            bail!(
                "activation journal entry must be a real file: {}",
                entry.path().display()
            );
        }
        if !matches!(
            name.as_str(),
            ACTIVATION_JOURNAL_RECORD
                | ACTIVATION_PHASE_READY
                | ACTIVATION_PHASE_ACTIVATING
                | ACTIVATION_PHASE_HEALTH_GATE
                | ACTIVATION_PHASE_COMMITTING
                | ACTIVATION_PHASE_ROLLING_BACK
        ) {
            bail!("activation journal contains unknown entry: {name}");
        }
        names.insert(name);
    }

    let ready = names.contains(ACTIVATION_PHASE_READY);
    let activating = names.contains(ACTIVATION_PHASE_ACTIVATING);
    let health_gate = names.contains(ACTIVATION_PHASE_HEALTH_GATE);
    let committing = names.contains(ACTIVATION_PHASE_COMMITTING);
    let rolling_back = names.contains(ACTIVATION_PHASE_ROLLING_BACK);
    if !ready {
        if activating || health_gate || committing || rolling_back {
            bail!("activation journal phase markers are missing the ready record");
        }
        return Ok(None);
    }
    if !names.contains(ACTIVATION_JOURNAL_RECORD) {
        bail!("activation journal is missing its immutable record");
    }
    if activating && !ready
        || health_gate && !activating
        || committing && !health_gate
        || committing && rolling_back
    {
        bail!("activation journal phase markers are inconsistent");
    }
    if committing {
        Ok(Some(ActivationPhase::Committing))
    } else if rolling_back {
        Ok(Some(ActivationPhase::RollingBack))
    } else if health_gate {
        Ok(Some(ActivationPhase::HealthGate))
    } else if activating {
        Ok(Some(ActivationPhase::Activating))
    } else {
        Ok(Some(ActivationPhase::Preparing))
    }
}

fn activation_phase_marker(phase: ActivationPhase) -> &'static str {
    match phase {
        ActivationPhase::Preparing => ACTIVATION_PHASE_READY,
        ActivationPhase::Activating => ACTIVATION_PHASE_ACTIVATING,
        ActivationPhase::HealthGate => ACTIVATION_PHASE_HEALTH_GATE,
        ActivationPhase::Committing => ACTIVATION_PHASE_COMMITTING,
        ActivationPhase::RollingBack => ACTIVATION_PHASE_ROLLING_BACK,
    }
}

fn create_new_synced_file(path: &Path, bytes: &[u8], label: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("creating {label} {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("writing {label} {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("syncing {label} {}", path.display()))?;
    sync_parent(path)
}

fn remove_activation_journal(path: &Path) -> Result<()> {
    ensure_real_directory(path, "activation journal")?;
    fs::remove_dir_all(path)
        .with_context(|| format!("removing completed activation journal {}", path.display()))?;
    sync_parent(path)
}

fn copy_path_durable(source: &Path, destination: &Path, kind: ActivationTargetKind) -> Result<()> {
    ensure_path_absent(destination, "destination-local activation copy")?;
    match kind {
        ActivationTargetKind::File => copy_file_durable(source, destination),
        ActivationTargetKind::Directory => copy_directory_durable(source, destination),
    }
}

fn copy_file_durable(source: &Path, destination: &Path) -> Result<()> {
    ensure_real_file(source, "activation source file")?;
    let mut input =
        fs::File::open(source).with_context(|| format!("opening {}", source.display()))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .with_context(|| format!("creating {}", destination.display()))?;
    let mut buffer = [0u8; COPY_BUFFER_SIZE];
    loop {
        let read = input
            .read(&mut buffer)
            .with_context(|| format!("reading {}", source.display()))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .with_context(|| format!("writing {}", destination.display()))?;
    }
    output
        .set_permissions(
            fs::metadata(source)
                .with_context(|| format!("reading permissions for {}", source.display()))?
                .permissions(),
        )
        .with_context(|| format!("setting permissions on {}", destination.display()))?;
    output
        .sync_all()
        .with_context(|| format!("syncing {}", destination.display()))?;
    sync_parent(destination)
}

fn copy_directory_durable(source: &Path, destination: &Path) -> Result<()> {
    ensure_real_directory(source, "activation source directory")?;
    fs::create_dir(destination).with_context(|| format!("creating {}", destination.display()))?;
    let copy_result = (|| -> Result<()> {
        let mut entries = fs::read_dir(source)
            .with_context(|| format!("reading {}", source.display()))?
            .map(|entry| entry.with_context(|| format!("reading {}", source.display())))
            .collect::<Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let source_path = entry.path();
            let destination_path = destination.join(entry.file_name());
            let metadata = fs::symlink_metadata(&source_path)
                .with_context(|| format!("inspecting {}", source_path.display()))?;
            if metadata.file_type().is_symlink() {
                bail!(
                    "activation source tree must not contain symbolic links: {}",
                    source_path.display()
                );
            }
            if metadata.is_dir() {
                copy_directory_durable(&source_path, &destination_path)?;
            } else if metadata.is_file() {
                copy_file_durable(&source_path, &destination_path)?;
            } else {
                bail!(
                    "activation source tree path must be a regular file or directory: {}",
                    source_path.display()
                );
            }
        }
        fs::set_permissions(
            destination,
            fs::metadata(source)
                .with_context(|| format!("reading permissions for {}", source.display()))?
                .permissions(),
        )
        .with_context(|| format!("setting permissions on {}", destination.display()))?;
        sync_directory(destination)?;
        sync_parent(destination)
    })();
    if copy_result.is_err() {
        let _ = fs::remove_dir_all(destination);
    }
    copy_result
}

fn inventory_tree(root: &Path, label: &str) -> Result<BTreeMap<String, String>> {
    fn visit(
        directory: &Path,
        parent_segments: &[String],
        files: &mut BTreeMap<String, String>,
        label: &str,
    ) -> Result<()> {
        let entries = fs::read_dir(directory)
            .with_context(|| format!("reading {label} directory {}", directory.display()))?
            .map(|entry| {
                let entry =
                    entry.with_context(|| format!("reading directory {}", directory.display()))?;
                let name = entry.file_name().into_string().map_err(|_| {
                    anyhow::anyhow!(
                        "{label} directory contains a non-UTF-8 path: {}",
                        directory.display()
                    )
                })?;
                Ok((name, entry.path()))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut entries = entries;
        entries.sort_by(|left, right| left.0.cmp(&right.0));

        for (name, path) in entries {
            let mut segments = parent_segments.to_vec();
            segments.push(name);
            let relative = segments.join("/");
            validate_bundle_relative_path(&relative, &format!("{label} tree path"))?;
            let metadata = fs::symlink_metadata(&path)
                .with_context(|| format!("inspecting {label} path {}", path.display()))?;
            if metadata.file_type().is_symlink() {
                bail!("{label} tree path {relative} must not be a symbolic link");
            }
            if metadata.is_dir() {
                visit(&path, &segments, files, label)?;
            } else if metadata.is_file() {
                let digest = file_sha256_hex(&path)?;
                if files.insert(relative.clone(), digest).is_some() {
                    bail!("{label} tree contains duplicate path: {relative}");
                }
            } else {
                bail!("{label} tree path {relative} must be a regular file or directory");
            }
        }
        Ok(())
    }

    let mut files = BTreeMap::new();
    visit(root, &[], &mut files, label)?;
    Ok(files)
}

fn directory_ancestors<'a>(files: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    let mut directories = BTreeSet::new();
    for file in files {
        let segments = file.split('/').collect::<Vec<_>>();
        for end in 1..segments.len() {
            directories.insert(segments[..end].join("/"));
        }
    }
    directories
}

fn verify_directory_inventory(root: &Path, expected: &BTreeSet<String>, label: &str) -> Result<()> {
    fn visit(
        directory: &Path,
        parent_segments: &[String],
        actual: &mut BTreeSet<String>,
        label: &str,
    ) -> Result<()> {
        for entry in fs::read_dir(directory)
            .with_context(|| format!("reading {label} directory {}", directory.display()))?
        {
            let entry = entry.with_context(|| format!("reading {}", directory.display()))?;
            let metadata = entry
                .file_type()
                .with_context(|| format!("inspecting {}", entry.path().display()))?;
            if metadata.is_symlink() {
                bail!(
                    "{label} directory path must not be a symbolic link: {}",
                    entry.path().display()
                );
            }
            if !metadata.is_dir() {
                continue;
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("{label} directory contains a non-UTF-8 path"))?;
            let mut segments = parent_segments.to_vec();
            segments.push(name);
            let relative = segments.join("/");
            validate_bundle_relative_path(&relative, &format!("{label} directory path"))?;
            actual.insert(relative);
            visit(&entry.path(), &segments, actual, label)?;
        }
        Ok(())
    }

    let mut actual = BTreeSet::new();
    visit(root, &[], &mut actual, label)?;
    for directory in &actual {
        if !expected.contains(directory) {
            bail!("{label} contains unlisted extra directory: {directory}");
        }
    }
    for directory in expected {
        if !actual.contains(directory) {
            bail!("{label} is missing listed directory: {directory}");
        }
    }
    Ok(())
}

fn verify_checksum_metadata(stage_root: &Path, actual: &BTreeMap<String, String>) -> Result<()> {
    let checksum_path = stage_root.join(RELEASE_CHECKSUMS_PATH);
    let bytes = read_real_file(&checksum_path)?;
    let text = std::str::from_utf8(&bytes).context("SHA256SUMS must be valid UTF-8")?;
    if text.is_empty() || !text.ends_with('\n') || text.contains('\r') {
        bail!("SHA256SUMS must be non-empty LF-terminated text");
    }

    let mut checksums = BTreeMap::new();
    let mut previous: Option<&str> = None;
    for (index, line) in text[..text.len() - 1].split('\n').enumerate() {
        let (sha256, relative) = line
            .split_once("  ")
            .with_context(|| format!("SHA256SUMS line {} is malformed", index + 1))?;
        if sha256.len() != 64 || line.as_bytes().get(64..66) != Some(b"  ") {
            bail!("SHA256SUMS line {} is malformed", index + 1);
        }
        if !is_lower_sha256(sha256) {
            bail!(
                "SHA256SUMS line {} hash must be lowercase SHA-256 hex",
                index + 1
            );
        }
        validate_bundle_relative_path(relative, "SHA256SUMS path")?;
        if relative == RELEASE_CHECKSUMS_PATH {
            bail!("SHA256SUMS must not list itself");
        }
        if previous.is_some_and(|previous| previous >= relative) {
            bail!("SHA256SUMS paths must be unique and sorted in ascending byte order");
        }
        checksums.insert(relative, sha256);
        previous = Some(relative);
    }

    for (relative, actual_sha256) in actual {
        if relative == RELEASE_CHECKSUMS_PATH {
            continue;
        }
        let expected = checksums
            .get(relative.as_str())
            .with_context(|| format!("SHA256SUMS is missing staged file: {relative}"))?;
        if *expected != actual_sha256 {
            bail!(
                "SHA256SUMS hash mismatch for {relative} \
                 (expected {expected}, actual {actual_sha256})"
            );
        }
    }
    for relative in checksums.keys() {
        if !actual.contains_key(*relative) {
            bail!("SHA256SUMS contains unlisted path: {relative}");
        }
    }
    Ok(())
}

fn validate_bundle_relative_path(relative: &str, label: &str) -> Result<()> {
    if relative.is_empty()
        || !relative.is_ascii()
        || relative.starts_with('/')
        || relative.contains('\\')
        || relative
            .bytes()
            .any(|byte| !(0x20..=0x7e).contains(&byte) || b"<>:\"|?*".contains(&byte))
    {
        bail!("{label} is unsafe: {relative:?}");
    }
    if relative.split('/').any(|segment| {
        let reserved_base = segment.split('.').next().unwrap_or(segment);
        segment.is_empty()
            || matches!(segment, "." | "..")
            || segment.ends_with('.')
            || segment.ends_with(' ')
            || is_windows_reserved_file_name(reserved_base)
    }) {
        bail!("{label} is unsafe: {relative:?}");
    }
    Ok(())
}

fn is_windows_reserved_file_name(value: &str) -> bool {
    let value = value.to_ascii_uppercase();
    matches!(value.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || value
            .strip_prefix("COM")
            .or_else(|| value.strip_prefix("LPT"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
}

fn portable_path_key(value: &str) -> String {
    value.to_ascii_lowercase()
}

fn validate_portable_file_name(value: &str, label: &str) -> Result<()> {
    validate_bundle_relative_path(value, label)?;
    if value.contains('/') {
        bail!("{label} must be a file name without directory segments");
    }
    Ok(())
}

fn normalized_release_version(value: &str) -> Result<&str> {
    let normalized = value.strip_prefix('v').unwrap_or(value);
    if !is_explicit_semantic_version(normalized) {
        bail!("release version must be an explicit semantic version with an optional v prefix");
    }
    Ok(normalized)
}

fn is_explicit_semantic_version(value: &str) -> bool {
    Version::parse(value).is_ok_and(|version| {
        version.pre.is_empty() && version.build.is_empty() && version.to_string() == value
    })
}

fn primary_binary_name(target: &str) -> &'static str {
    if target.contains("windows") {
        "mayhem.exe"
    } else {
        "mayhem"
    }
}

fn required_release_binary_names(target: &str) -> Vec<String> {
    let extension = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    REQUIRED_RELEASE_BINARY_BASE_NAMES
        .iter()
        .map(|name| format!("{name}{extension}"))
        .collect()
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_bytes_hex(bytes: &[u8]) -> String {
    hex_lower(&Sha256::digest(bytes))
}

fn file_sha256_hex(path: &Path) -> Result<String> {
    let mut file =
        fs::File::open(path).with_context(|| format!("opening release file {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; COPY_BUFFER_SIZE];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("reading release file {}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex_lower(&digest.finalize()))
}

fn read_real_file(path: &Path) -> Result<Vec<u8>> {
    ensure_real_file(path, "release metadata file")?;
    fs::read(path).with_context(|| format!("reading {}", path.display()))
}

fn release_tree_digest(assets: &[ReleaseBundleAsset]) -> String {
    let mut digest = Sha256::new();
    digest.update(RELEASE_TREE_DIGEST_DOMAIN);
    for asset in assets {
        digest.update(asset.path.as_bytes());
        digest.update(b"\0");
        digest.update(asset.sha256.as_bytes());
        digest.update(b"\0");
    }
    hex_lower(&digest.finalize())
}

fn intercom_tree_digest(manifest: &IntercomBundleManifest) -> String {
    let mut digest = Sha256::new();
    digest.update(INTERCOM_TREE_DIGEST_DOMAIN);
    for asset in &manifest.assets {
        digest.update(asset.path.as_bytes());
        digest.update(b"\0");
        digest.update(asset.sha256.as_bytes());
        digest.update(b"\0");
    }
    hex_lower(&digest.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn validate_absolute_path(path: &Path, label: &str) -> Result<()> {
    if !path.is_absolute() {
        bail!("{label} must be absolute: {}", path.display());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        bail!("{label} must not contain dot segments: {}", path.display());
    }
    Ok(())
}

fn ensure_real_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspecting {label} {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("{label} must be a real directory: {}", path.display());
    }
    Ok(())
}

fn ensure_real_file(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspecting {label} {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("{label} must be a real file: {}", path.display());
    }
    Ok(())
}

fn ensure_optional_real_directory(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => Ok(()),
        Ok(_) => bail!("{label} must be a real directory: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspecting {label} {}", path.display())),
    }
}

fn ensure_descendant_directory(root: &Path, directory: &Path) -> Result<()> {
    let relative = directory.strip_prefix(root).with_context(|| {
        format!(
            "activation destination parent must be inside transaction directory {}: {}",
            root.display(),
            directory.display()
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            bail!(
                "activation destination parent contains an unsafe component: {}",
                directory.display()
            );
        };
        current.push(segment);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    bail!(
                        "activation destination ancestor must be a real directory: {}",
                        current.display()
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)
                    .with_context(|| format!("creating {}", current.display()))?;
                sync_parent(&current)?;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", current.display()));
            }
        }
    }
    Ok(())
}

fn ensure_path_absent(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => bail!("{label} must not already exist: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspecting {label} {}", path.display())),
    }
}

fn real_path_kind(path: &Path, label: &str) -> Result<ActivationTargetKind> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspecting {label} {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!("{label} must not be a symbolic link: {}", path.display());
    }
    if metadata.is_file() {
        Ok(ActivationTargetKind::File)
    } else if metadata.is_dir() {
        Ok(ActivationTargetKind::Directory)
    } else {
        bail!(
            "{label} must be a regular file or directory: {}",
            path.display()
        )
    }
}

fn ensure_path_kind(path: &Path, kind: ActivationTargetKind, label: &str) -> Result<()> {
    let actual = real_path_kind(path, label)?;
    if actual != kind {
        bail!("{label} has unexpected type: {}", path.display());
    }
    Ok(())
}

fn kind_matches_metadata(kind: ActivationTargetKind, metadata: &fs::Metadata) -> bool {
    match kind {
        ActivationTargetKind::File => metadata.is_file(),
        ActivationTargetKind::Directory => metadata.is_dir(),
    }
}

fn path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                bail!(
                    "activation transaction path must not be a symbolic link: {}",
                    path.display()
                );
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("inspecting {}", path.display())),
    }
}

fn remove_path_if_present(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("inspecting {}", path.display()));
        }
    };
    if metadata.file_type().is_symlink() {
        bail!(
            "refusing to remove symbolic link from activation transaction: {}",
            path.display()
        );
    }
    if metadata.is_dir() {
        fs::remove_dir_all(path).with_context(|| format!("removing {}", path.display()))
    } else if metadata.is_file() {
        fs::remove_file(path).with_context(|| format!("removing {}", path.display()))
    } else {
        bail!(
            "activation transaction path is not a regular file or directory: {}",
            path.display()
        )
    }
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn new_activation_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!(
        "{}-{nanos}-{}",
        std::process::id(),
        NEXT_ACTIVATION_ID.fetch_add(1, Ordering::Relaxed)
    )
}

fn validate_activation_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id.bytes().all(|byte| byte.is_ascii_digit() || byte == b'-')
    {
        bail!("activation journal id is invalid");
    }
    Ok(())
}

fn sync_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("path must have a parent: {}", path.display()))?;
    sync_directory(parent)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    fs::File::open(path)
        .with_context(|| format!("opening directory for fsync {}", path.display()))?
        .sync_all()
        .with_context(|| format!("fsyncing directory {}", path.display()))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use flate2::write::{DeflateEncoder, GzEncoder};
    use flate2::Compression;

    const TEST_SOURCE_GIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";
    const MUTATED_SOURCE_GIT_SHA: &str = "89abcdef0123456789abcdef0123456789abcdef";
    const TEST_RELEASE_BINARIES: &[(&str, &[u8])] = &[
        ("mayhem", b"new-mayhem"),
        ("mayhem-gateway", b"new-mayhem-gateway"),
        ("mayhem-pay", b"new-mayhem-pay"),
        ("mayhemd", b"new-mayhemd"),
        ("mayhem-enclave", b"new-mayhem-enclave"),
        ("mayhem-paygate", b"new-mayhem-paygate"),
        (
            "mayhem-attestation-verifier",
            b"new-mayhem-attestation-verifier",
        ),
    ];

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "mayhem-release-bundle-{label}-{}",
                new_activation_id()
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_file(root: &Path, relative: &str, contents: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("test file parent"))
            .expect("create test file parent");
        fs::write(path, contents).expect("write test file");
    }

    fn sha256(contents: &[u8]) -> String {
        hex_lower(&Sha256::digest(contents))
    }

    fn intercom_manifest_for_version(
        files: &[(&str, &[u8])],
        release_version: &str,
    ) -> IntercomBundleManifest {
        let mut assets = files
            .iter()
            .map(|(relative, contents)| IntercomBundleAsset {
                path: format!("{INTERCOM_BUNDLE_ASSET_PREFIX}{relative}"),
                sha256: sha256(contents),
            })
            .collect::<Vec<_>>();
        assets.sort_by(|left, right| left.path.cmp(&right.path));
        IntercomBundleManifest {
            schema: 1,
            release_version: release_version.to_owned(),
            contract_version: 13,
            contract_code_sha256: "a".repeat(64),
            assets,
        }
    }

    fn intercom_manifest_for(files: &[(&str, &[u8])]) -> IntercomBundleManifest {
        intercom_manifest_for_version(files, "0.2.24")
    }

    fn stage_release_for(version: &str, target: &str) -> (TestDir, ReleaseBundleManifest, Vec<u8>) {
        let temp = TestDir::new("release");
        let non_binary_payload: &[(&str, &[u8])] = &[
            ("README.md", b"readme"),
            ("share/mayhem/RULES.md", b"rules"),
            ("share/mayhem/intercom/contract/release.json", b"release"),
            ("share/mayhem/intercom/src/main.js", b"main"),
        ];
        let mut assets = Vec::new();
        for (relative, contents) in non_binary_payload {
            write_file(temp.path(), relative, contents);
            assets.push(ReleaseBundleAsset {
                path: (*relative).to_owned(),
                sha256: sha256(contents),
            });
        }
        let binaries = TEST_RELEASE_BINARIES
            .iter()
            .map(|(base_name, contents)| {
                let name = if target.contains("windows") {
                    format!("{base_name}.exe")
                } else {
                    (*base_name).to_owned()
                };
                let binary_path = format!("{RELEASE_BIN_ROOT}/{name}");
                write_file(temp.path(), &binary_path, contents);
                assets.push(ReleaseBundleAsset {
                    path: binary_path.clone(),
                    sha256: sha256(contents),
                });
                ReleaseBundleBinary {
                    name,
                    path: binary_path,
                    sha256: sha256(contents),
                }
            })
            .collect::<Vec<_>>();
        assets.sort_by(|left, right| left.path.cmp(&right.path));
        let intercom = intercom_manifest_for_version(
            &[
                ("contract/release.json", b"release"),
                ("src/main.js", b"main"),
            ],
            version.strip_prefix('v').unwrap_or(version),
        );
        let manifest = ReleaseBundleManifest {
            schema: 1,
            name: "mayhem".to_owned(),
            version: version.to_owned(),
            target: target.to_owned(),
            built_at_utc: "2026-07-19T12:00:00Z".to_owned(),
            source_git_sha: TEST_SOURCE_GIT_SHA.to_owned(),
            binaries,
            assets,
            intercom,
        };
        let mut manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("serialize manifest");
        manifest_bytes.push(b'\n');
        fs::write(temp.path().join(RELEASE_MANIFEST_PATH), &manifest_bytes)
            .expect("write manifest");
        write_test_checksums(temp.path());
        (temp, manifest, manifest_bytes)
    }

    fn stage_release() -> (TestDir, ReleaseBundleManifest, Vec<u8>) {
        stage_release_for("v0.2.24", "aarch64-apple-darwin")
    }

    fn write_test_checksums(root: &Path) {
        let actual = inventory_tree(root, "test release").expect("inventory test release");
        let mut output = String::new();
        for (relative, digest) in actual {
            if relative != RELEASE_CHECKSUMS_PATH {
                writeln!(&mut output, "{digest}  {relative}").expect("write checksum");
            }
        }
        fs::write(root.join(RELEASE_CHECKSUMS_PATH), output).expect("write checksums");
    }

    fn sign_test_release(
        manifest: &ReleaseBundleManifest,
        manifest_bytes: &[u8],
        seed_byte: u8,
    ) -> (Vec<u8>, TrustedReleaseKey) {
        let signing_key = SigningKey::from_bytes(&[seed_byte; 32]);
        let public_key = hex_lower(&signing_key.verifying_key().to_bytes());
        let mut signing_bytes =
            Vec::with_capacity(RELEASE_MANIFEST_SIGNATURE_DOMAIN.len() + manifest_bytes.len());
        signing_bytes.extend_from_slice(RELEASE_MANIFEST_SIGNATURE_DOMAIN);
        signing_bytes.extend_from_slice(manifest_bytes);
        let signature = signing_key.sign(&signing_bytes);
        let key_id = format!("test-release-{seed_byte}");
        let detached = ReleaseDetachedSignature {
            schema_version: 1,
            alg: "ed25519".to_owned(),
            signed_path: format!(
                "mayhem-{}-{}.manifest.json",
                manifest.version, manifest.target
            ),
            key_id: key_id.clone(),
            public_key: public_key.clone(),
            sha256: sha256_bytes_hex(manifest_bytes),
            sig: hex_lower(&signature.to_bytes()),
        };
        (
            serde_json::to_vec_pretty(&detached).expect("serialize test signature"),
            TrustedReleaseKey::new(key_id, public_key),
        )
    }

    fn trusted_key_map(trusted_key: &TrustedReleaseKey) -> BTreeMap<String, String> {
        BTreeMap::from([(trusted_key.key_id.clone(), trusted_key.public_key.clone())])
    }

    fn prepare_signed_test_stage(
        release_root: &Path,
        manifest: &ReleaseBundleManifest,
        manifest_bytes: &[u8],
        work_root: &Path,
        seed_byte: u8,
    ) -> (PathBuf, TrustedReleaseKey) {
        let archive_path = work_root.join(format!("release-{seed_byte}.tar.gz"));
        write_release_tar(release_root, manifest, &archive_path);
        let (signature_bytes, trusted_key) = sign_test_release(manifest, manifest_bytes, seed_byte);
        let stage_root = work_root.join(format!("staged-{seed_byte}"));
        stage_release_archive(
            &archive_path,
            manifest_bytes,
            &signature_bytes,
            &stage_root,
            1_800_000_000,
            &ReleaseExtractionLimits::default(),
        )
        .expect("stage signed test release");
        (stage_root, trusted_key)
    }

    fn authenticate_test_stage(
        stage_root: &Path,
        manifest: &ReleaseBundleManifest,
        floor_path: &Path,
        trusted_key: &TrustedReleaseKey,
    ) -> AuthenticatedReleaseStage {
        reauthenticate_release_stage(
            stage_root,
            &manifest.target,
            floor_path,
            std::slice::from_ref(trusted_key),
            Some(&trusted_key.key_id),
        )
        .expect("authenticate test release stage")
    }

    fn write_release_tar(
        release_root: &Path,
        manifest: &ReleaseBundleManifest,
        archive_path: &Path,
    ) {
        let file = fs::File::create(archive_path).expect("create test release tar");
        let encoder = GzEncoder::new(file, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        archive
            .append_dir_all(
                format!("mayhem-{}-{}", manifest.version, manifest.target),
                release_root,
            )
            .expect("append test release tree");
        archive.finish().expect("finish test release tar");
        archive
            .into_inner()
            .expect("finish test gzip stream")
            .finish()
            .expect("finish test release gzip");
    }

    enum TestTarEntry {
        File(String, Vec<u8>),
        Symlink(String, String),
    }

    fn write_test_tar(archive_path: &Path, entries: &[TestTarEntry]) {
        let file = fs::File::create(archive_path).expect("create adversarial tar");
        let encoder = GzEncoder::new(file, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        for entry in entries {
            match entry {
                TestTarEntry::File(path, bytes) => {
                    let mut header = tar::Header::new_gnu();
                    header.set_size(bytes.len() as u64);
                    header.set_mode(0o644);
                    header.set_cksum();
                    archive
                        .append_data(&mut header, path, bytes.as_slice())
                        .expect("append adversarial tar file");
                }
                TestTarEntry::Symlink(path, target) => {
                    let mut header = tar::Header::new_gnu();
                    header.set_size(0);
                    header.set_mode(0o777);
                    header.set_entry_type(tar::EntryType::Symlink);
                    header
                        .set_link_name(target)
                        .expect("set adversarial tar link target");
                    header.set_cksum();
                    archive
                        .append_data(&mut header, path, io::empty())
                        .expect("append adversarial tar symlink");
                }
            }
        }
        archive.finish().expect("finish adversarial tar");
        archive
            .into_inner()
            .expect("finish adversarial gzip stream")
            .finish()
            .expect("finish adversarial gzip");
    }

    #[derive(Clone)]
    struct TestZipEntry {
        name: String,
        contents: Vec<u8>,
        unix_mode: u32,
    }

    fn test_zip_file(name: impl Into<String>, contents: impl Into<Vec<u8>>) -> TestZipEntry {
        TestZipEntry {
            name: name.into(),
            contents: contents.into(),
            unix_mode: 0o100644,
        }
    }

    fn write_release_zip(
        release_root: &Path,
        manifest: &ReleaseBundleManifest,
        archive_path: &Path,
    ) {
        let archive_root = format!("mayhem-{}-{}", manifest.version, manifest.target);
        let source_parent = archive_path
            .parent()
            .expect("test ZIP path parent")
            .join("script-zip-source");
        fs::create_dir(&source_parent).expect("create script ZIP source parent");
        copy_directory_durable(release_root, &source_parent.join(&archive_root))
            .expect("copy script ZIP source");
        match std::process::Command::new("zip")
            .current_dir(&source_parent)
            .args(["-qr"])
            .arg(archive_path)
            .arg(&archive_root)
            .status()
        {
            Ok(status) if status.success() => return,
            Ok(status) => panic!("zip command failed with {status}"),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => panic!("starting zip command failed: {error}"),
        }

        let entries = inventory_tree(release_root, "test ZIP source")
            .expect("inventory test ZIP source")
            .into_keys()
            .map(|relative| {
                let mut entry = test_zip_file(
                    format!("{archive_root}/{relative}"),
                    fs::read(release_root.join(&relative)).expect("read test ZIP source"),
                );
                if relative.starts_with("bin/") {
                    entry.unix_mode = 0o100755;
                }
                entry
            })
            .collect::<Vec<_>>();
        write_test_zip(archive_path, &entries);
    }

    fn write_test_zip(archive_path: &Path, entries: &[TestZipEntry]) {
        struct CentralEntry {
            name: Vec<u8>,
            crc32: u32,
            compressed_size: u32,
            uncompressed_size: u32,
            unix_mode: u32,
            local_offset: u32,
        }

        let mut output = Vec::new();
        let mut central_entries = Vec::new();
        for entry in entries {
            let name = entry.name.as_bytes();
            let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
            encoder
                .write_all(&entry.contents)
                .expect("deflate test ZIP entry");
            let compressed = encoder.finish().expect("finish test ZIP deflate");
            let compressed_size =
                u32::try_from(compressed.len()).expect("test compressed size fits u32");
            let uncompressed_size =
                u32::try_from(entry.contents.len()).expect("test uncompressed size fits u32");
            let crc32 = test_crc32(&entry.contents);
            let local_offset = u32::try_from(output.len()).expect("test ZIP offset fits u32");
            push_zip_u32(&mut output, ZIP_LOCAL_FILE_HEADER_SIGNATURE);
            push_zip_u16(&mut output, 20);
            push_zip_u16(&mut output, 0);
            push_zip_u16(&mut output, 8);
            push_zip_u16(&mut output, 0);
            push_zip_u16(&mut output, 0);
            push_zip_u32(&mut output, crc32);
            push_zip_u32(&mut output, compressed_size);
            push_zip_u32(&mut output, uncompressed_size);
            push_zip_u16(
                &mut output,
                u16::try_from(name.len()).expect("test ZIP name fits u16"),
            );
            push_zip_u16(&mut output, 0);
            output.extend_from_slice(name);
            output.extend_from_slice(&compressed);
            central_entries.push(CentralEntry {
                name: name.to_vec(),
                crc32,
                compressed_size,
                uncompressed_size,
                unix_mode: entry.unix_mode,
                local_offset,
            });
        }

        let central_offset = u32::try_from(output.len()).expect("test ZIP offset fits u32");
        for entry in &central_entries {
            push_zip_u32(&mut output, ZIP_CENTRAL_DIRECTORY_SIGNATURE);
            push_zip_u16(&mut output, (3 << 8) | 20);
            push_zip_u16(&mut output, 20);
            push_zip_u16(&mut output, 0);
            push_zip_u16(&mut output, 8);
            push_zip_u16(&mut output, 0);
            push_zip_u16(&mut output, 0);
            push_zip_u32(&mut output, entry.crc32);
            push_zip_u32(&mut output, entry.compressed_size);
            push_zip_u32(&mut output, entry.uncompressed_size);
            push_zip_u16(
                &mut output,
                u16::try_from(entry.name.len()).expect("test ZIP name fits u16"),
            );
            push_zip_u16(&mut output, 0);
            push_zip_u16(&mut output, 0);
            push_zip_u16(&mut output, 0);
            push_zip_u16(&mut output, 0);
            push_zip_u32(&mut output, entry.unix_mode << 16);
            push_zip_u32(&mut output, entry.local_offset);
            output.extend_from_slice(&entry.name);
        }
        let central_size =
            u32::try_from(output.len() - central_offset as usize).expect("central size fits u32");
        push_zip_u32(&mut output, ZIP_END_OF_CENTRAL_DIRECTORY_SIGNATURE);
        push_zip_u16(&mut output, 0);
        push_zip_u16(&mut output, 0);
        push_zip_u16(
            &mut output,
            u16::try_from(central_entries.len()).expect("test ZIP entry count fits u16"),
        );
        push_zip_u16(
            &mut output,
            u16::try_from(central_entries.len()).expect("test ZIP entry count fits u16"),
        );
        push_zip_u32(&mut output, central_size);
        push_zip_u32(&mut output, central_offset);
        push_zip_u16(&mut output, 0);
        fs::write(archive_path, output).expect("write test ZIP");
    }

    fn push_zip_u16(output: &mut Vec<u8>, value: u16) {
        output.extend_from_slice(&value.to_le_bytes());
    }

    fn push_zip_u32(output: &mut Vec<u8>, value: u32) {
        output.extend_from_slice(&value.to_le_bytes());
    }

    fn test_crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in bytes {
            crc ^= *byte as u32;
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
            }
        }
        !crc
    }

    fn create_test_activation_journal(
        path: &Path,
        journal: &ActivationJournal,
        phase: ActivationPhase,
    ) {
        create_activation_journal(path, journal).expect("create activation journal");
        match phase {
            ActivationPhase::Preparing => {}
            ActivationPhase::Activating => {
                mark_activation_phase(path, ActivationPhase::Activating).expect("mark activating");
            }
            ActivationPhase::HealthGate => {
                mark_activation_phase(path, ActivationPhase::Activating).expect("mark activating");
                mark_activation_phase(path, ActivationPhase::HealthGate).expect("mark health gate");
            }
            ActivationPhase::Committing => {
                mark_activation_phase(path, ActivationPhase::Activating).expect("mark activating");
                mark_activation_phase(path, ActivationPhase::HealthGate).expect("mark health gate");
                mark_activation_phase(path, ActivationPhase::Committing).expect("mark committing");
            }
            ActivationPhase::RollingBack => {
                mark_activation_phase(path, ActivationPhase::RollingBack)
                    .expect("mark rolling back");
            }
        }
    }

    #[test]
    fn release_manifest_requires_exact_source_git_sha_and_rejects_unknown_fields() {
        let (_release, manifest, signed_manifest_bytes) = stage_release();
        let parsed: ReleaseBundleManifest =
            serde_json::from_slice(&signed_manifest_bytes).expect("parse valid release manifest");
        parsed.validate().expect("validate source-bound manifest");
        assert_eq!(parsed.source_git_sha, TEST_SOURCE_GIT_SHA);

        let mut missing = serde_json::to_value(&manifest).expect("serialize manifest value");
        missing
            .as_object_mut()
            .expect("manifest JSON object")
            .remove("source_git_sha");
        assert!(serde_json::from_value::<ReleaseBundleManifest>(missing)
            .expect_err("missing source_git_sha must fail")
            .to_string()
            .contains("missing field `source_git_sha`"));

        for malformed in [
            String::new(),
            "a".repeat(39),
            "a".repeat(41),
            "A".repeat(40),
            format!("{}g", "a".repeat(39)),
        ] {
            let mut value = serde_json::to_value(&manifest).expect("serialize manifest value");
            value["source_git_sha"] = serde_json::Value::String(malformed);
            let malformed_manifest: ReleaseBundleManifest =
                serde_json::from_value(value).expect("parse malformed source SHA shape");
            assert!(malformed_manifest
                .validate()
                .expect_err("malformed source_git_sha must fail")
                .to_string()
                .contains("exactly 40 lowercase hexadecimal characters"));
        }

        let mut unknown = serde_json::to_value(&manifest).expect("serialize manifest value");
        unknown
            .as_object_mut()
            .expect("manifest JSON object")
            .insert(
                "source_git_sha256".to_owned(),
                serde_json::Value::String("a".repeat(64)),
            );
        assert!(serde_json::from_value::<ReleaseBundleManifest>(unknown)
            .expect_err("unknown release manifest field must fail")
            .to_string()
            .contains("unknown field `source_git_sha256`"));
    }

    #[test]
    fn release_manifest_authentication_accepts_valid_exact_bytes() {
        let (_release, manifest, manifest_bytes) = stage_release();
        let (signature_bytes, trusted_key) = sign_test_release(&manifest, &manifest_bytes, 20);
        let trusted_keys = trusted_key_map(&trusted_key);

        let (authenticated_manifest, authenticated_signature) = authenticate_release_manifest(
            &manifest_bytes,
            &signature_bytes,
            &trusted_keys,
            Some(&trusted_key.key_id),
        )
        .expect("authenticate valid release manifest");

        assert_eq!(authenticated_manifest, manifest);
        assert_eq!(authenticated_signature.key_id, trusted_key.key_id);
        assert_eq!(
            authenticated_signature.sha256,
            sha256_bytes_hex(&manifest_bytes)
        );
    }

    #[test]
    fn release_manifest_authentication_rejects_tampered_manifest_and_signature() {
        let (_release, manifest, manifest_bytes) = stage_release();
        let (signature_bytes, trusted_key) = sign_test_release(&manifest, &manifest_bytes, 21);
        let trusted_keys = trusted_key_map(&trusted_key);

        let mut tampered_manifest_bytes = manifest_bytes.clone();
        tampered_manifest_bytes.push(b' ');
        assert!(authenticate_release_manifest(
            &tampered_manifest_bytes,
            &signature_bytes,
            &trusted_keys,
            Some(&trusted_key.key_id),
        )
        .expect_err("tampered manifest bytes must fail authentication")
        .to_string()
        .contains("release manifest signature hash mismatch"));

        let mut tampered_signature: ReleaseDetachedSignature =
            serde_json::from_slice(&signature_bytes).expect("parse test signature");
        let replacement = if tampered_signature.sig.starts_with('0') {
            "1"
        } else {
            "0"
        };
        tampered_signature.sig.replace_range(0..1, replacement);
        let tampered_signature_bytes =
            serde_json::to_vec(&tampered_signature).expect("serialize tampered signature");
        assert!(authenticate_release_manifest(
            &manifest_bytes,
            &tampered_signature_bytes,
            &trusted_keys,
            Some(&trusted_key.key_id),
        )
        .expect_err("tampered detached signature must fail authentication")
        .to_string()
        .contains("release manifest signature verification failed"));
    }

    #[test]
    fn release_manifest_authentication_rejects_wrong_expected_key_id() {
        let (_release, manifest, manifest_bytes) = stage_release();
        let (signature_bytes, trusted_key) = sign_test_release(&manifest, &manifest_bytes, 22);
        let trusted_keys = trusted_key_map(&trusted_key);

        assert!(authenticate_release_manifest(
            &manifest_bytes,
            &signature_bytes,
            &trusted_keys,
            Some("test-release-99"),
        )
        .expect_err("wrong expected key id must fail authentication")
        .to_string()
        .contains("signed by key id test-release-22, expected test-release-99"));
    }

    #[test]
    fn release_manifest_authentication_rejects_altered_and_untrusted_keys() {
        let (_release, manifest, manifest_bytes) = stage_release();
        let (signature_bytes, trusted_key) = sign_test_release(&manifest, &manifest_bytes, 23);
        let (_, different_key) = sign_test_release(&manifest, &manifest_bytes, 24);
        let altered_trusted_keys =
            BTreeMap::from([(trusted_key.key_id.clone(), different_key.public_key.clone())]);

        assert!(authenticate_release_manifest(
            &manifest_bytes,
            &signature_bytes,
            &altered_trusted_keys,
            Some(&trusted_key.key_id),
        )
        .expect_err("altered trusted public key must fail authentication")
        .to_string()
        .contains("public key does not match the live trusted key"));

        let untrusted_keys = trusted_key_map(&different_key);
        assert!(authenticate_release_manifest(
            &manifest_bytes,
            &signature_bytes,
            &untrusted_keys,
            Some(&trusted_key.key_id),
        )
        .expect_err("untrusted signing key must fail authentication")
        .to_string()
        .contains("release signing key test-release-23 is not trusted"));
    }

    #[test]
    fn release_manifest_authentication_rejects_malformed_and_incomplete_inventory() {
        let (_release, manifest, manifest_bytes) = stage_release();
        let (signature_bytes, trusted_key) = sign_test_release(&manifest, &manifest_bytes, 25);
        let trusted_keys = trusted_key_map(&trusted_key);

        assert!(authenticate_release_manifest(
            b"{",
            &signature_bytes,
            &trusted_keys,
            Some(&trusted_key.key_id),
        )
        .expect_err("malformed manifest must fail authentication")
        .to_string()
        .contains("parsing signed release manifest"));

        let mut incomplete_manifest = manifest;
        incomplete_manifest
            .binaries
            .retain(|binary| binary.name != "mayhem-paygate");
        incomplete_manifest
            .assets
            .retain(|asset| asset.path != "bin/mayhem-paygate");
        let incomplete_manifest_bytes =
            serde_json::to_vec(&incomplete_manifest).expect("serialize incomplete manifest");
        let (incomplete_signature_bytes, incomplete_trusted_key) =
            sign_test_release(&incomplete_manifest, &incomplete_manifest_bytes, 26);
        let incomplete_trusted_keys = trusted_key_map(&incomplete_trusted_key);

        assert!(authenticate_release_manifest(
            &incomplete_manifest_bytes,
            &incomplete_signature_bytes,
            &incomplete_trusted_keys,
            Some(&incomplete_trusted_key.key_id),
        )
        .expect_err("incomplete binary inventory must fail manifest validation")
        .to_string()
        .contains("does not include required sibling binary mayhem-paygate"));
    }

    #[test]
    fn staged_source_git_sha_is_reported_and_bound_by_hash_and_signature() {
        let (release, manifest, signed_manifest_bytes) = stage_release();
        let work = TestDir::new("source-git-sha-binding");
        let floor_path = work.path().join("install/release-floor.json");
        fs::create_dir_all(floor_path.parent().expect("floor parent"))
            .expect("create floor parent");
        initialize_release_anti_rollback_floor(&floor_path, "0.2.24")
            .expect("initialize release floor");
        let (stage_root, trusted_key) = prepare_signed_test_stage(
            release.path(),
            &manifest,
            &signed_manifest_bytes,
            work.path(),
            6,
        );

        let metadata_path = stage_root.join(RELEASE_STAGE_METADATA_PATH);
        let original_metadata_bytes =
            fs::read(&metadata_path).expect("read source-bound stage metadata");
        let metadata: ReleaseStageMetadata = serde_json::from_slice(&original_metadata_bytes)
            .expect("parse source-bound stage metadata");
        assert_eq!(metadata.source_git_sha, TEST_SOURCE_GIT_SHA);

        let authenticated =
            authenticate_test_stage(&stage_root, &manifest, &floor_path, &trusted_key);
        assert_eq!(authenticated.source_git_sha(), TEST_SOURCE_GIT_SHA);

        let mut mutated_metadata = metadata.clone();
        mutated_metadata.source_git_sha = MUTATED_SOURCE_GIT_SHA.to_owned();
        fs::write(
            &metadata_path,
            serde_json::to_vec_pretty(&mutated_metadata)
                .expect("serialize mutated source identity metadata"),
        )
        .expect("mutate staged source identity metadata");
        assert!(reauthenticate_release_stage(
            &stage_root,
            &manifest.target,
            &floor_path,
            std::slice::from_ref(&trusted_key),
            None,
        )
        .expect_err("stage metadata source SHA mutation must fail")
        .to_string()
        .contains("does not match signed manifest"));
        fs::write(&metadata_path, &original_metadata_bytes).expect("restore stage metadata");

        let mut mutated_manifest = manifest.clone();
        mutated_manifest.source_git_sha = MUTATED_SOURCE_GIT_SHA.to_owned();
        let mut mutated_manifest_bytes =
            serde_json::to_vec_pretty(&mutated_manifest).expect("serialize mutated manifest");
        mutated_manifest_bytes.push(b'\n');
        fs::write(
            stage_root
                .join(RELEASE_STAGE_PAYLOAD_ROOT)
                .join(RELEASE_MANIFEST_PATH),
            &mutated_manifest_bytes,
        )
        .expect("mutate staged signed manifest source SHA");
        assert!(reauthenticate_release_stage(
            &stage_root,
            &manifest.target,
            &floor_path,
            std::slice::from_ref(&trusted_key),
            None,
        )
        .expect_err("signed manifest source SHA mutation must fail stage hash binding")
        .to_string()
        .contains("manifest hash mismatch"));

        let mut rebound_metadata = metadata;
        rebound_metadata.manifest_sha256 = sha256_bytes_hex(&mutated_manifest_bytes);
        fs::write(
            &metadata_path,
            serde_json::to_vec_pretty(&rebound_metadata).expect("serialize rebound stage metadata"),
        )
        .expect("rebind staged manifest hash");
        assert!(reauthenticate_release_stage(
            &stage_root,
            &manifest.target,
            &floor_path,
            std::slice::from_ref(&trusted_key),
            None,
        )
        .expect_err("source SHA mutation must fail detached signature hash binding")
        .to_string()
        .contains("release manifest signature hash mismatch"));

        let signature_path = stage_root.join(RELEASE_STAGE_SIGNATURE_PATH);
        let mut signature: ReleaseDetachedSignature =
            serde_json::from_slice(&fs::read(&signature_path).expect("read detached signature"))
                .expect("parse detached signature");
        signature.sha256 = sha256_bytes_hex(&mutated_manifest_bytes);
        let mutated_signature_bytes =
            serde_json::to_vec_pretty(&signature).expect("serialize mutated signature");
        fs::write(&signature_path, &mutated_signature_bytes).expect("mutate detached signature");
        rebound_metadata.signature_sha256 = sha256_bytes_hex(&mutated_signature_bytes);
        fs::write(
            &metadata_path,
            serde_json::to_vec_pretty(&rebound_metadata)
                .expect("serialize signature-rebound metadata"),
        )
        .expect("rebind staged signature hash");
        assert!(reauthenticate_release_stage(
            &stage_root,
            &manifest.target,
            &floor_path,
            std::slice::from_ref(&trusted_key),
            None,
        )
        .expect_err("source SHA mutation must fail cryptographic signature")
        .to_string()
        .contains("release manifest signature verification failed"));
    }

    #[test]
    fn tar_release_is_bounded_staged_and_reauthenticated_from_live_trust() {
        let (release, manifest, manifest_bytes) = stage_release();
        let work = TestDir::new("tar-stage");
        let archive_path = work.path().join("release.tar.gz");
        write_release_tar(release.path(), &manifest, &archive_path);
        let (signature_bytes, trusted_key) = sign_test_release(&manifest, &manifest_bytes, 7);
        let stage_root = work.path().join("staged");
        let floor_path = work.path().join("install/release-floor.json");
        fs::create_dir_all(floor_path.parent().expect("floor parent"))
            .expect("create floor parent");
        initialize_release_anti_rollback_floor(&floor_path, "0.2.24")
            .expect("initialize release floor");

        let prepared = stage_release_archive(
            &archive_path,
            &manifest_bytes,
            &signature_bytes,
            &stage_root,
            1_800_000_000,
            &ReleaseExtractionLimits::default(),
        )
        .expect("stage bounded tar release");
        assert_eq!(prepared.stage_root(), stage_root);
        assert_eq!(prepared.source_git_sha(), TEST_SOURCE_GIT_SHA);
        assert!(prepared
            .payload_root()
            .join("bin/mayhem-attestation-verifier")
            .is_file());

        let authenticated = reauthenticate_release_stage(
            &stage_root,
            &manifest.target,
            &floor_path,
            std::slice::from_ref(&trusted_key),
            Some(&trusted_key.key_id),
        )
        .expect("reauthenticate staged release");
        assert_eq!(authenticated.manifest(), &manifest);
        assert_eq!(
            authenticated
                .verified_release()
                .primary_binary()
                .file_name()
                .and_then(|name| name.to_str()),
            Some("mayhem")
        );

        let wrong_key = sign_test_release(&manifest, &manifest_bytes, 8).1;
        assert!(reauthenticate_release_stage(
            &stage_root,
            &manifest.target,
            &floor_path,
            &[wrong_key],
            None,
        )
        .expect_err("stage must be checked against live trust")
        .to_string()
        .contains("is not trusted"));
        let higher_floor_path = work.path().join("higher/release-floor.json");
        fs::create_dir_all(higher_floor_path.parent().expect("higher floor parent"))
            .expect("create higher floor parent");
        initialize_release_anti_rollback_floor(&higher_floor_path, "0.2.25")
            .expect("initialize higher release floor");
        assert!(reauthenticate_release_stage(
            &stage_root,
            &manifest.target,
            &higher_floor_path,
            std::slice::from_ref(&trusted_key),
            None,
        )
        .expect_err("protected version floor must reject rollback")
        .to_string()
        .contains("anti-rollback floor"));

        let metadata_path = stage_root.join(RELEASE_STAGE_METADATA_PATH);
        let original_metadata = fs::read(&metadata_path).expect("read stage metadata");
        let mut metadata: ReleaseStageMetadata =
            serde_json::from_slice(&original_metadata).expect("parse stage metadata");
        metadata.manifest_sha256 = "0".repeat(64);
        fs::write(
            &metadata_path,
            serde_json::to_vec_pretty(&metadata).expect("serialize tampered metadata"),
        )
        .expect("tamper stage metadata");
        assert!(reauthenticate_release_stage(
            &stage_root,
            &manifest.target,
            &floor_path,
            std::slice::from_ref(&trusted_key),
            None,
        )
        .expect_err("stage metadata must not substitute for signed bytes")
        .to_string()
        .contains("manifest hash mismatch"));
        fs::write(&metadata_path, original_metadata).expect("restore stage metadata");

        write_file(&stage_root, "unexpected", b"extra");
        assert!(reauthenticate_release_stage(
            &stage_root,
            &manifest.target,
            &floor_path,
            std::slice::from_ref(&trusted_key),
            None,
        )
        .expect_err("stage envelope extras must fail")
        .to_string()
        .contains("unlisted extra entry"));
    }

    #[test]
    fn windows_zip_release_is_safely_staged_and_reauthenticated() {
        let (release, manifest, manifest_bytes) =
            stage_release_for("v0.2.24", "x86_64-pc-windows-msvc");
        let work = TestDir::new("zip-stage");
        let archive_path = work.path().join("release.zip");
        write_release_zip(release.path(), &manifest, &archive_path);
        let (signature_bytes, trusted_key) = sign_test_release(&manifest, &manifest_bytes, 9);
        let stage_root = work.path().join("staged");
        let floor_path = work.path().join("install/release-floor.json");
        fs::create_dir_all(floor_path.parent().expect("floor parent"))
            .expect("create floor parent");
        initialize_release_anti_rollback_floor(&floor_path, "0.2.24")
            .expect("initialize release floor");

        stage_release_archive(
            &archive_path,
            &manifest_bytes,
            &signature_bytes,
            &stage_root,
            1_800_000_000,
            &ReleaseExtractionLimits::default(),
        )
        .expect("stage bounded Windows ZIP release");
        let authenticated = reauthenticate_release_stage(
            &stage_root,
            &manifest.target,
            &floor_path,
            std::slice::from_ref(&trusted_key),
            None,
        )
        .expect("reauthenticate Windows ZIP stage");
        assert!(authenticated
            .verified_release()
            .bin_root()
            .join("mayhem-attestation-verifier.exe")
            .is_file());
    }

    #[test]
    fn tar_extraction_rejects_links_duplicates_extras_and_limits() {
        let (_release, manifest, _manifest_bytes) = stage_release();
        let work = TestDir::new("negative-tar");
        let archive_root = format!("mayhem-{}-{}", manifest.version, manifest.target);
        let expected = format!("{archive_root}/README.md");
        let limits = ReleaseExtractionLimits::default();

        let cases = [
            (
                "link",
                vec![TestTarEntry::Symlink(
                    expected.clone(),
                    "../../escape".to_owned(),
                )],
                "link or special file",
            ),
            (
                "duplicate",
                vec![
                    TestTarEntry::File(expected.clone(), b"first".to_vec()),
                    TestTarEntry::File(expected.clone(), b"second".to_vec()),
                ],
                "duplicate path",
            ),
            (
                "extra",
                vec![TestTarEntry::File(
                    format!("{archive_root}/unlisted"),
                    b"extra".to_vec(),
                )],
                "unlisted extra file",
            ),
        ];
        for (label, entries, expected_error) in cases {
            let archive_path = work.path().join(format!("{label}.tar.gz"));
            write_test_tar(&archive_path, &entries);
            let payload = work.path().join(format!("{label}-payload"));
            fs::create_dir(&payload).expect("create negative tar payload");
            assert!(extract_release_archive_bounded(
                &archive_path,
                ReleaseArchiveFormat::TarGz,
                &manifest,
                &payload,
                &limits,
            )
            .expect_err("adversarial tar must fail")
            .to_string()
            .contains(expected_error));
        }

        let archive_path = work.path().join("bounded.tar.gz");
        write_test_tar(
            &archive_path,
            &[TestTarEntry::File(expected, b"too-large".to_vec())],
        );
        let payload = work.path().join("bounded-payload");
        fs::create_dir(&payload).expect("create bounded tar payload");
        let tiny_limits = ReleaseExtractionLimits {
            max_file_bytes: 2,
            max_total_file_bytes: 2,
            ..limits
        };
        assert!(extract_release_archive_bounded(
            &archive_path,
            ReleaseArchiveFormat::TarGz,
            &manifest,
            &payload,
            &tiny_limits,
        )
        .expect_err("oversized tar entry must fail")
        .to_string()
        .contains("max_file_bytes"));
    }

    #[test]
    fn zip_extraction_rejects_escape_links_duplicates_extras_and_limits() {
        let (_release, manifest, _manifest_bytes) =
            stage_release_for("v0.2.24", "x86_64-pc-windows-msvc");
        let work = TestDir::new("negative-zip");
        let archive_root = format!("mayhem-{}-{}", manifest.version, manifest.target);
        let expected = format!("{archive_root}/README.md");
        let mut link = test_zip_file(expected.clone(), b"link".to_vec());
        link.unix_mode = 0o120777;
        let cases = [
            (
                "escape",
                vec![test_zip_file(
                    format!("{archive_root}/../escape"),
                    b"escape".to_vec(),
                )],
                "is unsafe",
            ),
            ("link", vec![link], "link or special file"),
            (
                "duplicate",
                vec![
                    test_zip_file(expected.clone(), b"first".to_vec()),
                    test_zip_file(expected.clone(), b"second".to_vec()),
                ],
                "duplicate path",
            ),
            (
                "extra",
                vec![test_zip_file(
                    format!("{archive_root}/unlisted"),
                    b"extra".to_vec(),
                )],
                "unlisted extra file",
            ),
        ];
        for (label, entries, expected_error) in cases {
            let archive_path = work.path().join(format!("{label}.zip"));
            write_test_zip(&archive_path, &entries);
            let payload = work.path().join(format!("{label}-payload"));
            fs::create_dir(&payload).expect("create negative ZIP payload");
            assert!(extract_release_archive_bounded(
                &archive_path,
                ReleaseArchiveFormat::Zip,
                &manifest,
                &payload,
                &ReleaseExtractionLimits::default(),
            )
            .expect_err("adversarial ZIP must fail")
            .to_string()
            .contains(expected_error));
        }

        let archive_path = work.path().join("bounded.zip");
        write_test_zip(
            &archive_path,
            &[test_zip_file(expected, b"too-large".to_vec())],
        );
        let payload = work.path().join("bounded-payload");
        fs::create_dir(&payload).expect("create bounded ZIP payload");
        let tiny_limits = ReleaseExtractionLimits {
            max_file_bytes: 2,
            max_total_file_bytes: 2,
            ..ReleaseExtractionLimits::default()
        };
        assert!(extract_release_archive_bounded(
            &archive_path,
            ReleaseArchiveFormat::Zip,
            &manifest,
            &payload,
            &tiny_limits,
        )
        .expect_err("oversized ZIP entry must fail")
        .to_string()
        .contains("max_file_bytes"));
    }

    #[test]
    fn outer_manifest_rejects_extra_missing_and_tampered_payloads() {
        let (stage, manifest, signed) = stage_release();
        let verified =
            verify_staged_release_tree(&manifest, &signed, stage.path()).expect("verify release");
        assert_eq!(verified.file_count(), manifest.assets.len());
        assert_eq!(verified.bin_root(), stage.path().join(RELEASE_BIN_ROOT));
        assert_eq!(verified.asset_root(), stage.path().join(MAYHEM_ASSET_ROOT));
        assert_eq!(
            verified.primary_binary_in(Path::new("/opt/mayhem/bin")),
            PathBuf::from("/opt/mayhem/bin/mayhem")
        );

        write_file(stage.path(), "bin/mayhem-debug", b"extra");
        assert!(verify_staged_release_tree(&manifest, &signed, stage.path())
            .expect_err("unlisted sibling binary must fail")
            .to_string()
            .contains("unlisted extra payload"));
        fs::remove_file(stage.path().join("bin/mayhem-debug")).expect("remove extra binary");

        fs::create_dir(stage.path().join("share/mayhem/empty-extra"))
            .expect("create unlisted empty directory");
        assert!(verify_staged_release_tree(&manifest, &signed, stage.path())
            .expect_err("unlisted empty directory must fail")
            .to_string()
            .contains("unlisted extra directory"));
        fs::remove_dir(stage.path().join("share/mayhem/empty-extra"))
            .expect("remove unlisted empty directory");

        fs::remove_file(stage.path().join("bin/mayhem-gateway")).expect("remove sibling binary");
        assert!(verify_staged_release_tree(&manifest, &signed, stage.path())
            .expect_err("missing sibling binary must fail")
            .to_string()
            .contains("is missing"));
        write_file(stage.path(), "bin/mayhem-gateway", b"new-mayhem-gateway");

        fs::write(stage.path().join("bin/mayhem-paygate"), b"tampered")
            .expect("tamper sibling binary");
        assert!(verify_staged_release_tree(&manifest, &signed, stage.path())
            .expect_err("tampered payload must fail")
            .to_string()
            .contains("SHA-256 mismatch"));
    }

    #[test]
    fn outer_manifest_and_checksum_paths_are_strictly_safe() {
        let (stage, mut manifest, signed) = stage_release();
        let mut missing_binary_listing = manifest.clone();
        missing_binary_listing
            .binaries
            .retain(|binary| binary.name != "mayhem-gateway");
        assert!(missing_binary_listing
            .validate()
            .expect_err("signed sibling without binary listing must fail")
            .to_string()
            .contains("not listed as a release binary"));

        let mut missing_attestation_verifier = manifest.clone();
        missing_attestation_verifier
            .binaries
            .retain(|binary| binary.name != "mayhem-attestation-verifier");
        missing_attestation_verifier
            .assets
            .retain(|asset| asset.path != "bin/mayhem-attestation-verifier");
        assert!(missing_attestation_verifier
            .validate()
            .expect_err("attestation verifier sibling must be required")
            .to_string()
            .contains("required sibling binary mayhem-attestation-verifier"));

        manifest.assets[0].path = "../escape".to_owned();
        assert!(manifest
            .validate()
            .expect_err("unsafe manifest path must fail")
            .to_string()
            .contains("is unsafe"));

        let checksums = stage.path().join(RELEASE_CHECKSUMS_PATH);
        let original = fs::read_to_string(&checksums).expect("read checksums");
        fs::write(&checksums, original.replace("bin/mayhem", "bin/../mayhem"))
            .expect("write unsafe checksums");
        assert!(verify_staged_release_tree(
            &serde_json::from_slice(&signed).expect("parse signed manifest"),
            &signed,
            stage.path()
        )
        .expect_err("unsafe checksum path must fail")
        .to_string()
        .contains("is unsafe"));
    }

    #[test]
    fn release_version_binding_accepts_optional_v_and_rejects_drift() {
        let intercom = intercom_manifest_for(&[("src/main.js", b"main")]);
        verify_release_version_binding("0.2.24", &intercom).expect("bind plain version");
        verify_release_version_binding("v0.2.24", &intercom).expect("bind tagged version");
        assert!(verify_release_version_binding("v0.2.25", &intercom)
            .expect_err("version drift must fail")
            .to_string()
            .contains("does not bind"));
        assert!(verify_release_version_binding("release-0.2.24", &intercom)
            .expect_err("implicit version must fail")
            .to_string()
            .contains("explicit semantic version"));
    }

    #[test]
    fn coordinated_bin_and_asset_activation_updates_all_binaries_and_rolls_back_together() {
        let (release, manifest, signed) = stage_release();
        let temp = TestDir::new("activation");
        let transaction_dir = temp.path().join("install");
        let active_bin_root = transaction_dir.join(RELEASE_BIN_ROOT);
        let active_assets = transaction_dir.join("share/mayhem");
        let floor_path = transaction_dir.join("updates/release-floor.json");
        let stores = transaction_dir.join("stores");
        for (name, _) in TEST_RELEASE_BINARIES {
            let old = format!("old-{name}");
            write_file(&active_bin_root, name, old.as_bytes());
        }
        write_file(&active_bin_root, "legacy-helper", b"old-legacy");
        write_file(
            &transaction_dir,
            "share/mayhem/catalog/models.json",
            b"old-assets",
        );
        write_file(&stores, "main/db", b"state");
        fs::create_dir_all(floor_path.parent().expect("floor parent"))
            .expect("create floor parent");
        initialize_release_anti_rollback_floor(&floor_path, "0.2.22")
            .expect("initialize release floor");
        let (stage_root, trusted_key) =
            prepare_signed_test_stage(release.path(), &manifest, &signed, temp.path(), 31);
        let authenticated =
            authenticate_test_stage(&stage_root, &manifest, &floor_path, &trusted_key);
        assert!(authenticated
            .verified_release()
            .bin_root()
            .join("mayhem-gateway")
            .exists());
        assert!(authenticated.verified_release().asset_root().exists());

        let activation = activate_authenticated_release(
            &transaction_dir,
            authenticated,
            &active_bin_root,
            &active_assets,
            std::slice::from_ref(&stores),
        )
        .expect("activate verified release");
        for (name, expected) in TEST_RELEASE_BINARIES {
            assert_eq!(
                fs::read(active_bin_root.join(name)).expect("read activated sibling binary"),
                *expected
            );
        }
        assert!(!active_bin_root.join("legacy-helper").exists());
        assert_eq!(
            fs::read(active_assets.join("RULES.md")).expect("read activated assets"),
            b"rules"
        );
        assert_eq!(
            fs::read(stores.join("main/db")).expect("read store"),
            b"state"
        );
        assert_eq!(
            read_release_anti_rollback_floor(&floor_path).expect("read activated floor"),
            "0.2.24"
        );
        assert_eq!(
            activation.backup_path(0).expect("bin backup").parent(),
            active_bin_root.parent()
        );
        assert_eq!(
            activation.backup_path(1).expect("asset backup").parent(),
            active_assets.parent()
        );
        activation.rollback().expect("rollback transaction");
        for (name, _) in TEST_RELEASE_BINARIES {
            assert_eq!(
                fs::read(active_bin_root.join(name)).expect("read rolled back sibling binary"),
                format!("old-{name}").as_bytes()
            );
        }
        assert_eq!(
            fs::read(active_bin_root.join("legacy-helper")).expect("read restored legacy binary"),
            b"old-legacy"
        );
        assert_eq!(
            fs::read(active_assets.join("catalog/models.json")).expect("read assets"),
            b"old-assets"
        );
        assert_eq!(
            fs::read(stores.join("main/db")).expect("read store"),
            b"state"
        );
        assert_eq!(
            read_release_anti_rollback_floor(&floor_path).expect("read rolled back floor"),
            "0.2.22"
        );

        let authenticated =
            authenticate_test_stage(&stage_root, &manifest, &floor_path, &trusted_key);
        let activation = activate_authenticated_release(
            &transaction_dir,
            authenticated,
            &active_bin_root,
            &active_assets,
            std::slice::from_ref(&stores),
        )
        .expect("reactivate verified release");
        activation.commit().expect("commit transaction");
        for (name, expected) in TEST_RELEASE_BINARIES {
            assert_eq!(
                fs::read(active_bin_root.join(name)).expect("read committed sibling binary"),
                *expected
            );
        }
        assert!(!active_bin_root.join("legacy-helper").exists());
        assert_eq!(
            fs::read(active_assets.join("RULES.md")).expect("read committed assets"),
            b"rules"
        );
        assert!(!activation_journal_path(&transaction_dir).exists());
        assert_eq!(
            fs::read(stores.join("main/db")).expect("read store"),
            b"state"
        );
        assert_eq!(
            read_release_anti_rollback_floor(&floor_path).expect("read committed floor"),
            "0.2.24"
        );
    }

    #[test]
    fn activation_rejects_stage_mutation_before_touching_active_destinations() {
        let (release, manifest, signed) = stage_release();
        let temp = TestDir::new("mutated-authenticated-stage");
        let transaction_dir = temp.path().join("install");
        let active_bin = transaction_dir.join(RELEASE_BIN_ROOT);
        let active_share = transaction_dir.join(MAYHEM_ASSET_ROOT);
        let floor_path = transaction_dir.join("updates/release-floor.json");
        let stores = transaction_dir.join("stores");
        write_file(&active_bin, "mayhem", b"old-bin");
        write_file(&active_share, "RULES.md", b"old-share");
        write_file(&stores, "main/db", b"state");
        fs::create_dir_all(floor_path.parent().expect("floor parent"))
            .expect("create floor parent");
        initialize_release_anti_rollback_floor(&floor_path, "0.2.22")
            .expect("initialize release floor");
        let (stage_root, trusted_key) =
            prepare_signed_test_stage(release.path(), &manifest, &signed, temp.path(), 32);
        let authenticated =
            authenticate_test_stage(&stage_root, &manifest, &floor_path, &trusted_key);

        fs::write(
            stage_root.join("release/bin/mayhem-gateway"),
            b"mutated-after-authentication",
        )
        .expect("mutate authenticated stage");
        let error = activate_authenticated_release(
            &transaction_dir,
            authenticated,
            &active_bin,
            &active_share,
            std::slice::from_ref(&stores),
        )
        .expect_err("mutated authenticated stage must not activate");
        let error = format!("{error:#}");
        assert!(error.contains("SHA-256 mismatch"), "{error}");
        assert_eq!(
            fs::read(active_bin.join("mayhem")).expect("read active bin"),
            b"old-bin"
        );
        assert_eq!(
            fs::read(active_share.join("RULES.md")).expect("read active share"),
            b"old-share"
        );
        assert_eq!(
            read_release_anti_rollback_floor(&floor_path).expect("read active floor"),
            "0.2.22"
        );
        assert_eq!(
            fs::read(stores.join("main/db")).expect("read protected store"),
            b"state"
        );
        assert!(!activation_journal_path(&transaction_dir).exists());
    }

    #[test]
    fn activation_refuses_targets_overlapping_protected_stores() {
        let temp = TestDir::new("protected-store");
        let transaction_dir = temp.path().join("install");
        let staged = temp.path().join("staged");
        let stores = transaction_dir.join("stores");
        write_file(&staged, "new/payload", b"new");
        write_file(&stores, "main/db", b"state");
        let target = ActivationTarget::new(staged.join("new"), stores.join("runtime"));
        assert!(validate_activation_targets(
            &transaction_dir,
            &[target],
            &[ActivationTargetRole::BinRoot],
            std::slice::from_ref(&stores),
            Some("123-456-0"),
        )
        .expect_err("store overlap must fail")
        .to_string()
        .contains("protected store path"));
        assert_eq!(
            fs::read(stores.join("main/db")).expect("read store"),
            b"state"
        );
    }

    #[test]
    fn activation_journal_phase_transitions_are_append_only() {
        let temp = TestDir::new("append-only-journal");
        let transaction_dir = temp.path().join("install");
        fs::create_dir_all(&transaction_dir).expect("create transaction directory");
        let journal_path = activation_journal_path(&transaction_dir);
        let journal = ActivationJournal {
            schema: ACTIVATION_JOURNAL_SCHEMA,
            id: "123-456-0".to_owned(),
            manifest_sha256: "a".repeat(64),
            release_version: "0.2.24".to_owned(),
            previous_floor_version: "0.2.22".to_owned(),
            entries: vec![ActivationJournalEntry {
                role: ActivationTargetRole::BinRoot,
                destination: transaction_dir.join("bin").to_string_lossy().into_owned(),
                kind: ActivationTargetKind::Directory,
                had_active: true,
            }],
        };
        create_activation_journal(&journal_path, &journal).expect("create journal");
        let immutable_record =
            fs::read(journal_path.join(ACTIVATION_JOURNAL_RECORD)).expect("read journal record");

        mark_activation_phase(&journal_path, ActivationPhase::Activating).expect("mark activating");
        mark_activation_phase(&journal_path, ActivationPhase::HealthGate)
            .expect("mark health gate");
        mark_activation_phase(&journal_path, ActivationPhase::Committing).expect("mark committing");
        mark_activation_phase(&journal_path, ActivationPhase::Committing)
            .expect("repeat committing idempotently");

        assert_eq!(
            fs::read(journal_path.join(ACTIVATION_JOURNAL_RECORD)).expect("reread journal record"),
            immutable_record
        );
        for marker in [
            ACTIVATION_PHASE_READY,
            ACTIVATION_PHASE_ACTIVATING,
            ACTIVATION_PHASE_HEALTH_GATE,
            ACTIVATION_PHASE_COMMITTING,
        ] {
            assert!(journal_path.join(marker).is_file());
        }
        assert_eq!(
            read_activation_phase(&journal_path).expect("read journal phase"),
            Some(ActivationPhase::Committing)
        );
        remove_activation_journal(&journal_path).expect("remove journal");
    }

    #[test]
    fn windows_bin_activation_requires_an_external_detached_helper() {
        let active_bin_root = Path::new("/install/bin");
        let running_from_bin = Path::new("/install/bin/mayhem.exe");
        assert_eq!(
            release_activation_requirement_for_platform(true, running_from_bin, active_bin_root),
            ReleaseActivationRequirement::DetachedHelperRequired {
                running_executable: running_from_bin.to_path_buf()
            }
        );
        assert_eq!(
            release_activation_requirement_for_platform(
                true,
                Path::new("/install/.update-helper/mayhem.exe"),
                active_bin_root
            ),
            ReleaseActivationRequirement::InProcessSafe
        );
        assert_eq!(
            release_activation_requirement_for_platform(false, running_from_bin, active_bin_root),
            ReleaseActivationRequirement::InProcessSafe
        );
    }

    #[cfg(unix)]
    #[test]
    fn activation_refuses_a_symlinked_destination_ancestor() {
        use std::os::unix::fs::symlink;

        let temp = TestDir::new("symlinked-destination");
        let transaction_dir = temp.path().join("install");
        let staged = temp.path().join("staged");
        let stores = transaction_dir.join("stores");
        write_file(&staged, "new/payload", b"new");
        write_file(&stores, "main/db", b"state");
        symlink(&stores, transaction_dir.join("redirect")).expect("create redirect");

        let target =
            ActivationTarget::new(staged.join("new"), transaction_dir.join("redirect/runtime"));
        assert!(validate_activation_targets(
            &transaction_dir,
            &[target],
            &[ActivationTargetRole::BinRoot],
            std::slice::from_ref(&stores),
            Some("123-456-0"),
        )
        .expect_err("symlinked destination ancestor must fail")
        .to_string()
        .contains("ancestor must be a real directory"));
        assert_eq!(
            fs::read(stores.join("main/db")).expect("read store"),
            b"state"
        );
    }

    fn create_crashed_release_activation(
        transaction_dir: &Path,
        phase: ActivationPhase,
        swapped_entries: usize,
    ) -> (ActivationJournal, PathBuf, PathBuf, PathBuf, PathBuf) {
        let active_bin = transaction_dir.join(RELEASE_BIN_ROOT);
        let active_share = transaction_dir.join(MAYHEM_ASSET_ROOT);
        let floor_path = transaction_dir.join("updates/release-floor.json");
        let stores = transaction_dir.join("stores");
        write_file(&active_bin, "mayhem", b"old-bin");
        write_file(&active_share, "RULES.md", b"old-share");
        write_file(&stores, "main/db", b"protected");
        fs::create_dir_all(floor_path.parent().expect("floor parent"))
            .expect("create floor parent");
        initialize_release_anti_rollback_floor(&floor_path, "0.2.22")
            .expect("initialize old release floor");

        let journal = ActivationJournal {
            schema: ACTIVATION_JOURNAL_SCHEMA,
            id: "123-456-0".to_owned(),
            manifest_sha256: "a".repeat(64),
            release_version: "0.2.24".to_owned(),
            previous_floor_version: "0.2.22".to_owned(),
            entries: vec![
                ActivationJournalEntry {
                    role: ActivationTargetRole::BinRoot,
                    destination: active_bin.to_string_lossy().into_owned(),
                    kind: ActivationTargetKind::Directory,
                    had_active: true,
                },
                ActivationJournalEntry {
                    role: ActivationTargetRole::AssetRoot,
                    destination: active_share.to_string_lossy().into_owned(),
                    kind: ActivationTargetKind::Directory,
                    had_active: true,
                },
                ActivationJournalEntry {
                    role: ActivationTargetRole::AntiRollbackFloor,
                    destination: floor_path.to_string_lossy().into_owned(),
                    kind: ActivationTargetKind::File,
                    had_active: true,
                },
            ],
        };
        let incoming_bin = activation_scratch_path(&journal.id, 0, &journal.entries[0], "new");
        let incoming_share = activation_scratch_path(&journal.id, 1, &journal.entries[1], "new");
        let incoming_floor = activation_scratch_path(&journal.id, 2, &journal.entries[2], "new");
        write_file(&incoming_bin, "mayhem", b"new-bin");
        write_file(&incoming_share, "RULES.md", b"new-share");
        write_release_anti_rollback_floor(&incoming_floor, "0.2.24")
            .expect("write incoming release floor");
        let journal_path = activation_journal_path(transaction_dir);
        create_test_activation_journal(
            &journal_path,
            &journal,
            if phase == ActivationPhase::Preparing {
                ActivationPhase::Preparing
            } else {
                ActivationPhase::Activating
            },
        );
        for (index, entry) in journal.entries.iter().take(swapped_entries).enumerate() {
            let destination = Path::new(&entry.destination);
            let incoming = activation_scratch_path(&journal.id, index, entry, "new");
            let backup = activation_scratch_path(&journal.id, index, entry, "old");
            fs::rename(destination, &backup).expect("move old destination to backup");
            fs::rename(incoming, destination).expect("activate incoming destination");
        }
        match phase {
            ActivationPhase::Preparing | ActivationPhase::Activating => {}
            ActivationPhase::HealthGate => {
                mark_activation_phase(&journal_path, ActivationPhase::HealthGate)
                    .expect("mark crashed health gate");
            }
            ActivationPhase::Committing => {
                mark_activation_phase(&journal_path, ActivationPhase::HealthGate)
                    .expect("mark crashed health gate");
                mark_activation_phase(&journal_path, ActivationPhase::Committing)
                    .expect("mark crashed commit");
            }
            ActivationPhase::RollingBack => {
                mark_activation_phase(&journal_path, ActivationPhase::RollingBack)
                    .expect("mark crashed rollback");
            }
        }
        (journal, active_bin, active_share, floor_path, stores)
    }

    #[test]
    fn recovery_rolls_back_bin_share_and_floor_at_every_precommit_crash_phase() {
        for (phase, swapped_entries) in [
            (ActivationPhase::Preparing, 0),
            (ActivationPhase::Activating, 0),
            (ActivationPhase::Activating, 1),
            (ActivationPhase::Activating, 2),
            (ActivationPhase::Activating, 3),
            (ActivationPhase::HealthGate, 3),
            (ActivationPhase::RollingBack, 2),
        ] {
            let temp = TestDir::new("precommit-crash");
            let transaction_dir = temp.path().join("install");
            let (journal, active_bin, active_share, floor_path, stores) =
                create_crashed_release_activation(&transaction_dir, phase, swapped_entries);

            assert_eq!(
                recover_release_activation(
                    &transaction_dir,
                    &active_bin,
                    &active_share,
                    &floor_path,
                    std::slice::from_ref(&stores),
                )
                .expect("recover precommit activation"),
                ActivationRecovery::RolledBack
            );
            assert_eq!(
                fs::read(active_bin.join("mayhem")).expect("read recovered bin"),
                b"old-bin"
            );
            assert_eq!(
                fs::read(active_share.join("RULES.md")).expect("read recovered share"),
                b"old-share"
            );
            assert_eq!(
                read_release_anti_rollback_floor(&floor_path).expect("read recovered floor"),
                "0.2.22"
            );
            for (index, entry) in journal.entries.iter().enumerate() {
                assert!(!activation_scratch_path(&journal.id, index, entry, "new").exists());
                assert!(!activation_scratch_path(&journal.id, index, entry, "old").exists());
            }
            assert_eq!(
                fs::read(stores.join("main/db")).expect("read protected store"),
                b"protected"
            );
            assert!(!activation_journal_path(&transaction_dir).exists());
        }
    }

    #[test]
    fn recovery_completes_bin_share_and_floor_after_each_commit_cleanup_crash() {
        for cleaned_backups in 0..=3 {
            let temp = TestDir::new("commit-cleanup-crash");
            let transaction_dir = temp.path().join("install");
            let (journal, active_bin, active_share, floor_path, stores) =
                create_crashed_release_activation(&transaction_dir, ActivationPhase::Committing, 3);
            for (index, entry) in journal.entries.iter().take(cleaned_backups).enumerate() {
                remove_path_if_present(&activation_scratch_path(&journal.id, index, entry, "old"))
                    .expect("simulate committed backup cleanup");
            }

            assert_eq!(
                recover_release_activation(
                    &transaction_dir,
                    &active_bin,
                    &active_share,
                    &floor_path,
                    std::slice::from_ref(&stores),
                )
                .expect("recover committed activation"),
                ActivationRecovery::CommitCompleted
            );
            assert_eq!(
                fs::read(active_bin.join("mayhem")).expect("read committed bin"),
                b"new-bin"
            );
            assert_eq!(
                fs::read(active_share.join("RULES.md")).expect("read committed share"),
                b"new-share"
            );
            assert_eq!(
                read_release_anti_rollback_floor(&floor_path).expect("read committed floor"),
                "0.2.24"
            );
            assert_eq!(
                fs::read(stores.join("main/db")).expect("read protected store"),
                b"protected"
            );
            assert!(!activation_journal_path(&transaction_dir).exists());
        }
    }
}
