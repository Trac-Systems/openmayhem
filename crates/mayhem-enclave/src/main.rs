#![forbid(unsafe_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use mayhem_enclave::{
    boot_sealed_store, build_merkle_manifest, hex_secret, seal_artifact, BootOptions, KeyContext,
    SealOptions, DEFAULT_CHUNK_SIZE,
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
