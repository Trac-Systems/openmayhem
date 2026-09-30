//! Managed SM121 execution of the same signed Flash checkpoint. The signed
//! recipe pins the base image and each source overlay, with the normal Core
//! container ownership/cleanup lifecycle. No externally launched server is trusted.
use super::*;

const BASE_IMAGE: &str =
    "lmsysorg/sglang@sha256:14ed582518584c5c830206b5318a2c2769e68229c3422e48a28b952b3a888bd4";
const SOURCE_REVISION: &str = "d91c3682b0b429e4c70df63cd57f819588ce29b0";
const WEIGHT_REVISION: &str = "7b719225242aacd3dbd3f9407468c2ee9a9d2594";
const TABLE_BYTES: u64 = 51_200_245_760;
const TABLE_SHA256: &str = "b070f9644adf93794d8a1030584ab705809387e64396a9327a68fa3a3a6666b3";
const FILE_TARGETS: [(&str, &str); 5] = [
    (
        "tokenizer_manager",
        "/sgl-workspace/sglang/python/sglang/srt/managers/tokenizer_manager.py",
    ),
    (
        "model",
        "/sgl-workspace/sglang/python/sglang/srt/models/qwen4_exp.py",
    ),
    (
        "attention",
        "/sgl-workspace/sglang/python/sglang/srt/layers/attention/qwen_sparse_attn_backend.py",
    ),
    (
        "prefill",
        "/sgl-workspace/sglang/python/sglang/srt/layers/attention/qsa/sparse_attn.py",
    ),
    ("prepare_ple", "/mayhem/prepare_ple.py"),
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Recipe {
    schema_version: u32,
    profile_version: u32,
    public_model_id: String,
    artifact: RecipeArtifact,
    base_image: String,
    source_revision: String,
    code_sha256: String,
    files: BTreeMap<String, RecipeFile>,
    context: u32,
    max_concurrent: u32,
    max_running_requests: u32,
    max_total_tokens: u32,
    ple_table_bytes: u64,
    ple_table_sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeFile {
    sidecar: String,
    bytes: u64,
    sha256: String,
}

fn validate(inputs: &ManagedRuntimeInputs<'_>, recipe: &Recipe) -> Result<()> {
    ensure!(
        recipe.schema_version == 1 && recipe.profile_version == 1,
        "unsupported GB10 recipe version"
    );
    ensure!(
        recipe.public_model_id == inputs.public_model_id,
        "GB10 recipe does not bind the requested public model"
    );
    ensure!(
        recipe.base_image == BASE_IMAGE && recipe.source_revision == SOURCE_REVISION,
        "GB10 recipe must pin the qualified base image and source revision"
    );
    ensure!(
        BASE_IMAGE.split_once('@').map(|(_, digest)| digest) == Some(inputs.container_image_digest),
        "GB10 image differs from the signed runtime binding"
    );
    let artifact = &recipe.artifact;
    ensure!(
        artifact.repo == "RadixArk/Qwen3.8-Flash-Next-NVFP4"
            && artifact.repo == inputs.artifact_repo
            && artifact.revision == WEIGHT_REVISION
            && artifact.revision == inputs.artifact_revision
            && artifact.snapshot_manifest_sidecar == inputs.snapshot_manifest_sidecar
            && artifact.snapshot_manifest_sha256 == inputs.snapshot_manifest_sha256
            && artifact.file_count == inputs.snapshot_file_count
            && artifact.total_bytes == inputs.snapshot_total_bytes,
        "GB10 recipe must bind the exact signed baseline weight snapshot"
    );
    ensure!(
        recipe.context == 262_144
            && (1..=2).contains(&recipe.max_concurrent)
            && recipe.max_concurrent == inputs.max_concurrent
            && recipe.max_running_requests == 2
            && recipe.max_total_tokens == 525_312,
        "GB10 recipe exceeds its qualified context or scheduler envelope"
    );
    ensure!(
        recipe.ple_table_bytes == TABLE_BYTES && recipe.ple_table_sha256 == TABLE_SHA256,
        "GB10 recipe must bind the exact verified PLE table"
    );
    ensure!(
        recipe.files.len() == FILE_TARGETS.len(),
        "GB10 source inventory is incomplete"
    );
    let mut names = std::collections::BTreeSet::new();
    for (name, _) in FILE_TARGETS {
        let file = recipe
            .files
            .get(name)
            .context("GB10 recipe is missing a source file")?;
        ensure!(
            safe_sidecar_name(&file.sidecar)
                && names.insert(&file.sidecar)
                && (1..=16 * 1024 * 1024).contains(&file.bytes)
                && is_lower_sha(&file.sha256),
            "GB10 source file binding is invalid or duplicated"
        );
    }
    let code = serde_json::json!({"base_image":recipe.base_image,
        "source_revision":recipe.source_revision,"files":recipe.files});
    let code_sha =
        sha256_bytes(&mayhem_proto::stable_json_bytes(&code).map_err(anyhow::Error::msg)?);
    ensure!(
        code_sha == recipe.code_sha256 && recipe.code_sha256 == inputs.runtime_revision,
        "GB10 source inventory differs from the signed runtime revision"
    );
    Ok(())
}

pub(super) fn prepare(
    inputs: ManagedRuntimeInputs<'_>,
    recipe: Recipe,
) -> Result<ManagedOpenAiRuntime> {
    validate(&inputs, &recipe)?;
    ensure!(
        cfg!(all(target_os = "linux", target_arch = "aarch64")),
        "GB10 managed runtime requires Linux aarch64"
    );
    image_preflight(inputs.docker)?;
    let root = inputs
        .home
        .join("runtime/managed/openai-compatible")
        .join(safe_component(inputs.enclave_id));
    fs::create_dir_all(&root)?;
    set_private_directory(&root)?;
    let lock = private_open(&root.join("lock"))?;
    lock.try_lock_exclusive()
        .context("managed runtime is already owned by another process")?;
    reconcile_previous_runtime(inputs.docker, &root, &lock)?;
    let mut files = BTreeMap::new();
    let code = root.join(format!("code-{}", recipe.code_sha256));
    fs::create_dir_all(&code)?;
    set_private_directory(&code)?;
    for (name, record) in &recipe.files {
        let source = inputs
            .sidecars
            .get(&record.sidecar)
            .context("GB10 signed source sidecar was not downloaded")?;
        verify_file(source, record.bytes, &record.sha256)?;
        let target = code.join(format!("{name}.py"));
        // Copy into Core-owned storage so a caller cannot replace the downloaded
        // file after validation while the running container is using it.
        atomic_write_private(&target, &fs::read(source)?)?;
        files.insert(name.clone(), target);
    }
    let seccomp = root.join(format!("seccomp-{SECCOMP_SHA256}.json"));
    atomic_write_private(&seccomp, SECCOMP)?;
    let ple = root.join("ple");
    fs::create_dir_all(&ple)?;
    set_private_directory(&ple)?;
    let table = ple.join("ple-fp8.raw");
    let manifest = ple.join("ple-fp8.raw.json");
    if table.exists() || manifest.exists() {
        verify_table(&table, &manifest)?;
    } else {
        let free = fs2::available_space(&ple).context("checking PLE cache disk headroom")?;
        ensure!(
            free >= TABLE_BYTES + 8 * 1024 * 1024 * 1024,
            "GB10 PLE preparation requires table size plus 8 GiB free disk space"
        );
        let command = [
            "python3",
            "/mayhem/prepare_ple.py",
            "--snapshot",
            "/model",
            "--output",
            "/ple/ple-fp8.raw",
            "--revision",
            WEIGHT_REVISION,
            "--verify-samples",
            "256",
        ]
        .map(str::to_owned);
        run_owned_one_shot_capture_with_image(
            inputs.docker,
            &root,
            "gb10-ple",
            inputs.recipe_sha256,
            inputs.provider_id,
            inputs.enclave_id,
            &[
                (inputs.snapshot_dir, "/model", true),
                (&files["prepare_ple"], "/mayhem/prepare_ple.py", true),
                (&ple, "/ple", false),
            ],
            &[],
            &command,
            BASE_IMAGE,
        )?;
        verify_table(&table, &manifest)?;
    }
    let port = reserve_loopback_port()?;
    start_owned_runtime(&inputs, &root, lock, port, |name, labels| {
        create_args(
            &inputs, &recipe, &root, &files, &ple, &seccomp, name, labels, port,
        )
    })
}

fn image_preflight(docker: &Path) -> Result<()> {
    let version = docker_text(docker, &["version", "--format", "{{.Server.Version}}"])?;
    // Both engines were qualified with the same pinned image, explicit seccomp
    // profile, non-root identity and cgroup controls. Do not accept untested
    // engines merely because they are newer or share a major version.
    ensure!(
        matches!(version.trim(), "29.1.3" | "29.6.2"),
        "GB10 managed runtime requires qualified Docker 29.1.3 or 29.6.2 (found {})",
        version.trim()
    );
    let info: serde_json::Value =
        serde_json::from_str(&docker_text(docker, &["info", "--format", "{{json .}}"])?)?;
    ensure!(
        info["CgroupVersion"] == "2"
            && info["CgroupDriver"] == "systemd"
            && info["MemoryLimit"] == true
            && info["SwapLimit"] == true,
        "GB10 Docker cgroup v2 memory/swap controls are unavailable"
    );
    // GPU device requests are supported by Docker without a named NVIDIA
    // runtime. The owned GPU container and native preflight prove GPU access.
    if docker_text(
        docker,
        &["image", "inspect", BASE_IMAGE, "--format", "{{.Id}}"],
    )
    .is_err()
    {
        run_docker(
            docker,
            &[
                "pull".to_owned(),
                "--platform=linux/arm64".to_owned(),
                BASE_IMAGE.to_owned(),
            ],
        )?;
    }
    let image: serde_json::Value = serde_json::from_str(&docker_text(
        docker,
        &["image", "inspect", BASE_IMAGE, "--format", "{{json .}}"],
    )?)?;
    ensure!(
        image["Architecture"] == "arm64"
            && image["Os"] == "linux"
            && image["RepoDigests"]
                .as_array()
                .is_some_and(|digests| digests
                    .iter()
                    .any(|digest| digest.as_str() == Some(BASE_IMAGE))),
        "GB10 image architecture or digest does not match its signed profile"
    );
    Ok(())
}

fn verify_table(table: &Path, manifest: &Path) -> Result<()> {
    verify_file(table, TABLE_BYTES, TABLE_SHA256)?;
    ensure!(
        fs::metadata(manifest)?.len() <= 16 * 1024,
        "GB10 PLE manifest is oversized"
    );
    let value: serde_json::Value = serde_json::from_slice(&fs::read(manifest)?)?;
    ensure!(
        value["schema"] == 1
            && value["revision"] == WEIGHT_REVISION
            && value["source"] == "RadixArk/Qwen3.8-Flash-Next-NVFP4"
            && value["dtype"] == "float8_e4m3fn"
            && value["size_bytes"] == TABLE_BYTES
            && value["sha256"] == TABLE_SHA256
            && value["rows"]
                .as_u64()
                .zip(value["embedding_dim"].as_u64())
                .and_then(|(rows, dim)| rows.checked_mul(dim))
                == Some(TABLE_BYTES),
        "GB10 PLE manifest does not describe the verified model table"
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn create_args(
    inputs: &ManagedRuntimeInputs<'_>,
    recipe: &Recipe,
    root: &Path,
    files: &BTreeMap<String, PathBuf>,
    ple: &Path,
    seccomp: &Path,
    name: &str,
    labels: &BTreeMap<String, String>,
    port: u16,
) -> Result<Vec<String>> {
    let cache = root.join("cache");
    let runtime_home = root.join("home");
    for directory in [&cache, &runtime_home] {
        fs::create_dir_all(directory)?;
        set_private_directory(directory)?;
    }
    let mut args = [
        "create",
        "--name",
        name,
        "--restart=no",
        "--init",
        "--gpus=device=0",
        "--ipc=host",
        "--shm-size=16g",
        "--pids-limit=32768",
        "--log-driver=json-file",
        "--log-opt=max-size=10m",
        "--log-opt=max-file=3",
        "--security-opt=no-new-privileges:true",
        "--cap-drop=ALL",
        "--entrypoint=sglang",
    ]
    .map(str::to_owned)
    .to_vec();
    args.extend([
        format!("--memory={GB10_CONTAINER_MEMORY_BYTES}"),
        format!("--memory-swap={GB10_CONTAINER_MEMORY_BYTES}"),
        format!("--publish=127.0.0.1:{port}:30000"),
        format!("--security-opt=seccomp={}", seccomp.display()),
    ]);
    args.extend(owned_container_identity_args(root)?);
    for (key, value) in labels {
        args.push(format!("--label={key}={value}"));
    }
    for (host, target, readonly) in [
        (inputs.snapshot_dir, "/model", true),
        (ple, "/ple-cache", true),
        (cache.as_path(), "/mayhem/cache", false),
        (runtime_home.as_path(), "/mayhem/home", false),
    ] {
        args.extend(["--mount".to_owned(), mount_arg(host, target, readonly)?]);
    }
    for (key, target) in FILE_TARGETS {
        args.extend(["--mount".to_owned(), mount_arg(&files[key], target, true)?]);
    }
    for (key, value) in [
        ("HOME", "/mayhem/home"),
        ("XDG_CACHE_HOME", "/mayhem/cache"),
        ("HF_HUB_OFFLINE", "1"),
        ("SGLANG_TOOL_STRICT_LEVEL", "1"),
        ("SGLANG_QWEN4_PLE_DISK_CACHE_PATH", "/ple-cache/ple-fp8.raw"),
        (
            "SGLANG_QWEN4_PLE_DISK_CACHE_MANIFEST",
            "/ple-cache/ple-fp8.raw.json",
        ),
        ("SGLANG_QWEN4_PLE_SOURCE_REVISION", WEIGHT_REVISION),
        ("SGLANG_QWEN4_PLE_CACHE_BYTES", "268435456"),
        ("SGLANG_QWEN4_PLE_CACHE_MAX_LOOKUP_ROWS", "4096"),
    ] {
        args.extend(["--env".to_owned(), format!("{key}={value}")]);
    }
    args.push(BASE_IMAGE.to_owned());
    args.extend(server_args(inputs.public_model_id, recipe));
    Ok(args)
}

fn server_args(model: &str, recipe: &Recipe) -> Vec<String> {
    // Leave enough allocator budget for the qualified fixed KV pool even when
    // Core's resident startup overhead lowers the initial free-memory sample.
    // --max-total-tokens still caps the actual allocation at the signed size;
    // native preflight rejects any smaller pool instead of advertising two slots.
    let mut args = ["serve", "--model-path", "/model", "--served-model-name", model,
        "--chat-template", "/model/chat_template.jinja", "--trust-remote-code", "--host", "0.0.0.0",
        "--port", "30000", "--dtype", "bfloat16", "--quantization", "modelopt_fp4",
        "--fp4-gemm-backend", "flashinfer_cutlass", "--kv-cache-dtype", "fp8_e4m3", "--page-size", "64",
        "--enable-deterministic-inference", "--attention-backend", "triton",
        "--mamba-radix-cache-strategy", "extra_buffer", "--mamba-track-interval", "64", "--max-mamba-cache-size", "20",
        "--mamba-ssm-dtype", "float32", "--chunked-prefill-size", "4096", "--mem-fraction-static", "0.91",
        "--ple-offload-embedding", "--enable-metrics", "--reasoning-parser", "qwen3", "--tool-call-parser", "qwen3_coder",
        "--preferred-sampling-params", "{\"temperature\":1.0,\"top_p\":0.95,\"top_k\":20,\"min_p\":0.0,\"presence_penalty\":0.0,\"repetition_penalty\":1.0}",
        "--disable-prefill-cuda-graph", "--cuda-graph-backend-decode", "disabled", "--disable-flashinfer-autotune",
        "--speculative-algorithm", "NEXTN", "--speculative-num-steps", "2", "--speculative-eagle-topk", "1",
        "--speculative-num-draft-tokens", "3"].map(str::to_owned).to_vec();
    for (key, value) in [
        ("--max-running-requests", recipe.max_running_requests),
        ("--max-total-tokens", recipe.max_total_tokens),
        ("--context-length", recipe.context),
    ] {
        args.extend([key.to_owned(), value.to_string()]);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe() -> Recipe {
        let files: BTreeMap<_, _> = FILE_TARGETS
            .into_iter()
            .map(|(name, _)| {
                (
                    name.to_owned(),
                    RecipeFile {
                        sidecar: format!("gb10_{name}"),
                        bytes: 1024,
                        sha256: "ab".repeat(32),
                    },
                )
            })
            .collect();
        let code = serde_json::json!({"base_image":BASE_IMAGE,"source_revision":SOURCE_REVISION,"files":files});
        Recipe {
            schema_version: 1,
            profile_version: 1,
            public_model_id: "Qwen/Qwen3.8-Flash-Next".to_owned(),
            artifact: RecipeArtifact {
                repo: "RadixArk/Qwen3.8-Flash-Next-NVFP4".to_owned(),
                revision: WEIGHT_REVISION.to_owned(),
                snapshot_manifest_sidecar: "snapshot_manifest".to_owned(),
                snapshot_manifest_sha256: "cd".repeat(32),
                file_count: 419,
                total_bytes: 135_253_622_894,
            },
            base_image: BASE_IMAGE.to_owned(),
            source_revision: SOURCE_REVISION.to_owned(),
            code_sha256: sha256_bytes(&mayhem_proto::stable_json_bytes(&code).unwrap()),
            files,
            context: 262_144,
            max_concurrent: 2,
            max_running_requests: 2,
            max_total_tokens: 525_312,
            ple_table_bytes: TABLE_BYTES,
            ple_table_sha256: TABLE_SHA256.to_owned(),
        }
    }

    fn inputs<'a>(
        root: &'a Path,
        sidecars: &'a BTreeMap<String, PathBuf>,
        recipe: &'a Recipe,
    ) -> ManagedRuntimeInputs<'a> {
        ManagedRuntimeInputs {
            home: root,
            docker: Path::new("docker"),
            provider_id: "fixture-provider",
            enclave_id: "fixture-enclave",
            public_model_id: &recipe.public_model_id,
            artifact_repo: &recipe.artifact.repo,
            artifact_revision: &recipe.artifact.revision,
            snapshot_manifest_sidecar: &recipe.artifact.snapshot_manifest_sidecar,
            snapshot_manifest_sha256: &recipe.artifact.snapshot_manifest_sha256,
            snapshot_file_count: recipe.artifact.file_count,
            snapshot_total_bytes: recipe.artifact.total_bytes,
            runtime_revision: &recipe.code_sha256,
            container_image_digest: BASE_IMAGE.split_once('@').unwrap().1,
            max_concurrent: recipe.max_concurrent,
            recipe_sha256: "fixture",
            recipe_path: root,
            snapshot_dir: root,
            sidecars,
        }
    }

    #[test]
    fn gb10_recipe_rejects_unqualified_weights_code_and_capacity() {
        let baseline = recipe();
        let sidecars = BTreeMap::new();
        let bound_inputs = inputs(Path::new("/fixture"), &sidecars, &baseline);
        validate(&bound_inputs, &baseline).unwrap();
        for change in 0..8 {
            let mut changed = recipe();
            match change {
                0 => changed.artifact.revision = "00".repeat(20),
                1 => changed.context *= 2,
                2 => changed.max_concurrent = 3,
                3 => {
                    changed.files.remove("prefill");
                }
                4 => changed.files.get_mut("model").unwrap().sha256 = "11".repeat(32),
                5 => changed.ple_table_sha256 = "11".repeat(32),
                6 => changed.base_image = "lmsysorg/sglang:latest".to_owned(),
                _ => changed.files.get_mut("attention").unwrap().sidecar = "../code".to_owned(),
            }
            assert!(
                validate(&bound_inputs, &changed).is_err(),
                "change {change}"
            );
        }
        let mut single = recipe();
        single.max_concurrent = 1;
        validate(&inputs(Path::new("/fixture"), &sidecars, &single), &single).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn gb10_preflight_accepts_docker_line_endings_and_rejects_unqualified_engines() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("mayhem-gb10-preflight-{}", random_hex(8).unwrap()));
        fs::create_dir(&root).unwrap();
        let docker = root.join("docker");
        let info = serde_json::json!({"CgroupVersion":"2","CgroupDriver":"systemd","MemoryLimit":true,"SwapLimit":true,"Runtimes":{"runc":{}}});
        let image =
            serde_json::json!({"Architecture":"arm64","Os":"linux","RepoDigests":[BASE_IMAGE]});
        for (version, accepted) in [
            ("29.1.3", true),
            ("29.6.2", true),
            ("29.1.2", false),
            ("29.6.1", false),
            ("29.6.3", false),
            ("30.0.0", false),
            ("", false),
        ] {
            fs::write(&docker, format!("#!/bin/sh\ncase \"$1\" in\nversion) echo '{version}';;\ninfo) echo '{info}';;\nimage) echo '{image}';;\n*) exit 1;;\nesac\n")).unwrap();
            fs::set_permissions(&docker, fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(
                image_preflight(&docker).is_ok(),
                accepted,
                "version {version}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn gb10_launch_is_owned_loopback_bounded_and_uses_all_qualified_overlays() {
        let root = std::env::temp_dir().join(format!("mayhem-gb10-{}", random_hex(8).unwrap()));
        fs::create_dir(&root).unwrap();
        let recipe = recipe();
        let files = FILE_TARGETS
            .into_iter()
            .map(|(name, _)| (name.to_owned(), root.join(format!("{name}.py"))))
            .collect();
        let sidecars = BTreeMap::new();
        let inputs = inputs(&root, &sidecars, &recipe);
        let args = create_args(
            &inputs,
            &recipe,
            &root,
            &files,
            &root.join("ple"),
            &root.join("seccomp.json"),
            "fixture",
            &BTreeMap::from([("mayhem.owner".to_owned(), "fixture".to_owned())]),
            30042,
        )
        .unwrap();
        assert!(!args.iter().any(|arg| arg.starts_with("--runtime=")));
        assert!(args.iter().any(|arg| arg == "--gpus=device=0"));
        for expected in [
            "--publish=127.0.0.1:30042:30000",
            "--memory=120259084288",
            "--memory-swap=120259084288",
            "--restart=no",
            "--cap-drop=ALL",
            "--log-opt=max-size=10m",
            "--log-opt=max-file=3",
            BASE_IMAGE,
        ] {
            assert!(args.iter().any(|arg| arg == expected), "{expected}");
        }
        assert!(args.iter().any(|arg| arg.starts_with("--user=")));
        assert!(args.iter().any(|arg| arg == "SGLANG_TOOL_STRICT_LEVEL=1"));
        assert!(!args
            .iter()
            .any(|arg| arg == "--privileged" || arg == "--network=host"));
        for (_, target) in FILE_TARGETS {
            assert!(
                args.iter()
                    .any(|arg| arg.contains(target) && arg.contains("readonly")),
                "{target}"
            );
        }
        let server = server_args(&recipe.public_model_id, &recipe);
        assert!(server.iter().any(|arg| arg == "--enable-metrics"));
        assert!(server
            .iter()
            .any(|arg| arg == "--enable-deterministic-inference"));
        // SGLang must keep prefix caching enabled under deterministic inference.
        assert!(server
            .windows(2)
            .any(|args| args == ["--attention-backend", "triton"]));
        assert!(!server.iter().any(|arg| arg == "--disable-radix-cache"));
        assert!(server
            .windows(2)
            .any(|args| args == ["--context-length", "262144"]));
        assert!(server
            .windows(2)
            .any(|args| args == ["--max-total-tokens", "525312"]));
        assert!(server
            .windows(2)
            .any(|args| args == ["--tool-call-parser", "qwen3_coder"]));
        fs::remove_dir_all(root).unwrap();
    }
}
