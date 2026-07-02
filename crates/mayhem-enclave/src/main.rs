#![forbid(unsafe_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use mayhem_enclave::{
    boot_sealed_store, build_merkle_manifest, build_tier1_attestation_report, hex_secret,
    load_or_create_runtime_keypair_store, measure_binary, measure_current_binary,
    provider_signing_seed_from_hex, seal_artifact, unix_timestamp_now, BootOptions, KeyContext,
    RuntimeKeyContext, RuntimeKeypairStoreOptions, SealOptions, Tier1AttestationOptions,
    DEFAULT_CHUNK_SIZE,
};
use mayhem_proto::CatalogEnclaveIdentity;

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
    Attest(AttestArgs),
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

    /// Admin-created enclave identity expected for this report.
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

    /// Print a machine-readable report.
    #[arg(long)]
    json: bool,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Commands::SealLocal(args) => seal_local(args),
        Commands::BootCheck(args) => boot_check(args),
        Commands::MeasureBinary(args) => measure_binary_command(args),
        Commands::InitKeypair(args) => init_keypair(args),
        Commands::Attest(args) => attest(args),
    }
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
    let context = RuntimeKeyContext {
        provider_id: args.provider_id,
        enclave_id: args.enclave_id.clone(),
    };
    let keypair_options = RuntimeKeypairStoreOptions::new(
        args.keypair_store,
        context,
        hex_secret(&args.provider_secret_hex)?,
    );
    let runtime_keypair = load_or_create_runtime_keypair_store(&keypair_options)?;
    let binary_path = match args.binary {
        Some(path) => path,
        None => std::env::current_exe()?,
    };
    let report = build_tier1_attestation_report(&Tier1AttestationOptions {
        identity: CatalogEnclaveIdentity {
            admin_pubkey: args.admin_pubkey,
            model_id: args.model_id,
            artifact_root: args.artifact_root,
            manifest_hash: args.manifest_hash,
            binary_hash: String::new(),
        },
        runtime_keypair,
        provider_signing_seed: provider_signing_seed_from_hex(&args.provider_signing_seed_hex)?,
        binary_path,
        boot_epoch: args.boot_epoch.unwrap_or(now),
        report_ts: args.report_ts.unwrap_or(now),
        nonce_u: args.nonce_u,
    })?;

    if report.report.enclave_id != args.enclave_id {
        return Err(format!(
            "computed enclave_id {} does not match expected {}",
            report.report.enclave_id, args.enclave_id
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
