use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;

use anyhow::{bail, ensure, Context, Result};
use fs2::FileExt as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

const IMAGE: &str =
    "lmsysorg/sglang@sha256:12d3392bdc8be8d35e9a95f191df6aef99c5114bdbefd41bfdc7e760e6d25ec1";
const SOURCE_FORMAT: &str = "pennyroyal_source_bundle_tar_gzip_v1";
const SOURCE_LAYOUT: &str = "source";
const MATERIALIZATION: &str = "derive_from_signed_snapshot_v1";
const SECURITY_PROFILE: &str = "docker_29_1_3_default_plus_io_uring_v1";
const RESOURCE_PROFILE: &str = "single_sm120_96g_hostnet_hostipc_v1";
const LAUNCH_PROFILE: &str = "pennyroyal_flash_next_frspec_524k_nvme_deterministic_v1";
const SECCOMP: &[u8] = include_bytes!("../assets/docker-29.1.3-ple-io-uring.json");
const SECCOMP_SHA256: &str = "c7a33fb8ae1f8346356a61ce833d579c45acf2bc94967c6763634e81010ff816";

const LAUNCHER: &str = "configs/pennyroyal/serve-flash-next-frspec.sh";
const CHAT_TEMPLATE: &str = "configs/pennyroyal/templates/froggeric-v22.5.jinja";
const FRSPEC_MAP: &str = "configs/pennyroyal/frspec/flash-next-64k.pt";
const FRSPEC_MANIFEST: &str = "configs/pennyroyal/frspec/flash-next-64k.manifest.json";
const NIXL_CONFIG: &str = "configs/pennyroyal/nixl-posix-frspec.toml";
const PLE_PREPARER: &str = "scripts/pennyroyal/prepare_ple_nvme.py";
const PLE_CHECKER: &str = "scripts/pennyroyal/check_ple_nvme.py";
const PLE_PLUGIN_SOURCE: &str = "tools/ple_nvme/ssd_stream";
const PLE_READER_WHEEL_FILENAME: &str =
    "sglang_ssd_stream-0.2.0+pennyroyal2-cp312-cp312-linux_x86_64.whl";
const PLE_READER_WHEEL_BYTES: u64 = 292_987;
const PLE_READER_WHEEL_SHA256: &str =
    "5fd3bf79524aec7068729e99823a8278e4f3bd7dcacd222f2c55f4395454112f";

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ManagedRuntimeRecipeV1 {
    PennyroyalFlashNextFrspecV1 {
        schema_version: u32,
        profile_version: u32,
        public_model_id: String,
        artifact: RecipeArtifact,
        source: RecipeSource,
        proofs: RecipeProofs,
        ple: RecipePle,
        runtime: RecipeRuntime,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecipeArtifact {
    repo: String,
    revision: String,
    snapshot_manifest_sidecar: String,
    snapshot_manifest_sha256: String,
    file_count: u32,
    total_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecipeSource {
    sidecar: String,
    format: String,
    root_layout: String,
    revision: String,
    archive_bytes: u64,
    archive_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecipeProofs {
    launcher_sha256: String,
    chat_template_sha256: String,
    frspec_map_sha256: String,
    frspec_manifest_sha256: String,
    tokenizer_sha256: String,
    ple_plugin_source_inventory_sha256: String,
    seccomp_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecipePle {
    materialization: String,
    source_config_sha256: String,
    source_index_sha256: String,
    table_relpath: String,
    table_bytes: u64,
    table_sha256: String,
    table_rows: u64,
    table_columns: u32,
    table_dtype: String,
    portable_manifest_bytes: u64,
    portable_manifest_sha256: String,
    reader_version: String,
    reader_wheel: RecipeReaderWheel,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecipeReaderWheel {
    sidecar: String,
    filename: String,
    bytes: u64,
    sha256: String,
    python_tag: String,
    abi_tag: String,
    platform_tag: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecipeRuntime {
    image: String,
    security_profile: RecipeSecurityProfile,
    resource_profile: String,
    launch_profile: String,
    deterministic_inference: bool,
    attention_backend: String,
    linear_attn_prefill_backend: String,
    linear_attn_decode_backend: String,
    provider_max_concurrent: u32,
    scheduler_max_running_requests: u32,
    reasoning_default: String,
    reasoning_overrides: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeSecurityProfile {
    kind: String,
    sidecar: String,
    bytes: u64,
    sha256: String,
}

pub(crate) struct ManagedRuntimeInputs<'a> {
    pub home: &'a Path,
    pub docker: &'a Path,
    pub provider_id: &'a str,
    pub enclave_id: &'a str,
    pub public_model_id: &'a str,
    pub artifact_repo: &'a str,
    pub artifact_revision: &'a str,
    pub snapshot_manifest_sidecar: &'a str,
    pub snapshot_manifest_sha256: &'a str,
    pub snapshot_file_count: u32,
    pub snapshot_total_bytes: u64,
    pub runtime_revision: &'a str,
    pub container_image_digest: &'a str,
    pub max_concurrent: u32,
    pub recipe_sha256: &'a str,
    pub recipe_path: &'a Path,
    pub snapshot_dir: &'a Path,
    pub sidecars: &'a BTreeMap<String, PathBuf>,
}

pub(crate) struct ManagedOpenAiRuntime {
    base_url: String,
    process_id: u32,
    _owner: ManagedContainerOwner,
}

impl std::fmt::Debug for ManagedOpenAiRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ManagedOpenAiRuntime")
            .field("base_url", &self.base_url)
            .field("lifecycle", &"managed")
            .finish()
    }
}

impl ManagedOpenAiRuntime {
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    pub(crate) fn process_id(&self) -> u32 {
        self.process_id
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedContainerState {
    schema_version: u32,
    nonce: String,
    container_name: String,
    container_id: Option<String>,
    labels: BTreeMap<String, String>,
}

struct ManagedContainerOwner {
    docker: PathBuf,
    state_path: PathBuf,
    container_id: String,
    labels: BTreeMap<String, String>,
    lock: File,
    stopped: Mutex<bool>,
}

impl std::fmt::Debug for ManagedContainerOwner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ManagedContainerOwner")
            .field("container_id", &self.container_id)
            .finish_non_exhaustive()
    }
}

impl Drop for ManagedContainerOwner {
    fn drop(&mut self) {
        let Ok(mut stopped) = self.stopped.lock() else {
            return;
        };
        if *stopped {
            return;
        }
        *stopped = true;
        let mut removed = false;
        if container_has_exact_labels(&self.docker, &self.container_id, &self.labels)
            .unwrap_or(false)
        {
            let stopped_ok = Command::new(&self.docker)
                .args(["stop", "--time", "180", &self.container_id])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if stopped_ok
                && container_has_exact_labels(&self.docker, &self.container_id, &self.labels)
                    .unwrap_or(false)
            {
                removed = Command::new(&self.docker)
                    .args(["rm", &self.container_id])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success());
            }
        }
        // Keep recovery identity when Docker is unavailable or cleanup cannot
        // be proven.  A later owner reconciles only this exact ID and labels.
        if removed {
            let _ = fs::remove_file(&self.state_path);
        }
        let _ = self.lock.unlock();
    }
}

pub(crate) fn prepare_managed_runtime(
    inputs: ManagedRuntimeInputs<'_>,
) -> Result<ManagedOpenAiRuntime> {
    platform_preflight()?;
    let recipe_bytes = fs::read(inputs.recipe_path)
        .with_context(|| format!("reading runtime recipe {}", inputs.recipe_path.display()))?;
    ensure!(
        sha256_bytes(&recipe_bytes) == inputs.recipe_sha256,
        "managed runtime recipe hash differs from its signed binding"
    );
    let recipe: ManagedRuntimeRecipeV1 =
        serde_json::from_slice(&recipe_bytes).context("parsing strict managed runtime recipe")?;
    let ManagedRuntimeRecipeV1::PennyroyalFlashNextFrspecV1 {
        schema_version,
        profile_version,
        public_model_id,
        artifact,
        source,
        proofs,
        ple,
        runtime,
    } = recipe;
    validate_recipe(
        &inputs,
        schema_version,
        profile_version,
        &public_model_id,
        &artifact,
        &source,
        &proofs,
        &ple,
        &runtime,
    )?;
    docker_preflight(inputs.docker, &runtime)?;

    let managed_root = inputs
        .home
        .join("runtime")
        .join("managed")
        .join("openai-compatible")
        .join(safe_component(inputs.enclave_id));
    fs::create_dir_all(&managed_root)
        .with_context(|| format!("creating managed runtime {}", managed_root.display()))?;
    set_private_directory(&managed_root)?;
    let lock_path = managed_root.join("lock");
    let lock = private_open(&lock_path)?;
    lock.try_lock_exclusive().with_context(|| {
        format!(
            "managed runtime for enclave {} is already owned by another process",
            inputs.enclave_id
        )
    })?;
    reconcile_previous_runtime(inputs.docker, &managed_root, &lock)?;

    let source_archive = inputs.sidecars.get(&source.sidecar).with_context(|| {
        format!(
            "runtime recipe source sidecar {} was not downloaded",
            source.sidecar
        )
    })?;
    verify_file(source_archive, source.archive_bytes, &source.archive_sha256)?;
    aggregate_disk_preflight(
        &managed_root,
        source.archive_bytes,
        ple.reader_wheel.bytes,
        ple.table_bytes,
    )?;
    let source_dir = materialize_source_archive(&managed_root, source_archive, &source)?;
    verify_source_proofs(&source_dir, inputs.snapshot_dir, &proofs, &ple)?;
    let seccomp_sidecar = inputs
        .sidecars
        .get(&runtime.security_profile.sidecar)
        .context("typed runtime seccomp sidecar was not downloaded")?;
    let seccomp_path = write_seccomp(
        &managed_root,
        seccomp_sidecar,
        &runtime.security_profile,
        &proofs.seccomp_sha256,
    )?;
    verify_source_git(
        inputs.docker,
        &managed_root,
        &source_dir,
        inputs.recipe_sha256,
        inputs.provider_id,
        inputs.enclave_id,
        inputs.runtime_revision,
    )?;
    let plugin_wheel = inputs
        .sidecars
        .get(&ple.reader_wheel.sidecar)
        .context("typed runtime PLE reader wheel sidecar was not downloaded")?;
    verify_file(
        plugin_wheel,
        ple.reader_wheel.bytes,
        &ple.reader_wheel.sha256,
    )?;
    let plugin_dir = materialize_plugin(
        inputs.docker,
        &managed_root,
        plugin_wheel,
        inputs.recipe_sha256,
        inputs.provider_id,
        inputs.enclave_id,
        &ple.reader_wheel,
        &ple.reader_version,
    )?;
    let prepared_model = materialize_ple(
        inputs.docker,
        &managed_root,
        inputs.snapshot_dir,
        &source_dir,
        &plugin_dir,
        inputs.recipe_sha256,
        inputs.provider_id,
        inputs.enclave_id,
        &ple,
    )?;

    let port = reserve_loopback_port()?;
    let wrapper = write_launch_wrapper(&managed_root, inputs.public_model_id, port)?;
    let nonce = random_hex(16)?;
    let home_hash = sha256_bytes(inputs.home.as_os_str().as_encoded_bytes());
    let labels = BTreeMap::from([
        ("mayhem.managed".to_owned(), "true".to_owned()),
        ("mayhem.owner-home".to_owned(), home_hash),
        ("mayhem.provider".to_owned(), inputs.provider_id.to_owned()),
        ("mayhem.enclave".to_owned(), inputs.enclave_id.to_owned()),
        ("mayhem.recipe".to_owned(), inputs.recipe_sha256.to_owned()),
        ("mayhem.nonce".to_owned(), nonce.clone()),
    ]);
    let container_name = format!(
        "mayhem-openai-{}-{}",
        safe_component(inputs.enclave_id),
        &nonce[..12]
    );
    let state_path = managed_root.join("state.json");
    let mut state = ManagedContainerState {
        schema_version: 1,
        nonce: nonce.clone(),
        container_name: container_name.clone(),
        container_id: None,
        labels: labels.clone(),
    };
    atomic_write_private_json(&state_path, &state)?;
    let create_args = service_create_args(ServiceCreateInputs {
        name: &container_name,
        port,
        model_id: inputs.public_model_id,
        snapshot: inputs.snapshot_dir,
        prepared_model: &prepared_model,
        source: &source_dir,
        plugin: &plugin_dir,
        managed_root: &managed_root,
        seccomp: &seccomp_path,
        wrapper: &wrapper,
        labels: &labels,
    })?;
    let output = run_docker(inputs.docker, &create_args)?;
    let container_id = String::from_utf8(output.stdout)
        .context("Docker returned a non-UTF8 container ID")?
        .trim()
        .to_owned();
    ensure!(
        is_hex(&container_id, 12, 64),
        "Docker returned an invalid managed container ID"
    );
    ensure!(
        container_has_exact_labels(inputs.docker, &container_id, &labels)?,
        "created managed container labels do not match Core ownership"
    );
    state.container_id = Some(container_id.clone());
    atomic_write_private_json(&state_path, &state)?;
    if let Err(error) = run_docker(inputs.docker, &["start".to_owned(), container_id.clone()]) {
        if container_has_exact_labels(inputs.docker, &container_id, &labels).unwrap_or(false) {
            let removed = Command::new(inputs.docker)
                .args(["rm", &container_id])
                .status()
                .is_ok_and(|status| status.success());
            if removed {
                fs::remove_file(&state_path)?;
            }
        }
        return Err(error).context("starting managed OpenAI-compatible runtime");
    }
    let owner = ManagedContainerOwner {
        docker: inputs.docker.to_path_buf(),
        state_path,
        container_id: container_id.clone(),
        labels: labels.clone(),
        lock,
        stopped: Mutex::new(false),
    };
    let process_id = inspect_running_container_pid(inputs.docker, &container_id, &labels)
        .context("resolving the owned managed runtime host PID")?;
    Ok(ManagedOpenAiRuntime {
        base_url: format!("http://127.0.0.1:{port}/"),
        process_id,
        _owner: owner,
    })
}

fn inspect_running_container_pid(
    docker: &Path,
    container_id: &str,
    labels: &BTreeMap<String, String>,
) -> Result<u32> {
    ensure!(
        container_has_exact_labels(docker, container_id, labels)?,
        "managed runtime labels changed before PID inspection"
    );
    let output = run_docker(
        docker,
        &[
            "inspect".to_owned(),
            "--format".to_owned(),
            "{{.State.Running}} {{.State.Pid}}".to_owned(),
            container_id.to_owned(),
        ],
    )?;
    let text = String::from_utf8(output.stdout).context("Docker returned a non-UTF8 state")?;
    let mut fields = text.split_whitespace();
    ensure!(
        fields.next() == Some("true"),
        "managed runtime is not running"
    );
    let pid = fields
        .next()
        .context("Docker omitted the managed runtime PID")?
        .parse::<u32>()
        .context("Docker returned an invalid managed runtime PID")?;
    ensure!(
        pid > 0 && fields.next().is_none(),
        "Docker returned an invalid managed runtime state"
    );
    Ok(pid)
}

#[allow(clippy::too_many_arguments)]
fn validate_recipe(
    inputs: &ManagedRuntimeInputs<'_>,
    schema_version: u32,
    profile_version: u32,
    public_model_id: &str,
    artifact: &RecipeArtifact,
    source: &RecipeSource,
    proofs: &RecipeProofs,
    ple: &RecipePle,
    runtime: &RecipeRuntime,
) -> Result<()> {
    ensure!(
        schema_version == 1 && profile_version == 1,
        "unsupported managed runtime recipe version"
    );
    ensure!(
        public_model_id == inputs.public_model_id,
        "runtime recipe public model ID differs from the selected catalog model"
    );
    ensure!(
        artifact.repo == inputs.artifact_repo && artifact.revision == inputs.artifact_revision,
        "runtime recipe artifact identity differs from the selected catalog artifact"
    );
    ensure!(
        artifact.snapshot_manifest_sidecar == inputs.snapshot_manifest_sidecar
            && artifact.snapshot_manifest_sha256 == inputs.snapshot_manifest_sha256,
        "runtime recipe snapshot manifest differs from the signed runtime binding"
    );
    ensure!(
        artifact.file_count == inputs.snapshot_file_count
            && artifact.total_bytes == inputs.snapshot_total_bytes,
        "runtime recipe snapshot dimensions differ from the signed snapshot"
    );
    ensure!(
        source.format == SOURCE_FORMAT && source.root_layout == SOURCE_LAYOUT,
        "runtime recipe source archive profile is unsupported"
    );
    ensure!(
        source.revision == inputs.runtime_revision,
        "runtime recipe source revision differs from the signed runtime revision"
    );
    ensure!(
        safe_sidecar_name(&source.sidecar),
        "runtime recipe has an invalid source sidecar name"
    );
    ensure!(
        is_lower_sha(&source.archive_sha256),
        "runtime recipe has an invalid source archive SHA-256"
    );
    ensure!(
        source.archive_bytes > 0,
        "runtime recipe source archive size must be positive"
    );
    ensure!(
        proofs.seccomp_sha256 == SECCOMP_SHA256 && sha256_bytes(SECCOMP) == SECCOMP_SHA256,
        "runtime recipe seccomp profile is not the Core-embedded profile"
    );
    for digest in [
        &proofs.launcher_sha256,
        &proofs.chat_template_sha256,
        &proofs.frspec_map_sha256,
        &proofs.frspec_manifest_sha256,
        &proofs.tokenizer_sha256,
        &proofs.ple_plugin_source_inventory_sha256,
        &ple.source_config_sha256,
        &ple.source_index_sha256,
        &ple.table_sha256,
        &ple.portable_manifest_sha256,
    ] {
        ensure!(
            is_lower_sha(digest),
            "runtime recipe contains an invalid SHA-256 proof"
        );
    }
    ensure!(
        ple.materialization == MATERIALIZATION,
        "runtime recipe PLE materialization is unsupported"
    );
    ensure!(
        ple.table_relpath == "ple/layer-0.bin" && ple.table_bytes > 0,
        "runtime recipe PLE table profile is unsupported"
    );
    ensure!(
        ple.table_rows == 320_001_536
            && ple.table_columns == 160
            && ple.table_dtype == "float8_e4m3fn",
        "runtime recipe PLE tensor shape/dtype is unsupported"
    );
    ensure!(
        ple.portable_manifest_bytes > 0 && ple.reader_version == "0.2.0+pennyroyal2",
        "runtime recipe PLE reader profile is unsupported"
    );
    validate_reader_wheel(&ple.reader_wheel, &ple.reader_version)?;
    ensure!(
        runtime.image == IMAGE
            && inputs.container_image_digest
                == IMAGE
                    .split_once('@')
                    .map(|(_, digest)| digest)
                    .unwrap_or(""),
        "runtime recipe image differs from the compiled digest-only image profile"
    );
    ensure!(
        runtime.security_profile.kind == SECURITY_PROFILE
            && runtime.security_profile.sidecar == "seccomp_profile"
            && runtime.security_profile.bytes == SECCOMP.len() as u64
            && runtime.security_profile.sha256 == SECCOMP_SHA256
            && runtime.resource_profile == RESOURCE_PROFILE
            && runtime.launch_profile == LAUNCH_PROFILE
            && runtime.deterministic_inference
            && runtime.attention_backend == "triton"
            && runtime.linear_attn_prefill_backend == "triton"
            && runtime.linear_attn_decode_backend == "flashinfer",
        "runtime recipe selects an unsupported launch/security/resource profile"
    );
    ensure!(
        runtime.provider_max_concurrent == inputs.max_concurrent
            && runtime.provider_max_concurrent == 2
            && runtime.scheduler_max_running_requests == 4,
        "runtime recipe concurrency differs from the proven two-request provider envelope"
    );
    ensure!(
        runtime.reasoning_default == "xhigh"
            && runtime.reasoning_overrides == ["low", "medium", "xhigh"],
        "runtime recipe reasoning controls are unsupported"
    );
    Ok(())
}

fn validate_reader_wheel(wheel: &RecipeReaderWheel, version: &str) -> Result<()> {
    ensure!(
        wheel.sidecar == "ple_plugin_wheel",
        "runtime recipe selects an unsupported PLE reader wheel sidecar"
    );
    ensure!(
        wheel.bytes == PLE_READER_WHEEL_BYTES && wheel.sha256 == PLE_READER_WHEEL_SHA256,
        "runtime recipe PLE reader wheel differs from the qualified fixed artifact"
    );
    ensure!(
        wheel.python_tag == "cp312"
            && wheel.abi_tag == "cp312"
            && matches!(
                wheel.platform_tag.as_str(),
                "linux_x86_64" | "manylinux_2_28_x86_64"
            ),
        "runtime recipe PLE reader wheel is not compatible with the fixed CPython 3.12 Linux x86_64 image"
    );
    let expected = format!(
        "sglang_ssd_stream-{version}-{}-{}-{}.whl",
        wheel.python_tag, wheel.abi_tag, wheel.platform_tag
    );
    ensure!(
        wheel.filename == expected && wheel.filename == PLE_READER_WHEEL_FILENAME,
        "runtime recipe PLE reader wheel filename does not match its signed version/tags"
    );
    Ok(())
}

fn platform_preflight() -> Result<()> {
    ensure!(
        cfg!(target_os = "linux") && cfg!(target_arch = "x86_64"),
        "managed Pennyroyal runtime requires Linux x86_64"
    );
    Ok(())
}

fn docker_preflight(docker: &Path, runtime: &RecipeRuntime) -> Result<()> {
    let version = docker_text(docker, &["version", "--format", "{{.Server.Version}}"])?;
    ensure!(
        version.trim() == "29.1.3",
        "managed runtime security profile requires Docker server 29.1.3"
    );
    let runtimes = docker_text(docker, &["info", "--format", "{{json .Runtimes}}"])?;
    let runtimes: serde_json::Value =
        serde_json::from_str(runtimes.trim()).context("parsing Docker runtime inventory")?;
    ensure!(
        runtimes.get("nvidia").is_some(),
        "managed runtime requires Docker's NVIDIA runtime"
    );
    let cgroup = docker_text(
        docker,
        &["info", "--format", "{{.CgroupVersion}} {{.CgroupDriver}}"],
    )?;
    ensure!(
        cgroup.split_whitespace().eq(["2", "systemd"]),
        "managed runtime resource profile requires Docker cgroup v2 with the systemd driver"
    );
    let limit_support = docker_text(
        docker,
        &[
            "info",
            "--format",
            "{{json .MemoryLimit}} {{json .SwapLimit}}",
        ],
    )?;
    ensure!(
        limit_support.split_whitespace().eq(["true", "true"]),
        "managed runtime requires Docker cgroup memory and swap limit support"
    );
    run_docker(docker, &["pull".to_owned(), runtime.image.clone()])
        .context("pulling exact managed runtime image")?;
    let digests = docker_text(
        docker,
        &[
            "image",
            "inspect",
            "--format",
            "{{json .RepoDigests}}",
            &runtime.image,
        ],
    )?;
    let digests: Vec<String> =
        serde_json::from_str(digests.trim()).context("parsing Docker image RepoDigests")?;
    ensure!(
        digests.iter().any(|digest| digest == &runtime.image),
        "Docker image does not expose the signed repository digest"
    );
    Ok(())
}

fn materialize_source_archive(
    root: &Path,
    archive: &Path,
    source: &RecipeSource,
) -> Result<PathBuf> {
    let destination = root.join(format!("source-{}", &source.archive_sha256[..16]));
    if destination.is_dir() {
        return Ok(destination);
    }
    ensure!(
        !destination.exists(),
        "managed source cache exists with the wrong type"
    );
    let staging = root.join(format!(
        ".source-{}.partial-{}",
        &source.archive_sha256[..16],
        random_hex(8)?
    ));
    fs::create_dir(&staging).with_context(|| format!("creating {}", staging.display()))?;
    let result = (|| -> Result<()> {
        let file = File::open(archive).with_context(|| format!("opening {}", archive.display()))?;
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        let mut seen = BTreeSet::new();
        let mut directories = Vec::new();
        for entry in tar.entries().context("reading runtime source archive")? {
            let mut entry = entry.context("reading runtime source archive entry")?;
            let path = entry
                .path()
                .context("reading runtime source archive path")?
                .into_owned();
            let relative = source_bundle_relative(&path)?;
            if relative.as_os_str().is_empty() {
                continue;
            }
            ensure!(
                seen.insert(relative.clone()),
                "runtime source archive contains a duplicate path"
            );
            let destination_path = staging.join(&relative);
            let kind = entry.header().entry_type();
            if kind.is_dir() {
                fs::create_dir_all(&destination_path)?;
                directories.push(destination_path);
            } else if kind.is_file() {
                if let Some(parent) = destination_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut output = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&destination_path)?;
                std::io::copy(&mut entry, &mut output)?;
                output.sync_all()?;
                set_archive_mode(&destination_path, entry.header().mode().unwrap_or(0))?;
            } else if kind.is_symlink() {
                let target = entry
                    .link_name()
                    .context("reading runtime source symlink")?
                    .context("runtime source symlink has no target")?
                    .into_owned();
                validate_archive_symlink(&relative, &target)?;
                if let Some(parent) = destination_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                create_symlink(&target, &destination_path)?;
            } else {
                bail!("runtime source archive contains a hardlink or special file");
            }
        }
        ensure!(!seen.is_empty(), "runtime source archive is empty");
        directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        for directory in directories {
            set_directory_readonly(&directory)?;
        }
        fs::rename(&staging, &destination).context("atomically installing runtime source")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result?;
    Ok(destination)
}

fn source_bundle_relative(path: &Path) -> Result<PathBuf> {
    let safe = safe_relative(path)?;
    let mut components = safe.components();
    ensure!(
        components.next().and_then(|part| part.as_os_str().to_str()) == Some(SOURCE_LAYOUT),
        "runtime source archive entry is outside its fixed top-level source directory"
    );
    Ok(components.collect())
}

fn validate_archive_symlink(path: &Path, target: &Path) -> Result<()> {
    ensure!(
        !target.as_os_str().is_empty() && !target.is_absolute(),
        "runtime source symlink target must be relative"
    );
    let mut depth = path
        .parent()
        .map_or(0usize, |parent| parent.components().count());
    for component in target.components() {
        match component {
            Component::Normal(_) => depth = depth.saturating_add(1),
            Component::ParentDir if depth > 0 => depth -= 1,
            _ => bail!("runtime source symlink target escapes the extracted source"),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &Path, path: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, path).map_err(Into::into)
}

#[cfg(not(unix))]
fn create_symlink(_target: &Path, _path: &Path) -> Result<()> {
    bail!("managed runtime source symlinks require a Unix host")
}

fn set_archive_mode(path: &Path, archive_mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = if archive_mode & 0o111 != 0 {
            0o555
        } else {
            0o444
        };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

fn set_directory_readonly(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o555))?;
    }
    Ok(())
}

fn verify_source_proofs(
    source: &Path,
    snapshot: &Path,
    proofs: &RecipeProofs,
    ple: &RecipePle,
) -> Result<()> {
    for (relative, expected) in [
        (LAUNCHER, proofs.launcher_sha256.as_str()),
        (CHAT_TEMPLATE, proofs.chat_template_sha256.as_str()),
        (FRSPEC_MAP, proofs.frspec_map_sha256.as_str()),
        (FRSPEC_MANIFEST, proofs.frspec_manifest_sha256.as_str()),
    ] {
        verify_file_sha(&source.join(relative), expected)?;
    }
    verify_file_sha(&snapshot.join("tokenizer.json"), &proofs.tokenizer_sha256)?;
    verify_file_sha(&snapshot.join("config.json"), &ple.source_config_sha256)?;
    verify_file_sha(
        &snapshot.join("model.safetensors.index.json"),
        &ple.source_index_sha256,
    )?;
    ensure!(
        source.join(NIXL_CONFIG).is_file()
            && source.join(PLE_PREPARER).is_file()
            && source.join(PLE_CHECKER).is_file()
            && source.join(PLE_PLUGIN_SOURCE).is_dir(),
        "runtime source archive is missing fixed profile inputs"
    );
    ensure!(
        source_inventory_sha256(&source.join(PLE_PLUGIN_SOURCE))?
            == proofs.ple_plugin_source_inventory_sha256,
        "runtime PLE plugin source inventory differs from the signed recipe"
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn verify_source_git(
    docker: &Path,
    root: &Path,
    source: &Path,
    recipe: &str,
    provider: &str,
    enclave: &str,
    revision: &str,
) -> Result<()> {
    let common = [
        "-c",
        "safe.directory=/mayhem/source",
        "-C",
        "/mayhem/source",
    ];
    let mut command = vec!["git".to_owned()];
    command.extend(common.iter().map(|value| (*value).to_owned()));
    command.extend(["rev-parse".to_owned(), "HEAD".to_owned()]);
    let output = run_owned_one_shot_capture(
        docker,
        root,
        "source-head",
        recipe,
        provider,
        enclave,
        &[(source, "/mayhem/source", true)],
        &[],
        &command,
    )?;
    ensure!(
        String::from_utf8_lossy(&output.stdout).trim() == revision,
        "managed runtime source bundle HEAD differs from the signed revision"
    );
    let mut command = vec!["git".to_owned()];
    command.extend(common.iter().map(|value| (*value).to_owned()));
    command.extend([
        "status".to_owned(),
        "--porcelain".to_owned(),
        "--untracked-files=all".to_owned(),
    ]);
    let output = run_owned_one_shot_capture(
        docker,
        root,
        "source-clean",
        recipe,
        provider,
        enclave,
        &[(source, "/mayhem/source", true)],
        &[],
        &command,
    )?;
    ensure!(
        output.stdout.iter().all(u8::is_ascii_whitespace),
        "managed runtime source bundle does not match its tracked HEAD"
    );
    Ok(())
}

fn write_seccomp(
    root: &Path,
    sidecar: &Path,
    profile: &RecipeSecurityProfile,
    expected: &str,
) -> Result<PathBuf> {
    ensure!(
        expected == SECCOMP_SHA256
            && profile.sha256 == SECCOMP_SHA256
            && profile.bytes == SECCOMP.len() as u64,
        "runtime recipe seccomp hash/size mismatch"
    );
    verify_file(sidecar, profile.bytes, &profile.sha256)?;
    ensure!(
        fs::read(sidecar)? == SECCOMP,
        "typed seccomp sidecar differs from the Core-compiled security profile"
    );
    let path = root.join(format!("seccomp-{SECCOMP_SHA256}.json"));
    if path.exists() {
        verify_file(&path, SECCOMP.len() as u64, SECCOMP_SHA256)?;
        return Ok(path);
    }
    atomic_write_private(&path, &fs::read(sidecar)?)?;
    Ok(path)
}

fn aggregate_disk_preflight(
    root: &Path,
    source_bytes: u64,
    wheel_bytes: u64,
    table_bytes: u64,
) -> Result<()> {
    const CACHE_RESERVE: u64 = 64 * 1024 * 1024 * 1024;
    let required = source_bytes
        .checked_add(wheel_bytes)
        .and_then(|bytes| bytes.checked_add(table_bytes))
        .and_then(|bytes| bytes.checked_add(CACHE_RESERVE))
        .context("managed runtime disk requirement overflowed u64")?;
    let available = fs2::available_space(root)
        .with_context(|| format!("checking free space under {}", root.display()))?;
    ensure!(
        available >= required,
        "managed runtime requires {required} free bytes for source, PLE derivation, and cache reserve; only {available} are available"
    );
    Ok(())
}

fn source_inventory_sha256(root: &Path) -> Result<String> {
    let mut entries = Vec::<PathBuf>::new();
    collect_inventory_entries(root, root, &mut entries)?;
    entries.sort_by(|left, right| path_to_posix(left).cmp(&path_to_posix(right)));
    let mut digest = Sha256::new();
    for relative in entries {
        let path = root.join(&relative);
        let metadata = fs::symlink_metadata(&path)?;
        let relative = path_to_posix(&relative);
        let row = if metadata.file_type().is_file() {
            format!(
                "{{\"bytes\":{},\"path\":{},\"sha256\":{},\"type\":\"file\"}}\n",
                metadata.len(),
                serde_json::to_string(&relative)?,
                serde_json::to_string(&file_sha256(&path)?)?,
            )
        } else if metadata.file_type().is_dir() {
            format!(
                "{{\"path\":{},\"type\":\"directory\"}}\n",
                serde_json::to_string(&relative)?,
            )
        } else if metadata.file_type().is_symlink() {
            let target = fs::read_link(&path)?;
            format!(
                "{{\"path\":{},\"target\":{},\"type\":\"symlink\"}}\n",
                serde_json::to_string(&relative)?,
                serde_json::to_string(&target.to_string_lossy())?,
            )
        } else {
            bail!("plugin source inventory contains a special file")
        };
        digest.update(row.as_bytes());
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn collect_inventory_entries(
    root: &Path,
    directory: &Path,
    entries: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root)?.to_path_buf();
        entries.push(relative);
        if fs::symlink_metadata(&path)?.file_type().is_dir() {
            collect_inventory_entries(root, &path, entries)?;
        }
    }
    Ok(())
}

fn path_to_posix(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn materialize_plugin(
    docker: &Path,
    root: &Path,
    wheel_path: &Path,
    recipe_sha: &str,
    provider: &str,
    enclave: &str,
    wheel: &RecipeReaderWheel,
    version: &str,
) -> Result<PathBuf> {
    let destination = root.join(format!(
        "plugin-{}-{}",
        safe_component(version),
        &wheel.sha256[..16]
    ));
    if destination.exists() {
        ensure!(
            fs::symlink_metadata(&destination)?.file_type().is_dir(),
            "managed plugin cache has an unsafe type"
        );
        fs::remove_dir_all(&destination)
            .context("removing the prior Core-owned PLE plugin cache")?;
    }
    let staging = root.join(format!(".plugin.partial-{}", random_hex(8)?));
    fs::create_dir(&staging)?;
    let wheel_container_path = format!("/mayhem/wheel/{}", wheel.filename);
    let command = plugin_install_command(&wheel_container_path);
    let result = (|| -> Result<()> {
        run_owned_one_shot_capture(
            docker,
            root,
            "plugin",
            recipe_sha,
            provider,
            enclave,
            &[
                (wheel_path, wheel_container_path.as_str(), true),
                (&staging, "/mayhem/plugin", false),
            ],
            &[],
            &command,
        )?;
        ensure!(
            plugin_version(&staging).as_deref() == Some(version),
            "managed PLE plugin wheel installed the wrong version"
        );
        fs::rename(&staging, &destination).context("atomically installing managed PLE plugin")?;
        Ok(())
    })();
    if result.is_err() && !one_shot_state_exists(root, "plugin")? {
        let _ = fs::remove_dir_all(&staging);
    }
    result?;
    Ok(destination)
}

fn plugin_install_command(wheel_container_path: &str) -> Vec<String> {
    vec![
        "uv".to_owned(),
        "pip".to_owned(),
        "install".to_owned(),
        "--offline".to_owned(),
        "--no-cache".to_owned(),
        "--python".to_owned(),
        "/usr/bin/python3".to_owned(),
        "--target".to_owned(),
        "/mayhem/plugin".to_owned(),
        "--no-deps".to_owned(),
        wheel_container_path.to_owned(),
    ]
}

fn materialize_ple(
    docker: &Path,
    root: &Path,
    snapshot: &Path,
    source: &Path,
    plugin: &Path,
    recipe_sha: &str,
    provider: &str,
    enclave: &str,
    ple: &RecipePle,
) -> Result<PathBuf> {
    let destination = root.join(format!(
        "model-nvme-ple-{}-{}",
        &recipe_sha[..16],
        &ple.portable_manifest_sha256[..16]
    ));
    if verify_prepared_ple(&destination, ple).is_ok() {
        verify_ple_with_checker(
            docker,
            root,
            snapshot,
            source,
            plugin,
            &destination,
            recipe_sha,
            provider,
            enclave,
            ple,
        )?;
        return Ok(destination);
    }
    ensure!(
        !destination.exists(),
        "managed PLE cache failed signed validation"
    );
    let staging = root.join(format!(".ple.partial-{}", random_hex(8)?));
    fs::create_dir(&staging)?;
    let command = vec![
        "/usr/bin/python3".to_owned(),
        format!("/mayhem/source/{PLE_PREPARER}"),
        "--source".to_owned(),
        "/mayhem/model-source".to_owned(),
        "--output".to_owned(),
        "/mayhem/materialize/model-nvme-ple".to_owned(),
    ];
    let result = (|| -> Result<()> {
        run_owned_one_shot(
            docker,
            root,
            "ple",
            recipe_sha,
            provider,
            enclave,
            &[
                (snapshot, "/mayhem/model-source", true),
                (source, "/mayhem/source", true),
                (&staging, "/mayhem/materialize", false),
            ],
            &command,
        )?;
        let output = staging.join("model-nvme-ple");
        verify_prepared_ple(&output, ple)?;
        verify_ple_with_checker(
            docker, root, snapshot, source, plugin, &output, recipe_sha, provider, enclave, ple,
        )?;
        fs::rename(&output, &destination).context("atomically installing managed PLE overlay")?;
        let _ = fs::remove_dir(&staging);
        Ok(())
    })();
    if result.is_err()
        && !one_shot_state_exists(root, "ple")?
        && !one_shot_state_exists(root, "ple-check")?
    {
        let _ = fs::remove_dir_all(&staging);
    }
    result?;
    Ok(destination)
}

#[allow(clippy::too_many_arguments)]
fn verify_ple_with_checker(
    docker: &Path,
    root: &Path,
    snapshot: &Path,
    source: &Path,
    plugin: &Path,
    prepared: &Path,
    recipe_sha: &str,
    provider: &str,
    enclave: &str,
    ple: &RecipePle,
) -> Result<()> {
    let check = vec![
        "/usr/bin/python3".to_owned(),
        format!("/mayhem/source/{PLE_CHECKER}"),
        "--source".to_owned(),
        "/mayhem/model-source".to_owned(),
        "--prepared".to_owned(),
        "/mayhem/model".to_owned(),
    ];
    let checked = run_owned_one_shot_capture(
        docker,
        root,
        "ple-check",
        recipe_sha,
        provider,
        enclave,
        &[
            (snapshot, "/mayhem/model-source", true),
            (source, "/mayhem/source", true),
            (prepared, "/mayhem/model", true),
            (plugin, "/mayhem/plugin", true),
        ],
        &[("PYTHONPATH", "/mayhem/plugin:/mayhem/source/python")],
        &check,
    )?;
    ensure!(
        String::from_utf8_lossy(&checked.stdout).trim() == ple.portable_manifest_sha256,
        "managed PLE checker returned a different identity"
    );
    Ok(())
}

fn verify_prepared_ple(path: &Path, ple: &RecipePle) -> Result<()> {
    ensure!(path.is_dir(), "prepared PLE overlay is missing");
    verify_file(
        &path.join(&ple.table_relpath),
        ple.table_bytes,
        &ple.table_sha256,
    )?;
    verify_file(
        &path.join("ssd-stream.json"),
        ple.portable_manifest_bytes,
        &ple.portable_manifest_sha256,
    )?;
    Ok(())
}

fn plugin_version(root: &Path) -> Option<String> {
    let entries = fs::read_dir(root).ok()?;
    let mut versions = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_str()?;
        if name.starts_with("sglang_ssd_stream-") && name.ends_with(".dist-info") {
            let metadata = fs::read_to_string(entry.path().join("METADATA")).ok()?;
            versions.extend(
                metadata
                    .lines()
                    .filter_map(|line| line.strip_prefix("Version: "))
                    .map(str::to_owned),
            );
        }
    }
    (versions.len() == 1 && root.join("sglang_ssd_stream/plugin.py").is_file())
        .then(|| versions.remove(0))
}

fn write_launch_wrapper(root: &Path, model_id: &str, port: u16) -> Result<PathBuf> {
    let model = serde_json::to_string(model_id)?;
    let expected = serde_json::to_string(&qualified_launcher_args())?;
    let effective = serde_json::to_string(&effective_launcher_args(model_id, port))?;
    let script = format!(
        r#"#!/usr/bin/python3
import os
import sys

args = sys.argv[1:]
expected = {expected}
effective = {effective}
if args != expected:
    raise SystemExit("qualified launcher argument vector mismatch")

def replace(flag, old, new):
    if args.count(flag) != 1:
        raise SystemExit("qualified launcher flag mismatch: " + flag)
    index = args.index(flag) + 1
    if index >= len(args) or args[index] != old:
        raise SystemExit("qualified launcher value mismatch: " + flag)
    args[index] = new

replace("--host", "0.0.0.0", "127.0.0.1")
replace("--port", "8001", "{port}")
replace("--served-model-name", "pennyroyal", {model})
replace("--default-chat-template-kwargs",
        '{{"enable_thinking":true,"preserve_thinking":true,"reasoning_effort":"medium"}}',
        '{{"enable_thinking":true,"preserve_thinking":true,"reasoning_effort":"xhigh"}}')
replace("--linear-attn-prefill-backend", "flashinfer", "triton")
args.extend(["--attention-backend", "triton"])
args.append("--enable-deterministic-inference")
if args != effective:
    raise SystemExit("deterministic launcher argument vector mismatch")
os.execv("/usr/local/bin/sglang", ["sglang", *args])
"#,
    );
    let digest = sha256_bytes(script.as_bytes());
    let path = root.join(format!("sglang-wrapper-{digest}"));
    if path.exists() {
        verify_file(&path, script.len() as u64, &digest)?;
        return Ok(path);
    }
    atomic_write_private(&path, script.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o500))?;
    }
    Ok(path)
}

fn effective_launcher_args(model_id: &str, port: u16) -> Vec<String> {
    let mut args = qualified_launcher_args()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for (flag, old, new) in [
        ("--host", "0.0.0.0", "127.0.0.1".to_owned()),
        ("--port", "8001", port.to_string()),
        ("--served-model-name", "pennyroyal", model_id.to_owned()),
        (
            "--default-chat-template-kwargs",
            r#"{"enable_thinking":true,"preserve_thinking":true,"reasoning_effort":"medium"}"#,
            r#"{"enable_thinking":true,"preserve_thinking":true,"reasoning_effort":"xhigh"}"#
                .to_owned(),
        ),
        (
            "--linear-attn-prefill-backend",
            "flashinfer",
            "triton".to_owned(),
        ),
    ] {
        let positions = args
            .iter()
            .enumerate()
            .filter_map(|(index, value)| (value == flag).then_some(index))
            .collect::<Vec<_>>();
        assert_eq!(
            positions.len(),
            1,
            "qualified launcher flag mismatch: {flag}"
        );
        let value = positions[0] + 1;
        assert_eq!(args.get(value).map(String::as_str), Some(old));
        args[value] = new;
    }
    args.extend(["--attention-backend".to_owned(), "triton".to_owned()]);
    args.push("--enable-deterministic-inference".to_owned());
    args
}

fn qualified_launcher_args() -> Vec<&'static str> {
    vec![
        "serve",
        "--model-path",
        "/mayhem/model",
        "--load-format",
        "safetensors",
        "--served-model-name",
        "pennyroyal",
        "--host",
        "0.0.0.0",
        "--port",
        "8001",
        "--tp",
        "1",
        "--dtype",
        "bfloat16",
        "--quantization",
        "modelopt_fp4",
        "--kv-cache-dtype",
        "fp8_e4m3",
        "--mem-fraction-static",
        "0.981",
        "--max-total-tokens",
        "824384",
        "--warmups=structured_output",
        "--context-length",
        "524288",
        "--json-model-override-args",
        r#"{"text_config":{"rope_parameters":{"mrope_interleaved":true,"mrope_section":[11,11,10],"rope_type":"yarn","rope_theta":10000000,"partial_rotary_factor":0.25,"factor":2.0,"original_max_position_embeddings":262144}}}"#,
        "--page-size",
        "64",
        "--max-running-requests",
        "4",
        "--sleep-on-idle",
        "--chunked-prefill-size",
        "4096",
        "--mamba-radix-cache-strategy",
        "extra_buffer",
        "--mamba-ssm-dtype",
        "bfloat16",
        "--max-mamba-cache-size",
        "24",
        "--gdn-mtp-cache-mode",
        "none",
        "--linear-attn-decode-backend",
        "flashinfer",
        "--linear-attn-prefill-backend",
        "flashinfer",
        "--mamba-track-interval",
        "64",
        "--enable-hierarchical-cache",
        "--hicache-size",
        "32",
        "--hicache-host-memory-mode",
        "cache",
        "--hicache-write-policy",
        "write_through",
        "--hicache-io-backend",
        "kernel",
        "--hicache-mem-layout",
        "page_first",
        "--hicache-storage-backend",
        "nixl",
        "--hicache-storage-prefetch-policy",
        "timeout",
        "--hicache-storage-backend-extra-config",
        "@/mayhem/source/configs/pennyroyal/nixl-posix-frspec.toml",
        "--trust-remote-code",
        "--chat-template",
        "/mayhem/source/configs/pennyroyal/templates/froggeric-v22.5.jinja",
        "--image-processor-backend",
        "pil",
        "--reasoning-parser",
        "qwen3",
        "--tool-call-parser",
        "qwen3_coder",
        "--enable-request-time-stats-logging",
        "--enable-metrics",
        "--default-chat-template-kwargs",
        r#"{"enable_thinking":true,"preserve_thinking":true,"reasoning_effort":"medium"}"#,
        "--speculative-algorithm",
        "NEXTN",
        "--speculative-num-steps",
        "3",
        "--speculative-eagle-topk",
        "1",
        "--speculative-num-draft-tokens",
        "4",
        "--speculative-draft-model-quantization",
        "unquant",
        "--speculative-token-map",
        "/mayhem/source/configs/pennyroyal/frspec/flash-next-64k.pt",
        "--watchdog-timeout",
        "1800",
    ]
}

struct ServiceCreateInputs<'a> {
    name: &'a str,
    port: u16,
    model_id: &'a str,
    snapshot: &'a Path,
    prepared_model: &'a Path,
    source: &'a Path,
    plugin: &'a Path,
    managed_root: &'a Path,
    seccomp: &'a Path,
    wrapper: &'a Path,
    labels: &'a BTreeMap<String, String>,
}

fn service_create_args(inputs: ServiceCreateInputs<'_>) -> Result<Vec<String>> {
    let cache = inputs.managed_root.join("cache");
    let nixl = inputs.managed_root.join("nixl");
    let runtime_home = inputs.managed_root.join("home");
    let tmp = inputs.managed_root.join("tmp");
    for path in [&cache, &nixl, &runtime_home, &tmp] {
        fs::create_dir_all(path)?;
    }
    let mut args = vec![
        "create".to_owned(),
        "--name".to_owned(),
        inputs.name.to_owned(),
        "--restart=no".to_owned(),
        "--runtime=nvidia".to_owned(),
        "--gpus=device=0".to_owned(),
        "--network=host".to_owned(),
        "--ipc=host".to_owned(),
        "--shm-size=32g".to_owned(),
        "--memory=104g".to_owned(),
        "--memory-swap=104g".to_owned(),
        "--pids-limit=32768".to_owned(),
        "--ulimit=memlock=-1:-1".to_owned(),
        "--security-opt=no-new-privileges:true".to_owned(),
        format!("--security-opt=seccomp={}", inputs.seccomp.display()),
        "--entrypoint=/usr/bin/bash".to_owned(),
    ];
    args.push(owned_container_user_arg(inputs.managed_root)?);
    for (name, value) in inputs.labels {
        args.push(format!("--label={name}={value}"));
    }
    for (host, container, read_only) in [
        (inputs.snapshot, "/mayhem/model-source", true),
        (inputs.prepared_model, "/mayhem/model", true),
        (inputs.source, "/mayhem/source", true),
        (inputs.plugin, "/mayhem/plugin", true),
        (&cache, "/mayhem/cache", false),
        (&nixl, "/mayhem/nixl", false),
        (&runtime_home, "/mayhem/home", false),
        (&tmp, "/mayhem/tmp", false),
        (inputs.wrapper, "/mayhem/bin/sglang-wrapper", true),
    ] {
        args.extend(["--mount".to_owned(), mount_arg(host, container, read_only)?]);
    }
    for (name, value) in [
        ("HOME", "/mayhem/home"),
        ("TMPDIR", "/mayhem/tmp"),
        ("REPO_ROOT", "/mayhem/source"),
        ("SGLANG_EXE", "/mayhem/bin/sglang-wrapper"),
        ("PYTHON", "/usr/bin/python3"),
        ("TARGET_MODEL", "/mayhem/model-source"),
        ("CACHE_BASE", "/mayhem/cache"),
        ("NIXL_STORAGE_BASE", "/mayhem/nixl"),
        ("PENNY_PLE_BACKEND", "nvme"),
        ("PENNY_PLE_NVME_MODEL", "/mayhem/model"),
        ("PENNY_PLE_PLUGIN_DIR", "/mayhem/plugin"),
        ("PYTHONPATH", "/mayhem/source/python"),
        ("SGLANG_SM120_ONLINE_MXFP8", "true"),
        ("SGLANG_MM_PREPROCESS_DEVICE", "cpu"),
        ("MAX_TOTAL_TOKENS", "824384"),
        ("CUDA_HOME", "/usr/local/cuda"),
        ("CC", "/usr/bin/gcc"),
        ("CXX", "/usr/bin/g++"),
        ("CUDAHOSTCXX", "/usr/bin/g++"),
        ("TORCH_CUDA_ARCH_LIST", "12.0"),
        ("PENNY_BUILD_JOBS", "2"),
        ("MAX_JOBS", "2"),
        ("CMAKE_BUILD_PARALLEL_LEVEL", "2"),
        ("CARGO_BUILD_JOBS", "2"),
        ("FLASHINFER_NINJA_JOBS", "2"),
        ("FLASHINFER_NVCC_THREADS", "1"),
        ("TORCHINDUCTOR_COMPILE_THREADS", "2"),
        ("OMP_NUM_THREADS", "4"),
        ("MKL_NUM_THREADS", "4"),
        ("CUDA_VISIBLE_DEVICES", "0"),
        ("NUMPY_MADVISE_HUGEPAGE", "0"),
    ] {
        args.extend(["--env".to_owned(), format!("{name}={value}")]);
    }
    args.push(IMAGE.to_owned());
    args.extend(service_command());
    let _ = (inputs.model_id, inputs.port);
    Ok(args)
}

fn service_command() -> Vec<String> {
    vec!["/mayhem/source/configs/pennyroyal/serve-flash-next-frspec.sh".to_owned()]
}

fn run_owned_one_shot(
    docker: &Path,
    root: &Path,
    purpose: &str,
    recipe: &str,
    provider: &str,
    enclave: &str,
    mounts: &[(&Path, &str, bool)],
    command: &[String],
) -> Result<()> {
    run_owned_one_shot_capture(
        docker,
        root,
        purpose,
        recipe,
        provider,
        enclave,
        mounts,
        &[],
        command,
    )
    .map(|_| ())
}

#[allow(clippy::too_many_arguments)]
fn run_owned_one_shot_capture(
    docker: &Path,
    root: &Path,
    purpose: &str,
    recipe: &str,
    provider: &str,
    enclave: &str,
    mounts: &[(&Path, &str, bool)],
    envs: &[(&str, &str)],
    command: &[String],
) -> Result<Output> {
    let nonce = random_hex(12)?;
    let container_name = format!("mayhem-{purpose}-{}", &nonce[..12]);
    let labels = BTreeMap::from([
        ("mayhem.managed".to_owned(), "true".to_owned()),
        ("mayhem.purpose".to_owned(), purpose.to_owned()),
        ("mayhem.provider".to_owned(), provider.to_owned()),
        ("mayhem.enclave".to_owned(), enclave.to_owned()),
        ("mayhem.recipe".to_owned(), recipe.to_owned()),
        ("mayhem.nonce".to_owned(), nonce.clone()),
    ]);
    let state_path = root.join(format!("oneshot-{purpose}-{nonce}.json"));
    let mut state = ManagedContainerState {
        schema_version: 1,
        nonce: nonce.clone(),
        container_name: container_name.clone(),
        container_id: None,
        labels: labels.clone(),
    };
    atomic_write_private_json(&state_path, &state)?;
    let mut args = vec![
        "create".to_owned(),
        "--name".to_owned(),
        container_name,
        "--restart=no".to_owned(),
        "--network=none".to_owned(),
        "--pids-limit=32768".to_owned(),
        "--security-opt=no-new-privileges:true".to_owned(),
        format!(
            "--security-opt=seccomp={}",
            root.join(format!("seccomp-{SECCOMP_SHA256}.json"))
                .display()
        ),
        format!(
            "--entrypoint={}",
            command
                .first()
                .context("managed one-shot command is empty")?
        ),
    ];
    args.extend(owned_container_identity_args(root)?);
    for (name, value) in &labels {
        args.push(format!("--label={name}={value}"));
    }
    for (host, container, read_only) in mounts {
        args.extend([
            "--mount".to_owned(),
            mount_arg(host, container, *read_only)?,
        ]);
    }
    for (name, value) in envs {
        args.extend(["--env".to_owned(), format!("{name}={value}")]);
    }
    args.push(IMAGE.to_owned());
    args.extend_from_slice(&command[1..]);
    let created = run_docker(docker, &args)?;
    let id = String::from_utf8(created.stdout)?.trim().to_owned();
    ensure!(
        is_hex(&id, 12, 64) && container_has_exact_labels(docker, &id, &labels)?,
        "Docker one-shot container ownership mismatch"
    );
    state.container_id = Some(id.clone());
    atomic_write_private_json(&state_path, &state)?;
    let output = Command::new(docker)
        .args(["start", "--attach", &id])
        .output()
        .context("running managed preparation container")?;
    ensure!(
        container_has_exact_labels(docker, &id, &labels)?,
        "managed one-shot container changed identity before cleanup"
    );
    let removed = Command::new(docker)
        .args(["rm", &id])
        .status()
        .context("removing managed one-shot container")?;
    ensure!(
        removed.success(),
        "could not remove managed one-shot container"
    );
    fs::remove_file(&state_path)?;
    ensure!(
        output.status.success(),
        "managed {purpose} container failed: {}",
        bounded(&output.stderr)
    );
    Ok(output)
}

#[cfg(unix)]
fn owned_container_user_arg(root: &Path) -> Result<String> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = fs::metadata(root)
        .with_context(|| format!("reading managed runtime owner for {}", root.display()))?;
    ensure!(
        metadata.is_dir(),
        "managed runtime owner path is not a directory"
    );
    Ok(format!("--user={}:{}", metadata.uid(), metadata.gid()))
}

#[cfg(not(unix))]
fn owned_container_user_arg(_root: &Path) -> Result<String> {
    bail!("managed containers require a Unix host")
}

fn owned_container_identity_args(root: &Path) -> Result<[String; 3]> {
    Ok([
        owned_container_user_arg(root)?,
        "--env=HOME=/tmp".to_owned(),
        "--env=TMPDIR=/tmp".to_owned(),
    ])
}

fn reconcile_previous_runtime(docker: &Path, root: &Path, _lock: &File) -> Result<()> {
    let mut states = fs::read_dir(root)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name == "state.json"
                        || (name.starts_with("oneshot-") && name.ends_with(".json"))
                })
        })
        .collect::<Vec<_>>();
    states.sort();
    for state_path in states {
        reconcile_state_path(docker, &state_path)?;
    }
    cleanup_abandoned_partial_directories(root)?;
    Ok(())
}

fn cleanup_abandoned_partial_directories(root: &Path) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let managed_partial = (name.starts_with(".source-") && name.contains(".partial-"))
            || name.starts_with(".plugin.partial-")
            || name.starts_with(".ple.partial-");
        if !managed_partial {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        ensure!(
            metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
            "managed partial cache {} has an unsafe type",
            entry.path().display()
        );
        fs::remove_dir_all(entry.path())
            .with_context(|| format!("removing abandoned managed partial {name}"))?;
    }
    File::open(root)?.sync_all()?;
    Ok(())
}

fn one_shot_state_exists(root: &Path, purpose: &str) -> Result<bool> {
    let prefix = format!("oneshot-{purpose}-");
    Ok(fs::read_dir(root)?
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".json"))
        }))
}

fn reconcile_state_path(docker: &Path, state_path: &Path) -> Result<()> {
    let state: ManagedContainerState = serde_json::from_slice(&fs::read(&state_path)?)
        .context("parsing prior managed runtime state")?;
    ensure!(
        state.schema_version == 1,
        "prior managed runtime state has an unsupported schema"
    );
    let inspected = inspect_container(
        docker,
        state
            .container_id
            .as_deref()
            .unwrap_or(&state.container_name),
    )?;
    let Some((id, labels)) = inspected else {
        fs::remove_file(state_path)?;
        return Ok(());
    };
    ensure!(
        state
            .labels
            .iter()
            .all(|(name, value)| labels.get(name) == Some(value)),
        "prior managed runtime container identity differs from its Core ownership state"
    );
    if let Some(expected_id) = state.container_id.as_deref() {
        ensure!(
            id == expected_id,
            "prior managed runtime name resolved to a different container ID"
        );
    }
    let stopped = Command::new(docker)
        .args(["stop", "--time", "180", &id])
        .status()
        .context("stopping prior managed runtime container")?;
    ensure!(
        stopped.success(),
        "could not stop prior managed runtime container"
    );
    ensure!(
        container_has_exact_labels(docker, &id, &state.labels)?,
        "prior managed runtime container changed identity before cleanup"
    );
    run_docker(docker, &["rm".to_owned(), id])?;
    fs::remove_file(state_path)?;
    Ok(())
}

fn inspect_container(
    docker: &Path,
    reference: &str,
) -> Result<Option<(String, BTreeMap<String, String>)>> {
    let output = Command::new(docker)
        .args([
            "inspect",
            "--type",
            "container",
            "--format",
            "{{.Id}}\n{{json .Config.Labels}}",
            reference,
        ])
        .output()
        .context("inspecting managed Docker container")?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        if error.contains("No such object") || error.contains("No such container") {
            return Ok(None);
        }
        bail!(
            "Docker container inspection failed: {}",
            bounded(&output.stderr)
        );
    }
    let text =
        String::from_utf8(output.stdout).context("Docker inspect returned non-UTF8 output")?;
    let (id, labels) = text
        .trim_end()
        .split_once('\n')
        .context("Docker inspect omitted container ID or labels")?;
    ensure!(
        is_hex(id, 12, 64),
        "Docker inspect returned an invalid container ID"
    );
    let labels = serde_json::from_str(labels).context("parsing Docker inspect labels")?;
    Ok(Some((id.to_owned(), labels)))
}

fn container_has_exact_labels(
    docker: &Path,
    id: &str,
    expected: &BTreeMap<String, String>,
) -> Result<bool> {
    let output = Command::new(docker)
        .args(["inspect", "--format", "{{json .Config.Labels}}", id])
        .output()?;
    if !output.status.success() {
        return Ok(false);
    }
    let actual: BTreeMap<String, String> =
        serde_json::from_slice(&output.stdout).context("parsing Docker container labels")?;
    Ok(expected
        .iter()
        .all(|(name, value)| actual.get(name) == Some(value)))
}

fn reserve_loopback_port() -> Result<u16> {
    let listener =
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).context("reserving loopback runtime port")?;
    Ok(listener.local_addr()?.port())
}

fn run_docker(docker: &Path, args: &[String]) -> Result<Output> {
    let output = Command::new(docker)
        .args(args)
        .output()
        .context("running Docker")?;
    ensure!(
        output.status.success(),
        "Docker command failed: {}",
        bounded(&output.stderr)
    );
    Ok(output)
}

fn docker_text(docker: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new(docker)
        .args(args)
        .output()
        .context("running Docker preflight")?;
    ensure!(
        output.status.success(),
        "Docker preflight failed: {}",
        bounded(&output.stderr)
    );
    String::from_utf8(output.stdout).context("Docker preflight returned non-UTF8 output")
}

fn mount_arg(host: &Path, container: &str, read_only: bool) -> Result<String> {
    ensure!(
        host.is_absolute(),
        "managed runtime bind source must be absolute"
    );
    let host = host
        .to_str()
        .context("managed runtime bind source must be UTF-8")?;
    ensure!(
        !host.contains(',') && !host.chars().any(char::is_control),
        "managed runtime bind source contains a Docker mount separator/control character"
    );
    ensure!(
        container.starts_with('/')
            && !container.contains(',')
            && !container.chars().any(char::is_control),
        "managed runtime bind destination is invalid"
    );
    Ok(format!(
        "type=bind,src={host},dst={container}{}",
        if read_only { ",readonly" } else { "" }
    ))
}

fn safe_relative(path: &Path) -> Result<PathBuf> {
    ensure!(
        !path.as_os_str().is_empty() && !path.is_absolute(),
        "archive path must be a non-empty relative path"
    );
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => result.push(part),
            _ => bail!("archive path contains a traversal component"),
        }
    }
    Ok(result)
}

fn safe_component(value: &str) -> String {
    let value: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    value.trim_matches('-').chars().take(48).collect::<String>()
}

fn safe_sidecar_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

fn verify_file(path: &Path, bytes: u64, sha: &str) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("stat {}", path.display()))?;
    ensure!(
        metadata.file_type().is_file() && metadata.len() == bytes,
        "managed runtime input {} size/type mismatch",
        path.display()
    );
    verify_file_sha(path, sha)
}

fn verify_file_sha(path: &Path, sha: &str) -> Result<()> {
    ensure!(
        file_sha256(path)? == sha,
        "managed runtime input {} SHA-256 mismatch",
        path.display()
    );
    Ok(())
}

fn file_sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 8 * 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn is_lower_sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn is_hex(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn random_hex(bytes: usize) -> Result<String> {
    let mut value = vec![0u8; bytes];
    getrandom::fill(&mut value).context("generating managed runtime nonce")?;
    Ok(value.into_iter().map(|b| format!("{b:02x}")).collect())
}

fn private_open(path: &Path) -> Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        return OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(Into::into);
    }
    #[cfg(not(unix))]
    {
        OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)
            .map_err(Into::into)
    }
}

fn atomic_write_private_json(path: &Path, value: &impl Serialize) -> Result<()> {
    atomic_write_private(path, &serde_json::to_vec_pretty(value)?)
}
fn atomic_write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!("tmp-{}", random_hex(8)?));
    let mut file = private_open(&temporary)?;
    file.set_len(0)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn set_private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn bounded(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).chars().take(512).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_profile_is_digest_only_loopback_and_has_no_catalog_command_surface() {
        let root = std::env::temp_dir().join(format!("mayhem-wrapper-{}", random_hex(8).unwrap()));
        fs::create_dir(&root).unwrap();
        let wrapper_path = write_launch_wrapper(&root, "Qwen/Qwen3.8-Flash-Next", 32123).unwrap();
        let wrapper = fs::read_to_string(&wrapper_path).unwrap();
        assert!(wrapper.contains("if args != expected:"));
        assert!(wrapper.contains("replace(\"--host\", \"0.0.0.0\", \"127.0.0.1\")"));
        assert!(wrapper.contains("replace(\"--port\", \"8001\", \"32123\")"));
        assert!(wrapper.contains("Qwen/Qwen3.8-Flash-Next"));
        assert!(wrapper.contains("\"reasoning_effort\":\"xhigh\""));
        assert!(wrapper
            .contains("replace(\"--linear-attn-prefill-backend\", \"flashinfer\", \"triton\")"));
        assert!(wrapper.contains("args.append(\"--enable-deterministic-inference\")"));
        assert!(wrapper.contains("/usr/local/bin/sglang"));
        let rejected = Command::new(&wrapper_path)
            .args(["serve", "--unexpected"])
            .output()
            .unwrap();
        assert!(!rejected.status.success());
        assert!(String::from_utf8_lossy(&rejected.stderr)
            .contains("qualified launcher argument vector mismatch"));
        assert_eq!(
            service_command(),
            ["/mayhem/source/configs/pennyroyal/serve-flash-next-frspec.sh"]
        );
        assert!(IMAGE.contains("@sha256:"));
        let args = service_create_args(ServiceCreateInputs {
            name: "mayhem-test",
            port: 32123,
            model_id: "Qwen/Qwen3.8-Flash-Next",
            snapshot: &root,
            prepared_model: &root,
            source: &root,
            plugin: &root,
            managed_root: &root,
            seccomp: &root.join("seccomp.json"),
            wrapper: &root.join("wrapper"),
            labels: &BTreeMap::new(),
        })
        .unwrap();
        for required in [
            "--restart=no",
            "--network=host",
            "--ipc=host",
            "--memory=104g",
            "--memory-swap=104g",
            "CARGO_BUILD_JOBS=2",
            "MAX_TOTAL_TOKENS=824384",
        ] {
            assert!(
                args.iter().any(|argument| argument == required),
                "{required}"
            );
        }
        #[cfg(unix)]
        assert!(args
            .iter()
            .any(|argument| argument == &owned_container_user_arg(&root).unwrap()));
        assert_eq!(args.last().unwrap(), &format!("/mayhem/source/{LAUNCHER}"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deterministic_flash_next_profile_changes_only_the_signed_runtime_controls() {
        let source = qualified_launcher_args()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let effective = effective_launcher_args("Qwen/Qwen3.8-Flash-Next", 32123);

        assert_eq!(
            effective.last().map(String::as_str),
            Some("--enable-deterministic-inference")
        );
        for (flag, expected) in [
            ("--attention-backend", "triton"),
            ("--linear-attn-prefill-backend", "triton"),
            ("--linear-attn-decode-backend", "flashinfer"),
            ("--mamba-radix-cache-strategy", "extra_buffer"),
            ("--hicache-storage-backend", "nixl"),
        ] {
            let index = effective.iter().position(|value| value == flag).unwrap();
            assert_eq!(effective[index + 1], expected);
        }
        assert!(!effective
            .iter()
            .any(|value| value == "--disable-radix-cache"));

        let changed = source
            .iter()
            .zip(&effective)
            .filter(|(before, after)| before != after)
            .map(|(before, after)| (before.as_str(), after.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            changed,
            [
                ("pennyroyal", "Qwen/Qwen3.8-Flash-Next"),
                ("0.0.0.0", "127.0.0.1"),
                ("8001", "32123"),
                ("flashinfer", "triton"),
                (
                    r#"{"enable_thinking":true,"preserve_thinking":true,"reasoning_effort":"medium"}"#,
                    r#"{"enable_thinking":true,"preserve_thinking":true,"reasoning_effort":"xhigh"}"#,
                ),
            ]
        );
        assert_eq!(effective.len(), source.len() + 3);
    }

    #[test]
    fn safe_archive_paths_reject_escape_and_absolute_paths() {
        assert!(safe_relative(Path::new("configs/runtime.json")).is_ok());
        assert!(safe_relative(Path::new("../escape")).is_err());
        assert!(safe_relative(Path::new("/absolute")).is_err());
        assert!(safe_relative(Path::new("./dot")).is_err());
    }

    #[test]
    fn embedded_seccomp_identity_is_stable() {
        assert_eq!(SECCOMP.len(), 16_286);
        assert_eq!(sha256_bytes(SECCOMP), SECCOMP_SHA256);
    }

    #[test]
    fn docker_mount_encoding_rejects_separator_injection() {
        assert!(mount_arg(Path::new("/safe/path"), "/mayhem/model", true).is_ok());
        assert!(mount_arg(Path::new("/unsafe,path"), "/mayhem/model", true).is_err());
        assert!(mount_arg(Path::new("relative"), "/mayhem/model", true).is_err());
    }

    #[test]
    fn reader_wheel_is_exact_and_installs_without_network_or_dependency_resolution() {
        let wheel = RecipeReaderWheel {
            sidecar: "ple_plugin_wheel".to_owned(),
            filename: PLE_READER_WHEEL_FILENAME.to_owned(),
            bytes: PLE_READER_WHEEL_BYTES,
            sha256: PLE_READER_WHEEL_SHA256.to_owned(),
            python_tag: "cp312".to_owned(),
            abi_tag: "cp312".to_owned(),
            platform_tag: "linux_x86_64".to_owned(),
        };
        validate_reader_wheel(&wheel, "0.2.0+pennyroyal2").unwrap();
        let wheel_container_path = format!("/mayhem/wheel/{}", wheel.filename);
        let command = plugin_install_command(&wheel_container_path);
        assert!(command.iter().any(|argument| argument == "--offline"));
        assert!(command.iter().any(|argument| argument == "--no-cache"));
        assert!(command.iter().any(|argument| argument == "--no-deps"));
        assert_eq!(command.last().unwrap(), &wheel_container_path);
        assert!(wheel_container_path.ends_with(PLE_READER_WHEEL_FILENAME));

        let mut wrong_platform = wheel;
        wrong_platform.platform_tag = "linux_aarch64".to_owned();
        assert!(validate_reader_wheel(&wrong_platform, "0.2.0+pennyroyal2").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn one_shot_containers_write_as_the_managed_runtime_owner() {
        use std::os::unix::fs::MetadataExt as _;

        let root =
            std::env::temp_dir().join(format!("mayhem-one-shot-owner-{}", random_hex(8).unwrap()));
        fs::create_dir(&root).unwrap();
        let metadata = fs::metadata(&root).unwrap();
        let args = owned_container_identity_args(&root).unwrap();
        assert_eq!(
            args[0],
            format!("--user={}:{}", metadata.uid(), metadata.gid())
        );
        assert_eq!(args[1], "--env=HOME=/tmp");
        assert_eq!(args[2], "--env=TMPDIR=/tmp");
        fs::remove_dir(&root).unwrap();
    }

    #[test]
    fn abandoned_partial_cleanup_is_limited_to_owned_prefixes() {
        let root = std::env::temp_dir().join(format!("mayhem-partials-{}", random_hex(8).unwrap()));
        fs::create_dir(&root).unwrap();
        for name in [
            ".source-abcd.partial-1",
            ".plugin.partial-2",
            ".ple.partial-3",
            "operator-data",
        ] {
            fs::create_dir(root.join(name)).unwrap();
        }
        cleanup_abandoned_partial_directories(&root).unwrap();
        assert!(!root.join(".source-abcd.partial-1").exists());
        assert!(!root.join(".plugin.partial-2").exists());
        assert!(!root.join(".ple.partial-3").exists());
        assert!(root.join("operator-data").is_dir());
        fs::remove_dir_all(root).unwrap();
    }
}
