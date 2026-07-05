#![forbid(unsafe_code)]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use mayhem_enclave::{
    boot_sealed_store, build_merkle_manifest, build_sandbox_profile,
    build_tier1_attestation_report, build_tier2_attestation_report, exec_sandboxed_child,
    expect_outbound_tcp_denied, hex_secret, load_or_create_runtime_keypair_store, measure_binary,
    measure_current_binary, prepare_tier2_hardware_quote_binding, probe_outbound_tcp,
    provider_signing_seed_from_hex, run_sandboxed_command, seal_artifact, unix_timestamp_now,
    BootOptions, KeyContext, RuntimeKeyContext, RuntimeKeypairStoreOptions, SandboxConfig,
    SandboxPlatform, SealOptions, Tier1AttestationOptions, Tier2AttestationOptions,
    Tier2HardwareQuoteBindingOptions, DEFAULT_CHUNK_SIZE, DEFAULT_TCP_PROBE_TIMEOUT_MS,
};
use mayhem_proto::{
    catalog_enclave_id, AttestationRuntimeConfig, CatalogEnclaveIdentity, HardwareQuote,
    HardwareQuoteKind,
};

#[derive(Debug, Parser)]
#[command(name = "mayhem-enclave")]
#[command(about = "Mayhem sealed enclave host utilities")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Seal a local verified artifact into an encrypted enclave store.
    SealLocal(SealLocalArgs),
    /// Open a sealed store enough to verify boot-time integrity.
    BootCheck(BootCheckArgs),
    /// Print the BLAKE3 measurement of an enclave binary.
    MeasureBinary(MeasureBinaryArgs),
    /// Create or load the provider-sealed runtime attestation keypair.
    InitKeypair(RuntimeKeypairArgs),
    /// Build and sign a Tier-1 attestation report for a challenge nonce.
    Attest(Box<AttestArgs>),
    /// Print the platform sandbox policy for a sealed enclave process.
    SandboxProfile(SandboxProfileArgs),
    /// Run a command under the current platform sandbox.
    SandboxRun(SandboxRunArgs),
    #[command(hide = true, name = "sandbox-exec-child")]
    SandboxExecChild(SandboxExecChildArgs),
    /// Probe whether outbound TCP is denied from this process.
    SandboxProbeTcp(SandboxProbeTcpArgs),
    /// Probe whether sealed-store writes are denied from this process.
    SandboxProbeStoreWrite(SandboxProbeStoreWriteArgs),
}

#[derive(Debug, Parser)]
struct SealLocalArgs {
    /// Plain artifact path.
    #[arg(long, value_name = "PATH")]
    artifact: PathBuf,

    /// Destination sealed store directory.
    #[arg(long, value_name = "PATH")]
    sealed_store: PathBuf,

    /// Provider-local sealing secret as hex. Must be at least 32 bytes.
    #[arg(long)]
    provider_secret_hex: String,

    /// Provider identity used in the sealing key context.
    #[arg(long)]
    provider_id: String,

    /// Admin-created enclave identity used in the sealing key context.
    #[arg(long)]
    enclave_id: String,

    /// Catalog artifact Merkle root expected before sealing.
    #[arg(long)]
    artifact_root: Option<String>,

    /// Catalog manifest hash used in the sealing key context.
    #[arg(long)]
    manifest_hash: String,

    /// Chunk size in bytes.
    #[arg(long, default_value_t = DEFAULT_CHUNK_SIZE)]
    chunk_size: usize,

    /// Print a machine-readable report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct BootCheckArgs {
    /// Sealed store directory.
    #[arg(long, value_name = "PATH")]
    sealed_store: PathBuf,

    /// Provider-local sealing secret as hex. Must be at least 32 bytes.
    #[arg(long)]
    provider_secret_hex: String,

    /// Provider identity used in the sealing key context.
    #[arg(long)]
    provider_id: String,

    /// Admin-created enclave identity used in the sealing key context.
    #[arg(long)]
    enclave_id: String,

    /// Catalog artifact Merkle root expected at boot.
    #[arg(long)]
    artifact_root: String,

    /// Catalog manifest hash used in the sealing key context.
    #[arg(long)]
    manifest_hash: String,

    /// Optional decrypted output path for smoke/debug only.
    #[arg(long, value_name = "PATH")]
    output: Option<PathBuf>,

    /// Print a machine-readable report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct MeasureBinaryArgs {
    /// Binary path to measure. Defaults to this running mayhem-enclave binary.
    #[arg(long, value_name = "PATH")]
    binary: Option<PathBuf>,

    /// Print a machine-readable report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct RuntimeKeypairArgs {
    /// Runtime keypair store path.
    #[arg(long, value_name = "PATH")]
    keypair_store: PathBuf,

    /// Provider-local sealing secret as hex. Must be at least 32 bytes.
    #[arg(long)]
    provider_secret_hex: String,

    /// Provider identity used in the sealed keypair context.
    #[arg(long)]
    provider_id: String,

    /// Admin-created enclave identity used in the sealed keypair context.
    #[arg(long)]
    enclave_id: String,

    /// Print a machine-readable report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct AttestArgs {
    /// Runtime keypair store path.
    #[arg(long, value_name = "PATH")]
    keypair_store: PathBuf,

    /// Provider-local sealing secret as hex. Must be at least 32 bytes.
    #[arg(long)]
    provider_secret_hex: String,

    /// Provider identity used in the sealed keypair context.
    #[arg(long)]
    provider_id: String,

    /// Admin-created enclave identity expected for this report, or "auto" to compute it.
    #[arg(long)]
    enclave_id: String,

    /// Provider Ed25519 signing seed as 32 bytes of hex.
    #[arg(long)]
    provider_signing_seed_hex: String,

    /// Admin public key from the canonical enclave record.
    #[arg(long)]
    admin_pubkey: String,

    /// Catalog model id from the canonical enclave record.
    #[arg(long)]
    model_id: String,

    /// Catalog artifact Merkle root from the canonical enclave record.
    #[arg(long)]
    artifact_root: String,

    /// Catalog manifest hash from the canonical enclave record.
    #[arg(long)]
    manifest_hash: String,

    /// Binary path to self-measure. Defaults to this running mayhem-enclave binary.
    #[arg(long, value_name = "PATH")]
    binary: Option<PathBuf>,

    /// User challenge nonce as 32 bytes of hex.
    #[arg(long)]
    nonce_u: String,

    /// Boot epoch seconds. Defaults to the current Unix time.
    #[arg(long)]
    boot_epoch: Option<u64>,

    /// Report timestamp seconds. Defaults to the current Unix time.
    #[arg(long)]
    report_ts: Option<u64>,

    /// Runtime config JSON to bind into the report.
    #[arg(long, value_name = "JSON")]
    runtime_config_json: Option<String>,

    /// Print the Tier-2 hardware quote binding and exit. Use it as the vendor attestation nonce.
    #[arg(long)]
    print_hw_quote_binding: bool,

    /// Tier-2 hardware quote kind.
    #[arg(long, value_enum, default_value_t = HardwareQuoteKindArg::NvidiaNrasJwt)]
    hw_quote_kind: HardwareQuoteKindArg,

    /// Tier-2 hardware quote evidence file.
    #[arg(long, value_name = "PATH")]
    hw_quote_evidence_file: Option<PathBuf>,

    /// Tier-2 hardware quote binding. Must match the printed binding used as quote nonce.
    #[arg(long)]
    hw_quote_binding: Option<String>,

    /// Optional Tier-2 hardware quote endorsement string. Repeatable.
    #[arg(long)]
    hw_quote_endorsement: Vec<String>,

    /// Print a machine-readable report.
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum HardwareQuoteKindArg {
    AppleAppAttestJwt,
    AmdSevSnpVcek,
    IntelTdxDcap,
    NvidiaGb10DeviceJwt,
    NvidiaNrasJwt,
    MockTier2,
}

impl From<HardwareQuoteKindArg> for HardwareQuoteKind {
    fn from(value: HardwareQuoteKindArg) -> Self {
        match value {
            HardwareQuoteKindArg::AppleAppAttestJwt => HardwareQuoteKind::AppleAppAttestJwt,
            HardwareQuoteKindArg::AmdSevSnpVcek => HardwareQuoteKind::AmdSevSnpVcek,
            HardwareQuoteKindArg::IntelTdxDcap => HardwareQuoteKind::IntelTdxDcap,
            HardwareQuoteKindArg::NvidiaGb10DeviceJwt => HardwareQuoteKind::NvidiaGb10DeviceJwt,
            HardwareQuoteKindArg::NvidiaNrasJwt => HardwareQuoteKind::NvidiaNrasJwt,
            HardwareQuoteKindArg::MockTier2 => HardwareQuoteKind::MockTier2,
        }
    }
}

impl HardwareQuoteKindArg {
    fn as_str(self) -> &'static str {
        match self {
            HardwareQuoteKindArg::AppleAppAttestJwt => "apple_app_attest_jwt",
            HardwareQuoteKindArg::AmdSevSnpVcek => "amd_sev_snp_vcek",
            HardwareQuoteKindArg::IntelTdxDcap => "intel_tdx_dcap",
            HardwareQuoteKindArg::NvidiaGb10DeviceJwt => "nvidia_gb10_device_jwt",
            HardwareQuoteKindArg::NvidiaNrasJwt => "nvidia_nras_jwt",
            HardwareQuoteKindArg::MockTier2 => "mock_tier2",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum PlatformArg {
    Current,
    Linux,
    Macos,
    Windows,
}

#[derive(Debug, Parser)]
struct SandboxProfileArgs {
    /// Read-only sealed store directory.
    #[arg(long, value_name = "PATH")]
    sealed_store: PathBuf,

    /// Local IPC socket/named-pipe path allowed through the sandbox.
    #[arg(long, value_name = "PATH")]
    ipc_socket: PathBuf,

    /// Platform profile to render.
    #[arg(long, value_enum, default_value_t = PlatformArg::Current)]
    platform: PlatformArg,

    /// Print a machine-readable report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct SandboxRunArgs {
    /// Read-only sealed store directory.
    #[arg(long, value_name = "PATH")]
    sealed_store: PathBuf,

    /// Local IPC socket/named-pipe path allowed through the sandbox.
    #[arg(long, value_name = "PATH")]
    ipc_socket: PathBuf,

    /// Command and arguments to run under the sandbox.
    #[arg(required = true, trailing_var_arg = true)]
    command: Vec<String>,
}

#[derive(Debug, Parser)]
struct SandboxExecChildArgs {
    /// Read-only sealed store directory.
    #[arg(long, value_name = "PATH")]
    sealed_store: PathBuf,

    /// Local IPC socket/named-pipe path allowed through the sandbox.
    #[arg(long, value_name = "PATH")]
    ipc_socket: PathBuf,

    /// Command and arguments to exec after applying child-only sandbox rules.
    #[arg(required = true, trailing_var_arg = true)]
    command: Vec<String>,
}

#[derive(Debug, Parser)]
struct SandboxProbeTcpArgs {
    /// TCP address to connect to, e.g. 127.0.0.1:12345.
    #[arg(long)]
    addr: String,

    /// Treat sandbox-denied TCP as success and any other outcome as failure.
    #[arg(long)]
    expect_denied: bool,

    /// Probe timeout in milliseconds.
    #[arg(long, default_value_t = DEFAULT_TCP_PROBE_TIMEOUT_MS)]
    timeout_ms: u64,

    /// Print a machine-readable report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct SandboxProbeStoreWriteArgs {
    /// Sealed store directory to probe.
    #[arg(long, value_name = "PATH")]
    sealed_store: PathBuf,

    /// Treat sandbox-denied writes as success and successful writes as failure.
    #[arg(long)]
    expect_denied: bool,

    /// Try to restore owner write bits before probing the write path.
    #[arg(long)]
    attempt_restore_permissions: bool,

    /// Print a machine-readable report.
    #[arg(long)]
    json: bool,
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32, Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Commands::SealLocal(args) => seal_local(args)?,
        Commands::BootCheck(args) => boot_check(args)?,
        Commands::MeasureBinary(args) => measure_binary_command(args)?,
        Commands::InitKeypair(args) => init_keypair(args)?,
        Commands::Attest(args) => attest(*args)?,
        Commands::SandboxProfile(args) => sandbox_profile(args)?,
        Commands::SandboxRun(args) => return sandbox_run(args),
        Commands::SandboxExecChild(args) => return sandbox_exec_child(args),
        Commands::SandboxProbeTcp(args) => sandbox_probe_tcp(args)?,
        Commands::SandboxProbeStoreWrite(args) => sandbox_probe_store_write(args)?,
    }
    Ok(0)
}

fn seal_local(args: SealLocalArgs) -> Result<(), Box<dyn std::error::Error>> {
    let merkle = build_merkle_manifest(&args.artifact, args.chunk_size)?;
    let artifact_root = args.artifact_root.unwrap_or_else(|| merkle.root.clone());
    let context = KeyContext {
        provider_id: args.provider_id,
        enclave_id: args.enclave_id,
        artifact_root: artifact_root.clone(),
        manifest_hash: args.manifest_hash,
    };
    let mut options = SealOptions::new(
        args.artifact,
        args.sealed_store,
        context,
        hex_secret(&args.provider_secret_hex)?,
    );
    options.chunk_size = args.chunk_size;
    options.expected_merkle_root = Some(artifact_root);
    let report = seal_artifact(&options)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "sealed {} bytes in {} chunks at {}",
            report.total_bytes,
            report.chunk_count,
            report.store_dir.display()
        );
        println!("artifact_root={}", report.merkle_root);
    }
    Ok(())
}

fn measure_binary_command(args: MeasureBinaryArgs) -> Result<(), Box<dyn std::error::Error>> {
    let binary_hash = if let Some(path) = args.binary {
        measure_binary(&path)?
    } else {
        measure_current_binary()?
    };

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "binary_hash": binary_hash,
            }))?
        );
    } else {
        println!("binary_hash={binary_hash}");
    }
    Ok(())
}

fn init_keypair(args: RuntimeKeypairArgs) -> Result<(), Box<dyn std::error::Error>> {
    let context = RuntimeKeyContext {
        provider_id: args.provider_id,
        enclave_id: args.enclave_id,
    };
    let options = RuntimeKeypairStoreOptions::new(
        args.keypair_store,
        context,
        hex_secret(&args.provider_secret_hex)?,
    );
    let keypair = load_or_create_runtime_keypair_store(&options)?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "keypair_store": options.path,
                "enclave_pubkey": keypair.public_key_hex(),
            }))?
        );
    } else {
        println!("enclave_pubkey={}", keypair.public_key_hex());
        println!("keypair_store={}", options.path.display());
    }
    Ok(())
}

fn attest(args: AttestArgs) -> Result<(), Box<dyn std::error::Error>> {
    let now = unix_timestamp_now()?;
    let binary_path = match args.binary {
        Some(path) => path,
        None => std::env::current_exe()?,
    };
    let measured_binary_hash = measure_binary(&binary_path)?;
    let identity = CatalogEnclaveIdentity {
        admin_pubkey: args.admin_pubkey.clone(),
        model_id: args.model_id.clone(),
        artifact_root: args.artifact_root.clone(),
        manifest_hash: args.manifest_hash.clone(),
        binary_hash: measured_binary_hash.clone(),
    };
    let computed_enclave_id = catalog_enclave_id(&identity);
    if args.enclave_id != "auto" && args.enclave_id != computed_enclave_id {
        return Err(format!(
            "computed enclave_id {} does not match expected {}",
            computed_enclave_id, args.enclave_id
        )
        .into());
    }
    let context = RuntimeKeyContext {
        provider_id: args.provider_id.clone(),
        enclave_id: computed_enclave_id.clone(),
    };
    let keypair_options = RuntimeKeypairStoreOptions::new(
        args.keypair_store.clone(),
        context,
        hex_secret(&args.provider_secret_hex)?,
    );
    let runtime_keypair = load_or_create_runtime_keypair_store(&keypair_options)?;
    let provider_signing_seed = provider_signing_seed_from_hex(&args.provider_signing_seed_hex)?;
    let boot_epoch = args.boot_epoch.unwrap_or(now);
    let report_ts = args.report_ts.unwrap_or(now);
    let runtime_config = parse_runtime_config(args.runtime_config_json.as_deref())?;

    if args.print_hw_quote_binding {
        let binding = prepare_tier2_hardware_quote_binding(&Tier2HardwareQuoteBindingOptions {
            identity: identity.clone(),
            runtime_keypair,
            provider_signing_seed,
            binary_path,
            boot_epoch,
            report_ts,
            nonce_u: args.nonce_u,
            runtime_config,
        })?;
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "hw_quote_binding": binding,
                    "nonce": binding,
                    "enclave_id": computed_enclave_id,
                    "binary_hash": measured_binary_hash,
                    "quote_kind": args.hw_quote_kind.as_str(),
                }))?
            );
        } else {
            println!("hw_quote_binding={binding}");
            println!("enclave_id={computed_enclave_id}");
            println!("binary_hash={measured_binary_hash}");
            println!("Use this value as the hardware attestation nonce.");
        }
        return Ok(());
    }

    let report = if let Some(evidence_file) = args.hw_quote_evidence_file {
        let binding = args
            .hw_quote_binding
            .ok_or("Tier-2 attestation requires --hw-quote-binding")?;
        let evidence = fs::read_to_string(&evidence_file)?;
        build_tier2_attestation_report(&Tier2AttestationOptions {
            identity,
            runtime_keypair,
            provider_signing_seed,
            binary_path,
            boot_epoch,
            report_ts,
            nonce_u: args.nonce_u,
            hw_quote: HardwareQuote {
                kind: args.hw_quote_kind.into(),
                evidence,
                binding,
                endorsements: args.hw_quote_endorsement,
            },
            runtime_config,
        })?
    } else {
        if args.hw_quote_binding.is_some() || !args.hw_quote_endorsement.is_empty() {
            return Err(
                "hardware quote flags require --hw-quote-evidence-file or --print-hw-quote-binding"
                    .into(),
            );
        }
        build_tier1_attestation_report(&Tier1AttestationOptions {
            identity,
            runtime_keypair,
            provider_signing_seed,
            binary_path,
            boot_epoch,
            report_ts,
            nonce_u: args.nonce_u,
            runtime_config,
        })?
    };

    if report.report.enclave_id != computed_enclave_id {
        return Err(format!(
            "computed enclave_id {} does not match expected {}",
            report.report.enclave_id, computed_enclave_id
        )
        .into());
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("attestation ok");
        println!("enclave_id={}", report.report.enclave_id);
        println!("enclave_pubkey={}", report.report.enclave_pubkey);
        println!("provider_pubkey={}", report.report.provider_pubkey);
        println!("binary_hash={}", report.report.binary_hash);
        println!("manifest_hash={}", report.report.manifest_hash);
        println!("report_head={}", report.report_head);
    }
    Ok(())
}

fn parse_runtime_config(
    runtime_config_json: Option<&str>,
) -> Result<AttestationRuntimeConfig, Box<dyn std::error::Error>> {
    match runtime_config_json {
        Some(json) => Ok(serde_json::from_str(json)?),
        None => Ok(AttestationRuntimeConfig::default()),
    }
}

fn sandbox_profile(args: SandboxProfileArgs) -> Result<(), Box<dyn std::error::Error>> {
    let config = SandboxConfig::new(args.sealed_store, args.ipc_socket);
    let platform = match args.platform {
        PlatformArg::Current => mayhem_enclave::current_sandbox_platform()?,
        PlatformArg::Linux => SandboxPlatform::LinuxSeccompBpf,
        PlatformArg::Macos => SandboxPlatform::MacosSandboxExec,
        PlatformArg::Windows => SandboxPlatform::WindowsAppContainer,
    };
    let profile = build_sandbox_profile(&config, platform)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&profile)?);
    } else {
        println!("{}", profile.policy);
    }
    Ok(())
}

fn sandbox_run(args: SandboxRunArgs) -> Result<i32, Box<dyn std::error::Error>> {
    let config = SandboxConfig::new(args.sealed_store, args.ipc_socket);
    let report = run_sandboxed_command(&config, &args.command)?;
    Ok(report
        .status_code
        .unwrap_or(if report.success { 0 } else { 1 }))
}

fn sandbox_exec_child(args: SandboxExecChildArgs) -> Result<i32, Box<dyn std::error::Error>> {
    let config = SandboxConfig::new(args.sealed_store, args.ipc_socket);
    exec_sandboxed_child(&config, &args.command)?;
    Ok(1)
}

fn sandbox_probe_tcp(args: SandboxProbeTcpArgs) -> Result<(), Box<dyn std::error::Error>> {
    let timeout = Duration::from_millis(args.timeout_ms);
    let report = if args.expect_denied {
        expect_outbound_tcp_denied(&args.addr, timeout)?
    } else {
        probe_outbound_tcp(&args.addr, timeout)
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if report.connected {
        println!("outbound tcp connected: {}", report.addr);
    } else if report.denied {
        println!(
            "outbound tcp denied: {} kind={:?} raw_os_error={:?}",
            report.addr, report.error_kind, report.raw_os_error
        );
    } else {
        println!(
            "outbound tcp failed: {} kind={:?} raw_os_error={:?}",
            report.addr, report.error_kind, report.raw_os_error
        );
    }
    Ok(())
}

fn sandbox_probe_store_write(
    args: SandboxProbeStoreWriteArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = args.sealed_store.join(format!(
        ".mayhem-sandbox-write-probe-{}",
        std::process::id()
    ));
    let mut wrote = false;
    let mut denied = false;
    let mut error_kind = None;
    let mut raw_os_error = None;
    let mut restore_succeeded = false;
    let mut restore_denied = false;
    let mut restore_error_kind = None;
    let mut restore_raw_os_error = None;

    if args.attempt_restore_permissions {
        match try_restore_sealed_store_permissions(&args.sealed_store) {
            Ok(()) => {
                restore_succeeded = true;
            }
            Err(err) => {
                restore_denied = fs_denied_error(&err);
                restore_error_kind = Some(format!("{:?}", err.kind()));
                restore_raw_os_error = err.raw_os_error();
            }
        }
    }

    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            wrote = true;
            file.write_all(b"write-probe")?;
            let _ = fs::remove_file(&path);
        }
        Err(err) => {
            denied = fs_denied_error(&err);
            error_kind = Some(format!("{:?}", err.kind()));
            raw_os_error = err.raw_os_error();
        }
    }

    if args.expect_denied && !denied {
        return Err(format!(
            "sealed-store write was not denied: wrote={wrote} error_kind={error_kind:?} raw_os_error={raw_os_error:?}"
        )
        .into());
    }

    let report = serde_json::json!({
        "path": path,
        "wrote": wrote,
        "denied": denied,
        "error_kind": error_kind,
        "raw_os_error": raw_os_error,
        "restore_attempted": args.attempt_restore_permissions,
        "restore_succeeded": restore_succeeded,
        "restore_denied": restore_denied,
        "restore_error_kind": restore_error_kind,
        "restore_raw_os_error": restore_raw_os_error,
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if denied {
        println!("sealed-store write denied: {}", report["path"]);
    } else if wrote {
        println!(
            "sealed-store write succeeded unexpectedly: {}",
            report["path"]
        );
    } else {
        println!(
            "sealed-store write failed without denial: {}",
            report["path"]
        );
    }
    Ok(())
}

fn fs_denied_error(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::PermissionDenied
        || matches!(err.raw_os_error(), Some(1 | 13 | 30))
}

fn try_restore_sealed_store_permissions(path: &PathBuf) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(permissions.mode() | 0o700);
        fs::set_permissions(path, permissions)
    }

    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)
    }
}

fn boot_check(args: BootCheckArgs) -> Result<(), Box<dyn std::error::Error>> {
    let context = KeyContext {
        provider_id: args.provider_id,
        enclave_id: args.enclave_id,
        artifact_root: args.artifact_root.clone(),
        manifest_hash: args.manifest_hash,
    };
    let mut options = BootOptions::new(
        args.sealed_store,
        context,
        hex_secret(&args.provider_secret_hex)?,
    );
    options.output_path = args.output;
    options.expected_merkle_root = Some(args.artifact_root);
    let report = boot_sealed_store(&options)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "boot check ok: {} bytes in {} chunks",
            report.total_bytes, report.chunk_count
        );
        println!("artifact_root={}", report.merkle_root);
    }
    Ok(())
}
