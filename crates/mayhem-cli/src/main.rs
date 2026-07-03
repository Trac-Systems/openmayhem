#![forbid(unsafe_code)]

mod catalog;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use mayhem_bridge::{
    BridgeError, PeerRpcClient, ScBridgeClient, ScBridgeConfig, DEFAULT_RPC_URL,
    DEFAULT_SC_BRIDGE_URL,
};
use mayhem_enclave::{
    boot_sealed_store, build_merkle_manifest, download_resumable,
    finalize_tier1_attestation_report, load_or_create_runtime_keypair_store, measure_binary,
    prepare_tier1_attestation_report, seal_artifact, BootOptions, DownloadReport, DownloadRequest,
    DownloadSource, KeyContext, RuntimeKeyContext, RuntimeKeypairStoreOptions, SealOptions,
    Tier1AttestationReport, Tier1ExternalProviderAttestationOptions, DEFAULT_CHUNK_SIZE,
    SEALED_STORE_MANIFEST,
};
use mayhem_engine::{
    EngineBackend, GenerateRequest, GrammarSpec, LoadConfig, ModelArtifact, ToolSpec,
};
use mayhem_gateway::{
    heartbeat_signing_payload,
    openai::{
        serve as serve_gateway, GatewayModel, GatewayRouteCandidate, GatewayState, MayhemModelInfo,
        ModelCaps, PriceRefMu, ScBridgeGatewaySessionBackend, ScBridgeGatewaySessionConfig,
    },
};
use mayhem_hwprobe::{
    human_report, probe, BackendVerdict, FixtureProfile, HardwareReport, ProbeOptions,
    VerdictStatus,
};
use mayhem_proto::CatalogEnclaveIdentity;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::process::Command;
use tokio::time::sleep;

const DEFAULT_GATEWAY_URL: &str = "http://127.0.0.1:11435";
const DEFAULT_PAYGATE_URL: &str = "http://127.0.0.1:11436";
const OPENCODE_PROVIDER_ID: &str = "mayhem";
const OPENCODE_PROVIDER_NAME: &str = "Mayhem P2P";
const OPENCODE_PROVIDER_NPM: &str = "@ai-sdk/openai-compatible";
const OPENCODE_SCHEMA_URL: &str = "https://opencode.ai/config.json";
const OPENCODE_TEST_MARKER: &str = "mayhem-opencode-tool-ok";
const DEFAULT_EPOCH_LENGTH_MILLIS: u64 = 3_600_000;

#[derive(Debug, Parser)]
#[command(name = "mayhem")]
#[command(about = "Mayhem network CLI")]
#[command(arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Choose a role, create or import a wallet, and sign current router rules.
    Setup(SetupArgs),
    /// Probe local hardware and print enclave backend feasibility.
    Doctor(DoctorArgs),
    /// Inspect and verify the admin-signed model catalog.
    Catalog {
        #[command(subcommand)]
        command: CatalogCommands,
    },
    /// Provider serving lifecycle commands.
    Provider {
        #[command(subcommand)]
        command: Box<ProviderCommands>,
    },
    /// Start the local OpenAI-compatible user gateway.
    Use(UseArgs),
    /// List models from the local OpenAI-compatible gateway.
    Models(ModelsArgs),
    /// Buy Mayhem credits through fiat/crypto rails.
    Pay {
        #[command(subcommand)]
        command: PayCommands,
    },
    /// Show a canonical contract credit balance.
    Balance(BalanceArgs),
    /// Show provider payout evidence and treasury fee sweeps.
    Payouts(PayoutsArgs),
    /// Show provider earnings, holdback, paid, and released balances.
    Earnings(EarningsArgs),
    /// Auditor probe commands.
    Auditor {
        #[command(subcommand)]
        command: AuditorCommands,
    },
    /// Receipt audit commands.
    Receipts {
        #[command(subcommand)]
        command: ReceiptsCommands,
    },
    /// Inspect, hash, and re-consent to router rules.
    Rules {
        #[command(subcommand)]
        command: RulesCommands,
    },
    /// Run a gateway, peer, opencode, and receipt smoke test.
    Test(TestArgs),
}

#[derive(Debug, Subcommand)]
enum ProviderCommands {
    /// Pick an admin-created enclave, seal its artifact, join canonical rooms, and send heartbeats.
    Start(ProviderStartArgs),
}

#[derive(Debug, Subcommand)]
enum PayCommands {
    /// Buy credits via Stripe hosted checkout.
    Stripe(PayRailArgs),
    /// Buy credits via Coinbase hosted checkout.
    Coinbase(PayRailArgs),
}

#[derive(Debug, Subcommand)]
enum RulesCommands {
    /// Print the BLAKE3 hash of RULES.md.
    Hash(RulesHashArgs),
    /// Review current rules and sign fresh consent when needed.
    Review(RulesReviewArgs),
}

#[derive(Debug, Subcommand)]
enum ReceiptsCommands {
    /// Export an epoch audit bundle and verify it against ev/* roots.
    Export(ReceiptsExportArgs),
    /// Publish gateway receipts onto the canonical epoch sidechannel.
    Publish(ReceiptsPublishArgs),
    /// Collect epoch sidechannel receipts into a receipts-file compatible JSON bundle.
    Collect(ReceiptsCollectArgs),
}

#[derive(Debug, Subcommand)]
enum AuditorCommands {
    /// Run a canary probe through the normal gateway chat path and emit/submit probe_result evidence.
    Canary(AuditorCanaryArgs),
}

#[derive(Debug, Subcommand)]
enum CatalogCommands {
    /// Verify catalog structure, maintainer signature, canary refs, and optional dev downloads.
    Verify(CatalogVerifyArgs),
    /// Run catalog canaries against a local admin artifact and print catalog-ready fingerprints.
    CalibrateCanary(CatalogCalibrateCanaryArgs),
}

#[derive(Debug, Parser)]
struct PayoutsArgs {
    /// Peer JSON-RPC base URL, including /v1. Defaults to config.toml or local dev-net.
    #[arg(long)]
    rpc_url: Option<String>,

    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Show one payout evidence epoch.
    #[arg(long)]
    epoch: Option<u64>,

    /// Print machine-readable payout evidence.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct EarningsArgs {
    /// Peer JSON-RPC base URL, including /v1. Defaults to config.toml or local dev-net.
    #[arg(long)]
    rpc_url: Option<String>,

    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Restrict output to one provider public key.
    #[arg(long)]
    provider: Option<String>,

    /// Print machine-readable earnings.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct BalanceArgs {
    /// Peer JSON-RPC base URL, including /v1. Defaults to config.toml or local dev-net.
    #[arg(long)]
    rpc_url: Option<String>,

    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Intercom peer store name under <home>/stores when config.toml has no identity store.
    #[arg(long, default_value = "main")]
    peer_store_name: String,

    /// Password for the encrypted keypair.json. Empty by default.
    #[arg(long)]
    wallet_password: Option<String>,

    /// Public key to inspect. Defaults to the local wallet public key.
    #[arg(long)]
    who: Option<String>,

    /// Print a machine-readable balance report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct UseArgs {
    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Peer JSON-RPC base URL, including /v1. Defaults to config.toml or local dev-net.
    #[arg(long)]
    rpc_url: Option<String>,

    /// SC-Bridge websocket URL for direct provider sessions.
    #[arg(long)]
    sc_bridge_url: Option<String>,

    /// SC-Bridge token for direct provider sessions.
    #[arg(long)]
    sc_bridge_token: Option<String>,

    /// Address to bind, for example 127.0.0.1:11435. Defaults to 127.0.0.1:<port>.
    #[arg(long)]
    bind: Option<String>,

    /// Loopback port for the gateway when --bind is not provided.
    #[arg(long, default_value_t = 11_435)]
    port: u16,

    /// Print a machine-readable startup report before serving.
    #[arg(long)]
    json: bool,

    /// Development smoke only: use the embedded catalog instead of contract-backed canonical models.
    #[arg(long)]
    dev_embedded_catalog: bool,
}

#[derive(Debug, Parser)]
struct ModelsArgs {
    /// Gateway base URL. Defaults to config.toml, MAYHEM_GATEWAY_URL, or local gateway.
    #[arg(long)]
    gateway_url: Option<String>,

    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// HTTP timeout in seconds for gateway calls.
    #[arg(long, default_value_t = 30)]
    timeout_seconds: u64,

    /// Print a machine-readable model list.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct AuditorCanaryArgs {
    /// Gateway base URL. Defaults to config.toml, MAYHEM_GATEWAY_URL, or local gateway.
    #[arg(long)]
    gateway_url: Option<String>,

    /// Peer JSON-RPC base URL, including /v1. Required only with --submit.
    #[arg(long)]
    rpc_url: Option<String>,

    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Intercom peer store name under <home>/stores when config.toml has no identity store.
    #[arg(long, default_value = "main")]
    peer_store_name: String,

    /// Password for the encrypted auditor keypair.json. Empty by default.
    #[arg(long)]
    wallet_password: Option<String>,

    /// Model id to probe. Defaults to the first gateway model.
    #[arg(long)]
    model: Option<String>,

    /// Canary set id under catalog/canaries.
    #[arg(long, default_value = "canary-dev-v1")]
    canary_set: String,

    /// Canary prompt id. Defaults to the first prompt in the set.
    #[arg(long)]
    prompt_id: Option<String>,

    /// Directory containing canary set JSON files.
    #[arg(long, value_name = "PATH")]
    canaries_dir: Option<PathBuf>,

    /// Expected canary response text.
    #[arg(long)]
    expected_text: Option<String>,

    /// File containing expected canary response text.
    #[arg(long, value_name = "PATH")]
    expected_file: Option<PathBuf>,

    /// Provider public key. Defaults to the provider in the latest gateway receipt.
    #[arg(long)]
    provider: Option<String>,

    /// Admin-created enclave id. Defaults to the enclave in the latest gateway receipt.
    #[arg(long)]
    enclave_id: Option<String>,

    /// Epoch used in the probe_result command.
    #[arg(long)]
    epoch: u64,

    /// Probe timestamp in Unix seconds. Defaults to current time.
    #[arg(long)]
    at: Option<u64>,

    /// Minimum text-position match in basis points.
    #[arg(long, default_value_t = 9_000)]
    min_match_bps: u32,

    /// Override the generated probe id.
    #[arg(long)]
    probe_id: Option<String>,

    /// Write the full probe evidence bundle to this path.
    #[arg(long, value_name = "PATH")]
    evidence_output: Option<PathBuf>,

    /// Submit probe_result to the contract using the auditor wallet.
    #[arg(long)]
    submit: bool,

    /// Simulate the contract transaction when --submit is set.
    #[arg(long)]
    sim: bool,

    /// HTTP timeout in seconds for gateway calls.
    #[arg(long, default_value_t = 60)]
    timeout_seconds: u64,

    /// Print a machine-readable probe report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct ReceiptsExportArgs {
    /// Epoch to export and verify.
    #[arg(long)]
    epoch: u64,

    /// Peer JSON-RPC base URL, including /v1. Defaults to config.toml or local dev-net.
    #[arg(long)]
    rpc_url: Option<String>,

    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Gateway base URL used to read /mayhem/receipts when --receipts-file is omitted.
    #[arg(long)]
    gateway_url: Option<String>,

    /// Write the audit bundle to this file. Defaults to stdout.
    #[arg(long, value_name = "PATH")]
    output: Option<PathBuf>,

    /// Read receipt entries from a JSON file instead of the gateway.
    #[arg(long, value_name = "PATH")]
    receipts_file: Option<PathBuf>,

    /// Read deposit entries from a JSON file.
    #[arg(long, value_name = "PATH")]
    deposits_file: Option<PathBuf>,

    /// Read payout entries from a JSON file.
    #[arg(long, value_name = "PATH")]
    payouts_file: Option<PathBuf>,

    /// Read prior provider cumulative earnings as a JSON object.
    #[arg(long, value_name = "PATH")]
    prior_earnings_file: Option<PathBuf>,

    /// Read ev/* records from a JSON file instead of peer RPC.
    #[arg(long, value_name = "PATH")]
    evidence_file: Option<PathBuf>,

    /// Fee split in basis points for the independent root recompute.
    #[arg(long, default_value_t = 1_500)]
    fee_bps: u64,

    /// Prior fee/cum.mu before this epoch.
    #[arg(long, default_value_t = 0)]
    prior_fee_cum_mu: u64,

    /// Path to intercom/scripts/recompute-epoch-roots.mjs.
    #[arg(long, value_name = "PATH")]
    verifier_script: Option<PathBuf>,

    /// Only write the bundle; do not run independent root verification.
    #[arg(long)]
    no_verify: bool,

    /// Print a machine-readable export report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct ReceiptsPublishArgs {
    /// Epoch sidechannel to publish to. Defaults to mx/epoch/<epoch>.
    #[arg(long)]
    epoch: u64,

    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Gateway base URL used to read /mayhem/receipts.
    #[arg(long)]
    gateway_url: Option<String>,

    /// SC-Bridge websocket URL. Defaults to config.toml, env, or local dev-net.
    #[arg(long)]
    sc_bridge_url: Option<String>,

    /// SC-Bridge token.
    #[arg(long)]
    sc_bridge_token: Option<String>,

    /// Override the sidechannel name.
    #[arg(long)]
    channel: Option<String>,

    /// Keep polling the gateway and publish newly observed receipts.
    #[arg(long)]
    watch: bool,

    /// Poll interval while --watch is active.
    #[arg(long, default_value_t = 2_000)]
    poll_interval_ms: u64,

    /// Stop after publishing this many unique receipts.
    #[arg(long)]
    max_receipts: Option<usize>,

    /// Maximum seconds to watch. Zero means no time limit.
    #[arg(long, default_value_t = 0)]
    timeout_seconds: u64,

    /// Print a machine-readable publish report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct ReceiptsCollectArgs {
    /// Epoch sidechannel to collect from. Defaults to mx/epoch/<epoch>.
    #[arg(long)]
    epoch: u64,

    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// SC-Bridge websocket URL. Defaults to config.toml, env, or local dev-net.
    #[arg(long)]
    sc_bridge_url: Option<String>,

    /// SC-Bridge token.
    #[arg(long)]
    sc_bridge_token: Option<String>,

    /// Override the sidechannel name.
    #[arg(long)]
    channel: Option<String>,

    /// Write collected receipts to this JSON file. Defaults to stdout.
    #[arg(long, value_name = "PATH")]
    output: Option<PathBuf>,

    /// Stop after collecting this many unique receipts.
    #[arg(long)]
    max_receipts: Option<usize>,

    /// Maximum seconds to collect before writing what was observed.
    #[arg(long, default_value_t = 60)]
    timeout_seconds: u64,

    /// Print a machine-readable collection report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct PayRailArgs {
    /// USD amount to buy, for example 10 or 10.25.
    #[arg(long)]
    amount: String,

    /// Paygate base URL. Defaults to config.toml, MAYHEM_PAYGATE_URL, or local paygate.
    #[arg(long)]
    paygate_url: Option<String>,

    /// Peer JSON-RPC base URL, including /v1. Defaults to config.toml or local dev-net.
    #[arg(long)]
    rpc_url: Option<String>,

    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Intercom peer store name under <home>/stores when config.toml has no identity store.
    #[arg(long, default_value = "main")]
    peer_store_name: String,

    /// Password for the encrypted keypair.json. Empty by default.
    #[arg(long)]
    wallet_password: Option<String>,

    /// Idempotency key forwarded when the selected rail supports it.
    #[arg(long)]
    idempotency_key: Option<String>,

    /// Success redirect URL for hosted checkout.
    #[arg(long)]
    success_url: Option<String>,

    /// Cancel/failure redirect URL for hosted checkout.
    #[arg(long)]
    cancel_url: Option<String>,

    /// Print the checkout URL but do not launch a browser.
    #[arg(long)]
    no_open: bool,

    /// Do not wait for the contract ledger balance to reflect the credit.
    #[arg(long)]
    no_wait: bool,

    /// Maximum seconds to wait for ledger credit.
    #[arg(long, default_value_t = 900)]
    timeout_seconds: u64,

    /// Poll interval in milliseconds while waiting for ledger credit.
    #[arg(long, default_value_t = 2_000)]
    poll_interval_ms: u64,

    /// Print a machine-readable payment report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct TestArgs {
    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Gateway base URL. Defaults to config.toml, MAYHEM_GATEWAY_URL, or local gateway.
    #[arg(long)]
    gateway_url: Option<String>,

    /// Peer JSON-RPC base URL, including /v1. Defaults to config.toml or local dev-net.
    #[arg(long)]
    rpc_url: Option<String>,

    /// Model id to test. Defaults to the first tool-capable /v1/models entry.
    #[arg(long)]
    model: Option<String>,

    /// Refresh the Mayhem provider models in opencode.json from /v1/models.
    #[arg(long)]
    sync_models: bool,

    /// Path to opencode.json. Defaults to MAYHEM_OPENCODE_CONFIG or ~/.config/opencode/opencode.json.
    #[arg(long, value_name = "PATH")]
    opencode_config: Option<PathBuf>,

    /// Path to the opencode binary. Defaults to <home>/bin/opencode or PATH.
    #[arg(long, value_name = "PATH")]
    opencode_bin: Option<PathBuf>,

    /// Skip the peer RPC health check; useful for isolated gateway/opencode smoke tests.
    #[arg(long)]
    skip_peer_health: bool,

    /// Do not merge or run opencode.
    #[arg(long)]
    skip_opencode: bool,

    /// Merge opencode.json but do not execute opencode run.
    #[arg(long)]
    no_opencode_run: bool,

    /// Maximum seconds for individual HTTP/opencode checks.
    #[arg(long, default_value_t = 60)]
    timeout_seconds: u64,

    /// Print a machine-readable test report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct SetupArgs {
    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Node role for this install.
    #[arg(long, value_enum)]
    role: Option<Role>,

    /// Wallet setup mode.
    #[arg(long, value_enum, default_value_t = WalletMode::Auto)]
    wallet: WalletMode,

    /// BIP-39 mnemonic to import, or to use for deterministic test creation.
    #[arg(long)]
    mnemonic: Option<String>,

    /// Password for the encrypted keypair.json. Empty by default.
    #[arg(long)]
    wallet_password: Option<String>,

    /// Intercom peer store name under <home>/stores.
    #[arg(long, default_value = "main")]
    peer_store_name: String,

    /// Peer JSON-RPC base URL, including /v1.
    #[arg(long)]
    rpc_url: Option<String>,

    /// Skip router-rules consent.
    #[arg(long)]
    no_consent: bool,

    /// Dry-run consent through the contract simulator; does not persist state.
    #[arg(long)]
    sim: bool,

    /// Rules version to consent to. If omitted, read rules/current from RPC.
    #[arg(long)]
    rules_ver: Option<u64>,

    /// Rules hash to consent to. Must be paired with --rules-ver.
    #[arg(long)]
    rules_hash: Option<String>,

    /// Path to RULES.md. Defaults to the repo root RULES.md.
    #[arg(long, value_name = "PATH")]
    rules_path: Option<PathBuf>,

    /// Accept the displayed rules without an interactive prompt.
    #[arg(long)]
    yes: bool,

    /// Overwrite an existing keypair when using --wallet create/import.
    #[arg(long)]
    force: bool,

    /// Print a machine-readable setup report.
    #[arg(long)]
    print_json: bool,
}

#[derive(Debug, Parser)]
struct RulesHashArgs {
    /// Path to RULES.md. Defaults to the repo root RULES.md.
    #[arg(long, value_name = "PATH")]
    rules_path: Option<PathBuf>,

    /// Print a machine-readable hash report.
    #[arg(long)]
    print_json: bool,
}

#[derive(Debug, Parser)]
struct RulesReviewArgs {
    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Intercom peer store name under <home>/stores when config.toml is absent.
    #[arg(long, default_value = "main")]
    peer_store_name: String,

    /// Password for the encrypted keypair.json. Empty by default.
    #[arg(long)]
    wallet_password: Option<String>,

    /// Peer JSON-RPC base URL, including /v1. Defaults to config.toml or the bridge default.
    #[arg(long)]
    rpc_url: Option<String>,

    /// Path to RULES.md. Defaults to the repo root RULES.md.
    #[arg(long, value_name = "PATH")]
    rules_path: Option<PathBuf>,

    /// Accept the displayed rules without an interactive prompt.
    #[arg(long)]
    yes: bool,

    /// Dry-run consent through the contract simulator; does not persist state.
    #[arg(long)]
    sim: bool,

    /// Print a machine-readable review report.
    #[arg(long)]
    print_json: bool,
}

#[derive(Debug, Parser)]
struct DoctorArgs {
    /// Print the hardware report as JSON.
    #[arg(long)]
    json: bool,

    /// Run against a deterministic reference profile: apple-silicon, linux-nvidia, or cpu-only.
    #[arg(long)]
    fixture: Option<String>,

    /// Path used for disk free-space and write-throughput probes.
    #[arg(long, value_name = "PATH")]
    disk_path: Option<PathBuf>,

    /// Skip the disk write-throughput benchmark.
    #[arg(long)]
    skip_disk_bench: bool,

    /// Size of the temporary disk benchmark write.
    #[arg(long, default_value_t = 16)]
    disk_bench_mib: u64,
}

#[derive(Debug, Parser)]
struct CatalogVerifyArgs {
    /// Path to catalog/models.json. Defaults to the repo catalog.
    #[arg(long, value_name = "PATH")]
    catalog_path: Option<PathBuf>,

    /// Path to the detached catalog signature JSON.
    #[arg(long, value_name = "PATH")]
    signature_path: Option<PathBuf>,

    /// Directory containing catalog maintainer public keys.
    #[arg(long, value_name = "PATH")]
    keys_dir: Option<PathBuf>,

    /// Directory containing canary set JSON files.
    #[arg(long, value_name = "PATH")]
    canaries_dir: Option<PathBuf>,

    /// Probe ranged downloads for dev artifacts marked download_check=true.
    #[arg(long)]
    check_dev_downloads: bool,

    /// Hugging Face token file used only for --check-dev-downloads.
    #[arg(long, value_name = "PATH")]
    hf_token_file: Option<PathBuf>,

    /// Print a machine-readable verification report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct CatalogCalibrateCanaryArgs {
    /// Path to catalog/models.json. Defaults to the repo catalog.
    #[arg(long, value_name = "PATH")]
    catalog_path: Option<PathBuf>,

    /// Directory containing canary set JSON files.
    #[arg(long, value_name = "PATH")]
    canaries_dir: Option<PathBuf>,

    /// Catalog model id to calibrate.
    #[arg(long)]
    model: String,

    /// Artifact key in the model's artifacts map, e.g. gguf-q4_k_m or mlx-4bit.
    #[arg(long)]
    artifact: String,

    /// Local artifact file or snapshot path. It must match the admin catalog artifact.
    #[arg(long, value_name = "PATH")]
    artifact_path: PathBuf,

    /// Restrict calibration to one prompt id. Defaults to all prompts in the canary set.
    #[arg(long)]
    prompt_id: Option<String>,

    /// Engine context size for the calibration run.
    #[arg(long, default_value_t = 1024)]
    ctx_size: u32,

    /// Optional llama.cpp thread count.
    #[arg(long)]
    threads: Option<i32>,

    /// Optional GPU layer count for llama.cpp calibration.
    #[arg(long)]
    gpu_layers: Option<u32>,

    /// Seed for deterministic calibration requests.
    #[arg(long, default_value_t = 0)]
    seed: u32,

    /// Include raw generated text in the report.
    #[arg(long)]
    include_output: bool,

    /// Print a machine-readable calibration report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct ProviderStartArgs {
    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Admin-created model id or enclave id. If omitted, the first feasible admin enclave is used.
    #[arg(long)]
    enclave: Option<String>,

    /// Canonical room ids to join, comma-separated, or auto for all open admin rooms for the model.
    #[arg(long, default_value = "auto")]
    rooms: String,

    /// Peer JSON-RPC base URL, including /v1. Defaults to config.toml or the bridge default.
    #[arg(long)]
    rpc_url: Option<String>,

    /// SC-Bridge websocket URL for the local provider peer. Required for live heartbeats.
    #[arg(long)]
    sc_bridge_url: Option<String>,

    /// SC-Bridge token for the local provider peer.
    #[arg(long)]
    sc_bridge_token: Option<String>,

    /// Password for the encrypted keypair.json. Empty by default.
    #[arg(long)]
    wallet_password: Option<String>,

    /// Path to catalog/models.json. Defaults to the repo catalog.
    #[arg(long, value_name = "PATH")]
    catalog_path: Option<PathBuf>,

    /// Path to the detached catalog signature JSON.
    #[arg(long, value_name = "PATH")]
    signature_path: Option<PathBuf>,

    /// Directory containing catalog maintainer public keys.
    #[arg(long, value_name = "PATH")]
    keys_dir: Option<PathBuf>,

    /// Directory containing canary set JSON files.
    #[arg(long, value_name = "PATH")]
    canaries_dir: Option<PathBuf>,

    /// Use a local copy of the admin catalog artifact; it must match the enclave artifact_root.
    #[arg(long, value_name = "PATH")]
    artifact: Option<PathBuf>,

    /// Directory for downloaded/plain artifacts. Defaults to <home>/downloads.
    #[arg(long, value_name = "PATH")]
    downloads_dir: Option<PathBuf>,

    /// Hugging Face token file used when downloading a Hugging Face artifact.
    #[arg(long, value_name = "PATH")]
    hf_token_file: Option<PathBuf>,

    /// Override backend selection: auto, trt-llm, mlx, or llama.cpp.
    #[arg(long, default_value = "auto")]
    engine_backend: String,

    /// Run hwprobe against a deterministic reference profile.
    #[arg(long)]
    fixture: Option<String>,

    /// Path used for disk free-space and write-throughput probes.
    #[arg(long, value_name = "PATH")]
    disk_path: Option<PathBuf>,

    /// Skip the disk write-throughput benchmark.
    #[arg(long)]
    skip_disk_bench: bool,

    /// Chunk size for download verification and sealing.
    #[arg(long, default_value_t = DEFAULT_CHUNK_SIZE)]
    chunk_size: usize,

    /// Binary path to measure for Tier-1 attestation. Defaults to the running mayhem binary.
    #[arg(long, value_name = "PATH")]
    enclave_binary: Option<PathBuf>,

    /// Dry-run the provider registration/join txs through contract simulation.
    #[arg(long)]
    sim: bool,

    /// Do not connect to SC-Bridge or emit room heartbeats.
    #[arg(long)]
    no_heartbeat: bool,

    /// Number of heartbeat frames to send to each joined room.
    #[arg(long, default_value_t = 1)]
    heartbeat_count: u32,

    /// Keep running and answer direct mx/s/<session_id> requests over SC-Bridge.
    #[arg(long)]
    serve_sessions: bool,

    /// Stop session serving after this many seconds; 0 means run until killed.
    #[arg(long, default_value_t = 0)]
    serve_sessions_seconds: u64,

    /// Development-only: use deterministic session responses instead of the loaded engine.
    #[arg(long, hide = true)]
    dev_session_shim: bool,

    /// Print a machine-readable provider start report.
    #[arg(long)]
    print_json: bool,

    /// Development-only: load a local catalog fixture without verifying its detached signature.
    #[arg(long, hide = true)]
    dev_skip_catalog_verify: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Role {
    Provider,
    User,
    Both,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::User => "user",
            Self::Both => "both",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum WalletMode {
    /// Reuse an existing wallet, otherwise create one.
    Auto,
    /// Create a fresh wallet. Fails if keypair.json exists unless --force is used.
    Create,
    /// Import from --mnemonic. Fails if keypair.json exists unless --force is used.
    Import,
    /// Reuse an existing keypair.json.
    Reuse,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum PayRail {
    Stripe,
    Coinbase,
}

impl PayRail {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stripe => "stripe",
            Self::Coinbase => "coinbase",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct WalletInfo {
    created: bool,
    keypair_path: String,
    public_key: String,
    address: Option<String>,
    derivation_path: Option<String>,
    mnemonic: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SignOutput {
    signature: String,
}

#[derive(Debug, Clone, Serialize)]
struct RulesRef {
    ver: u64,
    hash: String,
}

#[derive(Debug, Serialize)]
struct ConsentReport {
    skipped: bool,
    simulated: bool,
    rules: Option<RulesRef>,
    tx: Option<String>,
    command_hash: Option<String>,
    result: Option<Value>,
    state: Option<Value>,
}

#[derive(Debug, Clone)]
struct RulesDoc {
    path: PathBuf,
    text: String,
    hash: String,
    bytes: usize,
}

#[derive(Debug, Deserialize)]
struct MayhemConfig {
    identity: Option<ConfigIdentity>,
    network: Option<ConfigNetwork>,
    provider: Option<ConfigProvider>,
    role: Option<ConfigRole>,
}

#[derive(Debug, Deserialize)]
struct ConfigIdentity {
    keypair_path: Option<String>,
    store_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConfigNetwork {
    rpc_url: Option<String>,
    sc_bridge_url: Option<String>,
    sc_bridge_token: Option<String>,
    gateway_url: Option<String>,
    paygate_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConfigProvider {
    engine_backend: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConfigRole {
    mode: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Setup(args) => setup(args).await,
        Commands::Doctor(args) => doctor(args),
        Commands::Catalog { command } => match command {
            CatalogCommands::Verify(args) => catalog_verify(args),
            CatalogCommands::CalibrateCanary(args) => catalog_calibrate_canary(args),
        },
        Commands::Provider { command } => match *command {
            ProviderCommands::Start(args) => provider_start(args).await,
        },
        Commands::Use(args) => use_gateway(args).await,
        Commands::Models(args) => models(args).await,
        Commands::Pay { command } => match command {
            PayCommands::Stripe(args) => pay(PayRail::Stripe, args).await,
            PayCommands::Coinbase(args) => pay(PayRail::Coinbase, args).await,
        },
        Commands::Balance(args) => balance(args).await,
        Commands::Payouts(args) => payouts(args).await,
        Commands::Earnings(args) => earnings(args).await,
        Commands::Auditor { command } => match command {
            AuditorCommands::Canary(args) => auditor_canary(args).await,
        },
        Commands::Receipts { command } => match command {
            ReceiptsCommands::Export(args) => receipts_export(args).await,
            ReceiptsCommands::Publish(args) => receipts_publish(args).await,
            ReceiptsCommands::Collect(args) => receipts_collect(args).await,
        },
        Commands::Rules { command } => match command {
            RulesCommands::Hash(args) => rules_hash(args),
            RulesCommands::Review(args) => rules_review(args).await,
        },
        Commands::Test(args) => mayhem_test(args).await,
    }
}

async fn setup(args: SetupArgs) -> Result<()> {
    let home = args.home.clone().map(Ok).unwrap_or_else(default_home)?;
    let home = absolutize(home)?;
    let role = select_role(args.role)?;
    let rpc_url = args
        .rpc_url
        .clone()
        .unwrap_or_else(|| DEFAULT_RPC_URL.to_owned());
    let store_path = home.join("stores").join(&args.peer_store_name);
    let keypair_path = store_path.join("db").join("keypair.json");
    let password = args.wallet_password.clone().unwrap_or_default();

    fs::create_dir_all(&home).with_context(|| format!("creating {}", home.display()))?;
    fs::create_dir_all(keypair_path.parent().expect("keypair has parent"))
        .with_context(|| format!("creating {}", keypair_path.display()))?;

    let wallet = materialize_wallet(&args, &keypair_path, &password).await?;
    let config_path = write_config(
        &home,
        &store_path,
        &wallet,
        role,
        &args.peer_store_name,
        &rpc_url,
    )?;
    let opencode_config = merge_mayhem_opencode_config(
        &default_opencode_config_path()?,
        DEFAULT_GATEWAY_URL,
        None,
        false,
    )?;

    let consent = if args.no_consent {
        ConsentReport {
            skipped: true,
            simulated: args.sim,
            rules: None,
            tx: None,
            command_hash: None,
            result: None,
            state: None,
        }
    } else {
        let rpc = PeerRpcClient::new(&rpc_url)?;
        let rules_doc = read_rules_doc(args.rules_path.as_deref())?;
        let rules = resolve_rules(
            args.rules_ver,
            args.rules_hash.as_deref(),
            &rpc,
            Some(&rules_doc),
        )
        .await?;
        if !args.print_json {
            print_rules_review(&home, &rules_doc, &rules, None, None)?;
        }
        confirm_rules_acceptance(args.yes)?;
        let consent = submit_consent(
            &rpc,
            &keypair_path,
            &password,
            &wallet,
            rules.clone(),
            args.sim,
        )
        .await?;
        if !args.sim {
            persist_rules_acceptance(&home, &rules_doc, &rules)?;
        }
        consent
    };

    let report = json!({
        "home": home,
        "role": role.as_str(),
        "config_path": config_path,
        "wallet": wallet,
        "network": {
            "rpc_url": rpc_url,
            "gateway_url": DEFAULT_GATEWAY_URL,
        },
        "opencode": opencode_config,
        "consent": consent,
    });

    if args.print_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human_report(&report)?;
    }

    Ok(())
}

fn rules_hash(args: RulesHashArgs) -> Result<()> {
    let rules_doc = read_rules_doc(args.rules_path.as_deref())?;
    if args.print_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "rules_path": rules_doc.path,
                "hash": rules_doc.hash,
                "bytes": rules_doc.bytes,
            }))?
        );
    } else {
        println!("{}", rules_doc.hash);
    }
    Ok(())
}

async fn rules_review(args: RulesReviewArgs) -> Result<()> {
    let home = args.home.clone().map(Ok).unwrap_or_else(default_home)?;
    let home = absolutize(home)?;
    let config = read_mayhem_config(&home)?;
    let rpc_url = args
        .rpc_url
        .clone()
        .or_else(|| {
            config
                .as_ref()
                .and_then(|config| config.network.as_ref())
                .and_then(|network| network.rpc_url.clone())
        })
        .unwrap_or_else(|| DEFAULT_RPC_URL.to_owned());
    let store_name = config
        .as_ref()
        .and_then(|config| config.identity.as_ref())
        .and_then(|identity| identity.store_name.clone())
        .unwrap_or_else(|| args.peer_store_name.clone());
    let keypair_path = config
        .as_ref()
        .and_then(|config| config.identity.as_ref())
        .and_then(|identity| identity.keypair_path.as_ref())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            home.join("stores")
                .join(&store_name)
                .join("db")
                .join("keypair.json")
        });
    let password = args.wallet_password.clone().unwrap_or_default();
    let wallet = inspect_wallet(&keypair_path, &password).await?;
    let rpc = PeerRpcClient::new(&rpc_url)?;
    let rules_doc = read_rules_doc(args.rules_path.as_deref())?;
    let rules = resolve_rules(None, None, &rpc, Some(&rules_doc)).await?;
    let prior_consent = read_consent_state(&rpc, &wallet.public_key).await?;
    let already_consented = consent_matches(prior_consent.as_ref(), &rules);
    let previous_rules = read_current_accepted_rules(&home)?;

    if !args.print_json {
        print_rules_review(
            &home,
            &rules_doc,
            &rules,
            previous_rules.as_deref(),
            prior_consent.as_ref(),
        )?;
    }

    let consent = if already_consented && !args.sim {
        if previous_rules.as_deref() != Some(rules_doc.text.as_str()) {
            confirm_rules_acceptance(args.yes)?;
            persist_rules_acceptance(&home, &rules_doc, &rules)?;
        }
        ConsentReport {
            skipped: true,
            simulated: false,
            rules: Some(rules.clone()),
            tx: None,
            command_hash: None,
            result: None,
            state: prior_consent.clone(),
        }
    } else {
        confirm_rules_acceptance(args.yes)?;
        let consent = submit_consent(
            &rpc,
            &keypair_path,
            &password,
            &wallet,
            rules.clone(),
            args.sim,
        )
        .await?;
        if !args.sim {
            persist_rules_acceptance(&home, &rules_doc, &rules)?;
        }
        consent
    };
    let consent_skipped = consent.skipped;
    let consent_simulated = consent.simulated;
    let rules_ver = rules.ver;

    let report = json!({
        "home": home,
        "wallet": {
            "public_key": wallet.public_key,
            "address": wallet.address,
            "keypair_path": wallet.keypair_path,
        },
        "network": {
            "rpc_url": rpc_url,
        },
        "rules": {
            "ver": rules.ver,
            "hash": rules.hash,
            "local_hash": rules_doc.hash,
            "path": rules_doc.path,
            "bytes": rules_doc.bytes,
        },
        "prior_consent": prior_consent,
        "already_consented": already_consented,
        "consent": consent,
    });

    if args.print_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if consent_skipped {
        println!("Consent: already current for rules v{rules_ver}");
    } else if consent_simulated {
        println!("Consent: simulated for rules v{rules_ver}");
    } else {
        println!("Consent: submitted and observed for rules v{rules_ver}");
    }

    Ok(())
}

fn doctor(args: DoctorArgs) -> Result<()> {
    let fixture = args
        .fixture
        .as_deref()
        .map(|value| {
            FixtureProfile::parse(value).with_context(|| {
                format!(
                    "unknown fixture {value}; expected apple-silicon, linux-nvidia, or cpu-only"
                )
            })
        })
        .transpose()?;

    let mut options = ProbeOptions::default();
    if let Some(path) = args.disk_path {
        options.disk_path = absolutize(path)?;
    }
    options.run_disk_bench = !args.skip_disk_bench;
    options.disk_bench_mib = args.disk_bench_mib;
    options.fixture = fixture;

    let report = probe(options);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", human_report(&report));
    }
    Ok(())
}

fn catalog_verify(args: CatalogVerifyArgs) -> Result<()> {
    let catalog_path = args
        .catalog_path
        .map(Ok)
        .unwrap_or_else(|| repo_path("catalog/models.json"))?;
    let catalog_path = absolutize(catalog_path)?;
    let signature_path = args
        .signature_path
        .map(Ok)
        .unwrap_or_else(|| repo_path("catalog/signatures/models.json.sig"))?;
    let signature_path = absolutize(signature_path)?;
    let keys_dir = args
        .keys_dir
        .map(Ok)
        .unwrap_or_else(|| repo_path("catalog/keys"))?;
    let keys_dir = absolutize(keys_dir)?;
    let canaries_dir = args
        .canaries_dir
        .map(Ok)
        .unwrap_or_else(|| repo_path("catalog/canaries"))?;
    let canaries_dir = absolutize(canaries_dir)?;
    let hf_token_file = args.hf_token_file.map(absolutize).transpose()?;

    let report = catalog::verify(catalog::VerifyOptions {
        catalog_path,
        signature_path,
        keys_dir,
        canaries_dir,
        check_dev_downloads: args.check_dev_downloads,
        hf_token_file,
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if report.ok {
        println!(
            "Catalog OK: {} models ({} dev, {} launch), {} artifacts, hash {}",
            report.model_count,
            report.dev_model_count,
            report.launch_model_count,
            report.artifact_count,
            report.catalog_hash
        );
        if !report.download_checks.is_empty() {
            println!("Dev downloads checked: {}", report.download_checks.len());
        }
    } else {
        for error in &report.errors {
            eprintln!("Catalog error: {error}");
        }
        bail!("catalog verification failed");
    }
    Ok(())
}

fn catalog_calibrate_canary(args: CatalogCalibrateCanaryArgs) -> Result<()> {
    let catalog_path = args
        .catalog_path
        .clone()
        .map(Ok)
        .unwrap_or_else(|| repo_path("catalog/models.json"))?;
    let catalog_path = absolutize(catalog_path)?;
    let canaries_dir = args
        .canaries_dir
        .clone()
        .map(Ok)
        .unwrap_or_else(|| repo_path("catalog/canaries"))?;
    let canaries_dir = absolutize(canaries_dir)?;
    let artifact_path = absolutize(args.artifact_path.clone())?;
    let catalog_doc = catalog::load_document(&catalog_path)?;
    let model = catalog_doc
        .models
        .iter()
        .find(|model| model.model_id == args.model)
        .with_context(|| {
            format!(
                "model {} not found in {}",
                args.model,
                catalog_path.display()
            )
        })?;
    let artifact = model.artifacts.get(&args.artifact).with_context(|| {
        format!(
            "artifact {} not found for model {}; available: {}",
            args.artifact,
            model.model_id,
            model
                .artifacts
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let prompts = load_canary_prompts(
        Some(&canaries_dir),
        &model.canary.set_id,
        args.prompt_id.as_deref(),
    )?;
    let mut backend = catalog_calibration_backend(artifact, &artifact_path, &args)?;
    let mut reports = Vec::with_capacity(prompts.len());
    for prompt in &prompts {
        reports.push(calibrate_canary_prompt(
            backend.as_mut(),
            model,
            prompt,
            args.seed,
            args.include_output,
        )?);
    }
    let catalog_fingerprint = aggregate_canary_fingerprint(&reports);
    let existing = model.canary.fingerprints.get(&args.artifact).cloned();
    let matches_existing = existing
        .as_ref()
        .map(|existing| existing == &catalog_fingerprint);
    let report = CatalogCanaryCalibrationReport {
        model_id: model.model_id.clone(),
        artifact: args.artifact,
        engine: artifact.engine.clone(),
        artifact_path,
        canary_set: model.canary.set_id.clone(),
        prompt_count: reports.len(),
        catalog_fingerprint,
        existing_catalog_fingerprint: existing,
        matches_existing_catalog: matches_existing,
        prompts: reports,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Canary calibration complete.");
        println!("Model: {}", report.model_id);
        println!("Artifact: {} ({})", report.artifact, report.engine);
        println!("Canary set: {}", report.canary_set);
        println!("Prompts: {}", report.prompt_count);
        println!("Catalog fingerprint: {}", report.catalog_fingerprint);
        if let Some(existing) = &report.existing_catalog_fingerprint {
            println!("Existing catalog fingerprint: {existing}");
            println!(
                "Matches existing catalog: {}",
                report.matches_existing_catalog.unwrap_or(false)
            );
        }
        println!("Per-prompt fingerprints:");
        for prompt in &report.prompts {
            println!(
                "- {}: {} ({} tokens)",
                prompt.prompt_id, prompt.fingerprint, prompt.token_count
            );
        }
    }
    Ok(())
}

fn catalog_calibration_backend(
    artifact: &catalog::CatalogArtifact,
    artifact_path: &Path,
    args: &CatalogCalibrateCanaryArgs,
) -> Result<Box<dyn EngineBackend>> {
    let mut config = match artifact.engine.as_str() {
        "llama.cpp" => LoadConfig::gguf(artifact_path),
        "mlx" => LoadConfig::mlx_safetensors(artifact_path),
        "trt-llm" => bail!(
            "TensorRT-LLM canary calibration is gated on the P2b.2/P2b.3 backend and reference hardware"
        ),
        other => bail!("unsupported canary calibration engine {other}"),
    };
    config.ctx_size = args.ctx_size.max(1);
    config.threads = args.threads;
    config.gpu_layers = args.gpu_layers;
    if let Some(sha256) = &artifact.source_sha256 {
        config.artifact = config.artifact.with_sha256(sha256.clone());
    }

    match artifact.engine.as_str() {
        "llama.cpp" => {
            let mut backend =
                mayhem_engine::LlamaCppBackend::new().context("initializing llama.cpp backend")?;
            backend
                .load(config)
                .context("loading llama.cpp canary calibration artifact")?;
            Ok(Box::new(backend))
        }
        "mlx" => {
            let mut backend =
                mayhem_engine::MlxBackend::new().context("initializing MLX backend")?;
            backend
                .load(config)
                .context("loading MLX canary calibration artifact")?;
            Ok(Box::new(backend))
        }
        _ => unreachable!("unsupported engines returned above"),
    }
}

fn calibrate_canary_prompt(
    backend: &mut dyn EngineBackend,
    model: &catalog::CatalogModel,
    prompt: &CanaryPrompt,
    seed: u32,
    include_output: bool,
) -> Result<CanaryCalibrationPromptReport> {
    let body = canary_probe_request(&model.model_id, prompt);
    let mut request = provider_engine_request_from_body(&body)?;
    request.temperature = Some(prompt.temperature.unwrap_or(0.0) as f32);
    request.seed = Some(seed);
    let max_tokens = request.max_new_tokens;
    let mut token_ids = Vec::new();
    let output = backend
        .generate(request, &mut |chunk: mayhem_engine::TokenChunk| {
            token_ids.push(chunk.token_id);
            Ok(())
        })
        .with_context(|| format!("generating canary prompt {}", prompt.id))?;
    if token_ids.is_empty() {
        bail!("canary prompt {} produced no tokens", prompt.id);
    }
    let fingerprint = canary_token_fingerprint(token_ids.iter().copied());
    Ok(CanaryCalibrationPromptReport {
        prompt_id: prompt.id.clone(),
        max_tokens,
        prompt_tokens: output.usage.prompt_tokens,
        completion_tokens: output.usage.completion_tokens,
        token_count: token_ids.len(),
        fingerprint,
        output_text: include_output.then_some(output.text),
    })
}

fn canary_token_fingerprint(tokens: impl IntoIterator<Item = i32>) -> String {
    let mut hasher = blake3::Hasher::new();
    for token in tokens {
        hasher.update(&token.to_be_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn aggregate_canary_fingerprint(prompts: &[CanaryCalibrationPromptReport]) -> String {
    let mut hasher = blake3::Hasher::new();
    for prompt in prompts {
        let prompt_id = prompt.prompt_id.as_bytes();
        hasher.update(&(prompt_id.len() as u32).to_be_bytes());
        hasher.update(prompt_id);
        hasher.update(prompt.fingerprint.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

async fn pay(rail: PayRail, args: PayRailArgs) -> Result<()> {
    if args.poll_interval_ms == 0 {
        bail!("--poll-interval-ms must be positive");
    }
    let home = args.home.clone().map(Ok).unwrap_or_else(default_home)?;
    let home = absolutize(home)?;
    let config = read_mayhem_config(&home)?;
    let wallet = resolve_cli_wallet(
        &home,
        config.as_ref(),
        &args.peer_store_name,
        args.wallet_password.as_deref().unwrap_or(""),
    )
    .await?;
    let rpc_url = resolve_cli_rpc_url(Some(&home), args.rpc_url.as_deref())?;
    let rpc = PeerRpcClient::new(&rpc_url)?;
    let paygate_url = resolve_cli_paygate_url(config.as_ref(), args.paygate_url.as_deref());
    let amount_mu = parse_usd_amount_to_mu(&args.amount)?;
    let before_mu = read_user_balance_mu(&rpc, &wallet.public_key).await?;
    let checkout = create_pay_checkout(
        rail,
        &paygate_url,
        &wallet.public_key,
        amount_mu,
        args.idempotency_key.as_deref(),
        args.success_url.as_deref(),
        args.cancel_url.as_deref(),
    )
    .await?;
    emit_checkout_handoff(args.json, rail, amount_mu, &checkout.url)?;
    let opened = open_checkout_url(&checkout.url, args.no_open).await;
    let target_mu = before_mu
        .checked_add(amount_mu)
        .context("target balance overflowed")?;
    let status = if args.no_wait {
        PayCreditStatus {
            credited: false,
            before_mu,
            current_mu: before_mu,
            target_mu,
            waited_ms: 0,
        }
    } else {
        wait_for_credit(
            &rpc,
            &wallet.public_key,
            before_mu,
            target_mu,
            Duration::from_secs(args.timeout_seconds),
            Duration::from_millis(args.poll_interval_ms),
        )
        .await?
    };

    let report = json!({
        "ok": status.credited || args.no_wait,
        "rail": rail.as_str(),
        "denom": "mu_usd",
        "amount_mu": amount_mu,
        "amount_usd": mu_to_usd_amount(amount_mu),
        "who": wallet.public_key,
        "paygate_url": paygate_url,
        "rpc_url": rpc_url,
        "checkout": {
            "id": checkout.id,
            "url": checkout.url,
            "reference": checkout.reference,
        },
        "opened": opened,
        "credit": {
            "credited": status.credited,
            "before_mu": status.before_mu,
            "current_mu": status.current_mu,
            "target_mu": status.target_mu,
            "waited_ms": status.waited_ms,
        },
    });

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        if opened {
            println!("Opened checkout in your browser.");
        } else if !args.no_open {
            println!("Could not open a browser automatically; use the URL above.");
        }
        if args.no_wait {
            println!("Not waiting for ledger credit (--no-wait).");
        } else if status.credited {
            println!(
                "Credited: balance {} -> {} mu_usd.",
                status.before_mu, status.current_mu
            );
        }
    }

    if !args.no_wait && !status.credited {
        bail!(
            "timed out waiting for {} mu_usd credit; current balance {} mu_usd, target {} mu_usd",
            amount_mu,
            status.current_mu,
            status.target_mu
        );
    }

    Ok(())
}

async fn use_gateway(args: UseArgs) -> Result<()> {
    let home = args.home.clone().map(Ok).unwrap_or_else(default_home)?;
    let home = absolutize(home)?;
    let config = read_mayhem_config(&home)?;
    let bind = gateway_bind_addr(config.as_ref(), args.bind.as_deref(), args.port)?;
    let gateway_url = gateway_public_url(bind);
    let openai_base_url = gateway_v1_url(&gateway_url);
    let (state, source, model_count, backend) = if args.dev_embedded_catalog {
        let state = GatewayState::from_embedded_catalog();
        (
            state,
            "dev-embedded-catalog".to_owned(),
            None,
            "local-openai-shape".to_owned(),
        )
    } else {
        let rpc_url = resolve_cli_rpc_url(Some(&home), args.rpc_url.as_deref())?;
        let rpc = PeerRpcClient::new(&rpc_url)?;
        let contract = read_contract_catalog(&rpc).await?;
        let models = gateway_models_from_contract(&contract)?;
        let model_count = models.len();
        let (sc_bridge_url, sc_bridge_token) = resolve_cli_sc_bridge(
            Some(&home),
            args.sc_bridge_url.as_deref(),
            args.sc_bridge_token.as_deref(),
        )?;
        let backend = ScBridgeGatewaySessionBackend::new(ScBridgeGatewaySessionConfig::new(
            sc_bridge_url.clone(),
            sc_bridge_token,
        ));
        (
            GatewayState::from_models(models).with_session_backend(Arc::new(backend)),
            format!("contract:{rpc_url}"),
            Some(model_count),
            format!("sc-bridge-direct-session:{sc_bridge_url}"),
        )
    };
    let report = json!({
        "ok": true,
        "home": home,
        "bind": bind.to_string(),
        "gateway_url": gateway_url,
        "openai_base_url": openai_base_url,
        "source": source,
        "backend": backend,
        "models": model_count,
    });

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Mayhem gateway starting.");
        println!("Bind: {bind}");
        println!("Copy/paste OpenAI base URL: {openai_base_url}");
        if args.dev_embedded_catalog {
            println!("Model source: development embedded catalog (non-canonical).");
            println!("Backend: local OpenAI-shape smoke backend.");
        } else {
            println!(
                "Model source: canonical contract state ({} models).",
                model_count.unwrap_or(0)
            );
            println!("Backend: {backend}");
        }
        println!("Use Ctrl-C to stop.");
    }
    io::stdout().flush()?;

    serve_gateway(bind, state)
        .await
        .with_context(|| format!("serving Mayhem gateway on {bind}"))?;
    Ok(())
}

async fn models(args: ModelsArgs) -> Result<()> {
    if args.timeout_seconds == 0 {
        bail!("--timeout-seconds must be positive");
    }
    let home = args.home.clone().map(Ok).unwrap_or_else(default_home)?;
    let home = absolutize(home)?;
    let config = read_mayhem_config(&home)?;
    let gateway_root = resolve_cli_gateway_url(config.as_ref(), args.gateway_url.as_deref());
    let timeout = Duration::from_secs(args.timeout_seconds);
    let client = reqwest::Client::builder().timeout(timeout).build()?;
    let models = fetch_gateway_models(&client, &gateway_root).await?;
    let summaries = gateway_model_summaries(&models)?;
    let report = json!({
        "ok": true,
        "gateway_url": gateway_root,
        "models": summaries,
    });

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_models_report(&report)?;
    }
    Ok(())
}

async fn balance(args: BalanceArgs) -> Result<()> {
    let home = args.home.clone().map(Ok).unwrap_or_else(default_home)?;
    let home = absolutize(home)?;
    let config = read_mayhem_config(&home)?;
    let rpc_url = resolve_cli_rpc_url(Some(&home), args.rpc_url.as_deref())?;
    let who = if let Some(who) = args.who.clone() {
        who
    } else {
        resolve_cli_wallet(
            &home,
            config.as_ref(),
            &args.peer_store_name,
            args.wallet_password.as_deref().unwrap_or(""),
        )
        .await?
        .public_key
    };
    let rpc = PeerRpcClient::new(&rpc_url)?;
    let balance_record = read_balance_record(&rpc, &who).await?;
    let mu = balance_record
        .get("mu")
        .and_then(Value::as_u64)
        .context("normalized balance record missing mu")?;
    let frozen = read_state_value(&rpc, &format!("frozen/{who}")).await?;
    let report = json!({
        "ok": true,
        "rpc_url": rpc_url,
        "who": who,
        "balance": balance_record,
        "credit": {
            "denom": "mu_usd",
            "mu": mu,
            "usd": mu_to_usd_amount(mu),
        },
        "frozen": frozen,
    });

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_balance_report(&report);
    }
    Ok(())
}

async fn mayhem_test(args: TestArgs) -> Result<()> {
    if args.timeout_seconds == 0 {
        bail!("--timeout-seconds must be positive");
    }

    let home = args.home.clone().map(Ok).unwrap_or_else(default_home)?;
    let home = absolutize(home)?;
    let config = read_mayhem_config(&home)?;
    let gateway_url = resolve_cli_gateway_url(config.as_ref(), args.gateway_url.as_deref());
    let gateway_root = normalize_gateway_root(&gateway_url);
    let timeout = Duration::from_secs(args.timeout_seconds);
    let client = reqwest::Client::builder().timeout(timeout).build()?;

    let gateway_status =
        fetch_gateway_json(&client, &format!("{gateway_root}/mayhem/status")).await?;
    let models = fetch_gateway_models(&client, &gateway_root).await?;
    let selected_model = select_test_model(&models, args.model.as_deref())?;
    let direct_tool = run_gateway_tool_smoke(&client, &gateway_root, &selected_model.id).await?;

    let peer_health = if args.skip_peer_health {
        json!({ "skipped": true })
    } else {
        let rpc_url = resolve_cli_rpc_url(Some(&home), args.rpc_url.as_deref())?;
        let rpc = PeerRpcClient::new(&rpc_url)?;
        json!({
            "skipped": false,
            "rpc_url": rpc_url,
            "health": rpc.health().await.context("checking peer RPC health")?,
        })
    };

    let opencode_config_path = args
        .opencode_config
        .clone()
        .map(absolutize)
        .transpose()?
        .unwrap_or(default_opencode_config_path()?);
    let existing_opencode_models =
        read_existing_mayhem_opencode_model_count(&opencode_config_path).unwrap_or(0);
    let should_write_models = args.sync_models
        || existing_opencode_models == 0
        || !opencode_model_exists(&opencode_config_path, &selected_model.id).unwrap_or(false);

    let opencode_merge = if args.skip_opencode {
        json!({ "skipped": true })
    } else {
        serde_json::to_value(merge_mayhem_opencode_config(
            &opencode_config_path,
            &gateway_root,
            if should_write_models {
                Some(&models)
            } else {
                None
            },
            should_write_models,
        )?)?
    };

    let opencode_run = if args.skip_opencode || args.no_opencode_run {
        json!({ "skipped": true })
    } else {
        let opencode_bin = resolve_opencode_bin(&home, args.opencode_bin.as_deref());
        serde_json::to_value(
            run_opencode_smoke(
                &opencode_bin,
                &opencode_config_path,
                &selected_model.id,
                timeout,
            )
            .await?,
        )?
    };

    let receipts = fetch_gateway_json(&client, &format!("{gateway_root}/mayhem/receipts")).await?;
    let receipt = latest_gateway_receipt(&receipts);
    let expected_evidence_key = receipt
        .as_ref()
        .and_then(expected_usage_evidence_key)
        .unwrap_or_else(|| "ev/use/<epoch>".to_owned());

    let report = json!({
        "ok": true,
        "home": home,
        "role": config
            .as_ref()
            .and_then(|config| config.role.as_ref())
            .and_then(|role| role.mode.as_deref())
            .unwrap_or("unknown"),
        "gateway": {
            "url": gateway_root,
            "status": gateway_status,
            "models": models.len(),
            "selected_model": selected_model,
            "direct_tool_call": direct_tool,
        },
        "peer": peer_health,
        "opencode": {
            "config": opencode_merge,
            "run": opencode_run,
        },
        "receipt": receipt,
        "expected_epoch_evidence_key": expected_evidence_key,
    });

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_test_report(&report);
    }

    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct TestModel {
    id: String,
    tools: bool,
    json: bool,
    context: u64,
}

#[derive(Debug, Clone, Serialize)]
struct ModelSummary {
    id: String,
    providers_online: u64,
    rooms: u64,
    denom: String,
    in_per_1k_mu: u64,
    out_per_1k_mu: u64,
    tools: bool,
    json: bool,
    context: u64,
    attestation_tiers: BTreeMap<String, u64>,
}

#[derive(Debug, Serialize)]
struct OpencodeMergeReport {
    path: PathBuf,
    provider_id: String,
    base_url: String,
    models_written: usize,
    created: bool,
    enabled_provider_added: bool,
}

#[derive(Debug, Serialize)]
struct OpencodeRunReport {
    binary: String,
    model: String,
    session_id: Option<String>,
    tool_use_seen: bool,
    marker_seen: bool,
    work_dir: PathBuf,
    stdout_lines: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct CanarySetDocument {
    set_id: String,
    #[serde(default)]
    prompts: Vec<CanaryPrompt>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CanaryPrompt {
    id: String,
    #[serde(default)]
    messages: Vec<Value>,
    #[serde(default)]
    tools: Option<Vec<Value>>,
    #[serde(default)]
    temperature: Option<f64>,
    #[serde(default)]
    max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
struct TextMatchEvaluation {
    expected_text_hash: String,
    observed_text_hash: String,
    matched_positions: u32,
    total_positions: u32,
    match_bps: u32,
    pass: bool,
}

#[derive(Debug, Clone, Serialize)]
struct CanaryCalibrationPromptReport {
    prompt_id: String,
    max_tokens: u32,
    prompt_tokens: u32,
    completion_tokens: u32,
    token_count: usize,
    fingerprint: String,
    output_text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CatalogCanaryCalibrationReport {
    model_id: String,
    artifact: String,
    engine: String,
    artifact_path: PathBuf,
    canary_set: String,
    prompt_count: usize,
    catalog_fingerprint: String,
    existing_catalog_fingerprint: Option<String>,
    matches_existing_catalog: Option<bool>,
    prompts: Vec<CanaryCalibrationPromptReport>,
}

async fn auditor_canary(args: AuditorCanaryArgs) -> Result<()> {
    if args.timeout_seconds == 0 {
        bail!("--timeout-seconds must be positive");
    }
    if args.min_match_bps > 10_000 {
        bail!("--min-match-bps must be <= 10000");
    }
    let home = args.home.clone().map(Ok).unwrap_or_else(default_home)?;
    let home = absolutize(home)?;
    let config = read_mayhem_config(&home)?;
    let gateway_root = resolve_cli_gateway_url(config.as_ref(), args.gateway_url.as_deref());
    let timeout = Duration::from_secs(args.timeout_seconds);
    let client = reqwest::Client::builder().timeout(timeout).build()?;

    let models = fetch_gateway_models(&client, &gateway_root).await?;
    let model = select_test_model(&models, args.model.as_deref())?;
    let canary = load_canary_prompt(
        args.canaries_dir.as_deref(),
        &args.canary_set,
        args.prompt_id.as_deref(),
    )?;
    let expected_text = read_expected_canary_text(&args)?;
    let request = canary_probe_request(&model.id, &canary);
    let response = post_gateway_json(
        &client,
        &format!("{gateway_root}/v1/chat/completions"),
        &request,
    )
    .await?;
    let observed_text = gateway_chat_observed_text(&response)?;
    let evaluation = evaluate_text_match(&expected_text, &observed_text, args.min_match_bps);
    let receipts = fetch_gateway_json(&client, &format!("{gateway_root}/mayhem/receipts")).await?;
    let latest_receipt = latest_gateway_receipt(&receipts);
    let session_receipt_hash = latest_receipt.as_ref().map(stable_value_hash);
    let receipt_body = latest_receipt.as_ref().and_then(receipt_body);
    let provider = args
        .provider
        .clone()
        .or_else(|| {
            receipt_body
                .and_then(|body| body.get("provider"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .context("--provider is required when the gateway receipt does not expose provider")?;
    let enclave_id = args
        .enclave_id
        .clone()
        .or_else(|| {
            receipt_body
                .and_then(|body| body.get("enclave_id"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .context("--enclave-id is required when the gateway receipt does not expose enclave_id")?;
    let at = args.at.unwrap_or(unix_epoch_seconds()?);
    let evidence = json!({
        "schema_version": 1,
        "kind": "mayhem-canary-probe-evidence",
        "gateway_url": gateway_root,
        "model": model.id,
        "canary_set": args.canary_set,
        "prompt_id": canary.id,
        "request": request,
        "response": response,
        "observed_text": observed_text,
        "expected_text_hash": evaluation.expected_text_hash,
        "evaluation": evaluation,
        "latest_receipt": latest_receipt,
    });
    let evidence_hash = stable_value_hash(&evidence);
    let probe_id = args.probe_id.unwrap_or_else(|| {
        stable_value_hash(&json!({
            "provider": provider,
            "enclave_id": enclave_id,
            "canary_set": args.canary_set,
            "prompt_id": canary.id,
            "epoch": args.epoch,
            "evidence_hash": evidence_hash,
        }))
    });
    let probe_command = canary_probe_command(CanaryProbeCommandInput {
        probe_id: probe_id.clone(),
        provider: provider.clone(),
        enclave_id: enclave_id.clone(),
        epoch: args.epoch,
        at,
        canary_set: args.canary_set.clone(),
        match_bps: evaluation.match_bps,
        pass: evaluation.pass,
        session_receipt_hash,
        evidence_hash: evidence_hash.clone(),
    });

    let evidence_output = args
        .evidence_output
        .as_ref()
        .map(|path| absolutize(path.clone()))
        .transpose()?;
    if let Some(path) = &evidence_output {
        let evidence_file = json!({
            "evidence": evidence,
            "probe_command": probe_command,
        });
        write_json_file(path, &evidence_file)?;
    }

    let tx = if args.submit {
        let wallet = resolve_cli_wallet(
            &home,
            config.as_ref(),
            &args.peer_store_name,
            args.wallet_password.as_deref().unwrap_or(""),
        )
        .await?;
        let rpc_url = resolve_cli_rpc_url(Some(&home), args.rpc_url.as_deref())?;
        let rpc = PeerRpcClient::new(&rpc_url)?;
        let keypair_path = PathBuf::from(wallet.keypair_path.clone());
        Some(
            submit_contract_command(
                &rpc,
                &keypair_path,
                args.wallet_password.as_deref().unwrap_or(""),
                &wallet,
                "probeResult",
                probe_command.clone(),
                args.sim,
            )
            .await?,
        )
    } else {
        None
    };

    let report = json!({
        "ok": evaluation.pass,
        "gateway_url": gateway_root,
        "model": model,
        "canary": {
            "set_id": args.canary_set,
            "prompt_id": canary.id,
        },
        "evaluation": evaluation,
        "provider": probe_command["provider"],
        "enclave_id": probe_command["enclave_id"],
        "probe_command": probe_command,
        "evidence_hash": evidence_hash,
        "evidence_output": evidence_output,
        "submitted": tx,
    });

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Canary probe {}: match {} bps.",
            if report["ok"].as_bool().unwrap_or(false) {
                "passed"
            } else {
                "failed"
            },
            report["evaluation"]["match_bps"]
                .as_u64()
                .unwrap_or_default()
        );
        println!(
            "Probe id: {}",
            report["probe_command"]["probe_id"].as_str().unwrap_or("")
        );
        println!("Evidence hash: {evidence_hash}");
        if let Some(path) = evidence_output {
            println!("Copy/paste evidence path: {}", path.display());
        }
        if !args.submit {
            println!("Copy/paste probe_result command:");
            println!(
                "{}",
                serde_json::to_string_pretty(&report["probe_command"])?
            );
        }
    }
    Ok(())
}

fn load_canary_prompt(
    canaries_dir: Option<&Path>,
    set_id: &str,
    prompt_id: Option<&str>,
) -> Result<CanaryPrompt> {
    let prompts = load_canary_prompts(canaries_dir, set_id, prompt_id)?;
    prompts
        .into_iter()
        .next()
        .with_context(|| format!("canary set {set_id} has no prompts"))
}

fn load_canary_prompts(
    canaries_dir: Option<&Path>,
    set_id: &str,
    prompt_id: Option<&str>,
) -> Result<Vec<CanaryPrompt>> {
    let canaries_dir = canaries_dir
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(|| repo_path("catalog/canaries"))?;
    let path = absolutize(canaries_dir)?.join(format!("{set_id}.json"));
    let doc: CanarySetDocument = serde_json::from_value(read_json_file(&path)?)
        .with_context(|| format!("parsing canary set {}", path.display()))?;
    if doc.set_id != set_id {
        bail!("canary file {} declares set {}", path.display(), doc.set_id);
    }
    let prompts = if let Some(prompt_id) = prompt_id {
        vec![doc
            .prompts
            .into_iter()
            .find(|prompt| prompt.id == prompt_id)
            .with_context(|| format!("canary prompt {prompt_id} not found in {set_id}"))?]
    } else {
        doc.prompts
    };
    if prompts.is_empty() {
        bail!("canary set {set_id} has no prompts");
    }
    for prompt in &prompts {
        if prompt.messages.is_empty() {
            bail!("canary prompt {} has no messages", prompt.id);
        }
    }
    Ok(prompts)
}

fn read_expected_canary_text(args: &AuditorCanaryArgs) -> Result<String> {
    match (&args.expected_text, &args.expected_file) {
        (Some(text), None) if !text.is_empty() => Ok(text.clone()),
        (None, Some(path)) => fs::read_to_string(path)
            .with_context(|| format!("reading expected canary text {}", path.display())),
        (Some(_), Some(_)) => bail!("use either --expected-text or --expected-file, not both"),
        _ => bail!("--expected-text or --expected-file is required for canary scoring"),
    }
}

fn canary_probe_request(model_id: &str, prompt: &CanaryPrompt) -> Value {
    let mut request = json!({
        "model": model_id,
        "messages": &prompt.messages,
        "temperature": prompt.temperature.unwrap_or(0.0),
        "max_tokens": prompt.max_tokens.unwrap_or(128),
        "stream": false,
    });
    if let Some(tools) = &prompt.tools {
        request["tools"] = json!(tools);
    }
    request
}

struct CanaryProbeCommandInput {
    probe_id: String,
    provider: String,
    enclave_id: String,
    epoch: u64,
    at: u64,
    canary_set: String,
    match_bps: u32,
    pass: bool,
    session_receipt_hash: Option<String>,
    evidence_hash: String,
}

fn canary_probe_command(input: CanaryProbeCommandInput) -> Value {
    let mut command = json!({
        "op": "probe_result",
        "probe_id": input.probe_id,
        "probe_kind": "canary",
        "provider": input.provider,
        "enclave_id": input.enclave_id,
        "epoch": input.epoch,
        "at": input.at,
        "canary_set": input.canary_set,
        "match_bps": input.match_bps,
        "pass": input.pass,
        "evidence_hash": input.evidence_hash,
    });
    if let Some(session_receipt_hash) = input.session_receipt_hash {
        command["session_receipt_hash"] = json!(session_receipt_hash);
    }
    command
}

fn gateway_chat_observed_text(response: &Value) -> Result<String> {
    let message = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .context("gateway canary response did not include choices[0].message")?;
    if let Some(content) = message.get("content").and_then(Value::as_str) {
        return Ok(content.to_owned());
    }
    if let Some(tool_calls) = message.get("tool_calls") {
        return Ok(tool_calls.to_string());
    }
    bail!("gateway canary response message had neither content nor tool_calls")
}

fn normalize_canary_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn evaluate_text_match(expected: &str, observed: &str, min_match_bps: u32) -> TextMatchEvaluation {
    let expected = normalize_canary_text(expected);
    let observed = normalize_canary_text(observed);
    let expected_chars = expected.chars().collect::<Vec<_>>();
    let observed_chars = observed.chars().collect::<Vec<_>>();
    let total_positions = expected_chars.len() as u32;
    let matched_positions = expected_chars
        .iter()
        .zip(observed_chars.iter())
        .filter(|(expected, observed)| expected == observed)
        .count() as u32;
    let match_bps = if total_positions == 0 {
        0
    } else {
        ((u64::from(matched_positions) * 10_000) / u64::from(total_positions)) as u32
    };
    TextMatchEvaluation {
        expected_text_hash: blake3::hash(expected.as_bytes()).to_hex().to_string(),
        observed_text_hash: blake3::hash(observed.as_bytes()).to_hex().to_string(),
        matched_positions,
        total_positions,
        match_bps,
        pass: match_bps >= min_match_bps,
    }
}

fn stable_value_hash(value: &Value) -> String {
    let stable = stable_json_value(value);
    blake3::hash(stable.to_string().as_bytes())
        .to_hex()
        .to_string()
}

fn stable_json_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(stable_json_value).collect()),
        Value::Object(map) => {
            let mut stable = serde_json::Map::new();
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (key, value) in entries {
                stable.insert(key.clone(), stable_json_value(value));
            }
            Value::Object(stable)
        }
        value => value.clone(),
    }
}

async fn fetch_gateway_json(client: &reqwest::Client, url: &str) -> Result<Value> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("requesting {url}"))?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        bail!("gateway returned {status} for {url}: {body}");
    }
    serde_json::from_str(&body).with_context(|| format!("parsing JSON from {url}"))
}

async fn fetch_gateway_models(client: &reqwest::Client, gateway_root: &str) -> Result<Vec<Value>> {
    let value = fetch_gateway_json(client, &format!("{gateway_root}/v1/models")).await?;
    let models = value
        .get("data")
        .and_then(Value::as_array)
        .filter(|models| !models.is_empty())
        .context("gateway /v1/models returned no models")?
        .clone();
    Ok(models)
}

fn select_test_model(models: &[Value], requested: Option<&str>) -> Result<TestModel> {
    let selected = if let Some(requested) = requested {
        models
            .iter()
            .find(|model| model.get("id").and_then(Value::as_str) == Some(requested))
            .with_context(|| format!("requested model {requested} was not in /v1/models"))?
    } else {
        models
            .iter()
            .find(|model| model_tool_capable(model))
            .unwrap_or(&models[0])
    };
    gateway_model_view(selected)
}

fn gateway_model_view(model: &Value) -> Result<TestModel> {
    let id = model
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .context("gateway model missing id")?;
    Ok(TestModel {
        id: id.to_owned(),
        tools: model_tool_capable(model),
        json: model
            .get("mayhem")
            .and_then(|mayhem| mayhem.get("caps"))
            .and_then(|caps| caps.get("json"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        context: model
            .get("mayhem")
            .and_then(|mayhem| mayhem.get("caps"))
            .and_then(|caps| caps.get("ctx"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
    })
}

fn gateway_model_summaries(models: &[Value]) -> Result<Vec<ModelSummary>> {
    models.iter().map(gateway_model_summary).collect()
}

fn gateway_model_summary(model: &Value) -> Result<ModelSummary> {
    let id = model
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .context("gateway model missing id")?;
    let mayhem = model.get("mayhem").unwrap_or(&Value::Null);
    let price = mayhem.get("price_ref_mu").unwrap_or(&Value::Null);
    let caps = mayhem.get("caps").unwrap_or(&Value::Null);
    let attestation_tiers = mayhem
        .get("attestation_tiers")
        .and_then(Value::as_object)
        .map(|tiers| {
            tiers
                .iter()
                .filter_map(|(tier, value)| value.as_u64().map(|count| (tier.clone(), count)))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    Ok(ModelSummary {
        id: id.to_owned(),
        providers_online: mayhem
            .get("providers_online")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        rooms: mayhem.get("rooms").and_then(Value::as_u64).unwrap_or(0),
        denom: price
            .get("denom")
            .and_then(Value::as_str)
            .unwrap_or("mu_usd")
            .to_owned(),
        in_per_1k_mu: price.get("in_per_1k").and_then(Value::as_u64).unwrap_or(0),
        out_per_1k_mu: price.get("out_per_1k").and_then(Value::as_u64).unwrap_or(0),
        tools: caps.get("tools").and_then(Value::as_bool).unwrap_or(false),
        json: caps.get("json").and_then(Value::as_bool).unwrap_or(false),
        context: caps.get("ctx").and_then(Value::as_u64).unwrap_or(0),
        attestation_tiers,
    })
}

fn model_tool_capable(model: &Value) -> bool {
    model
        .get("mayhem")
        .and_then(|mayhem| mayhem.get("caps"))
        .and_then(|caps| caps.get("tools"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

async fn run_gateway_tool_smoke(
    client: &reqwest::Client,
    gateway_root: &str,
    model_id: &str,
) -> Result<Value> {
    let request = json!({
        "model": model_id,
        "messages": [{ "role": "user", "content": "Call the mayhem_ping tool." }],
        "tools": [{
            "type": "function",
            "function": {
                "name": "mayhem_ping",
                "description": "Return a small Mayhem smoke-test marker.",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }
            }
        }],
        "tool_choice": { "type": "function", "function": { "name": "mayhem_ping" } }
    });
    let response = post_gateway_json(
        client,
        &format!("{gateway_root}/v1/chat/completions"),
        &request,
    )
    .await?;
    let tool_call = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("tool_calls"))
        .and_then(Value::as_array)
        .and_then(|calls| calls.first())
        .cloned()
        .context("gateway chat completion did not return a tool call")?;
    let tool_name = tool_call
        .get("function")
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .context("gateway tool call missing function name")?;
    if tool_name != "mayhem_ping" {
        bail!("gateway returned unexpected tool call {tool_name}");
    }
    let tool_call_id = tool_call
        .get("id")
        .and_then(Value::as_str)
        .context("gateway tool call missing id")?;
    let followup = json!({
        "model": model_id,
        "messages": [
            { "role": "user", "content": "Call the mayhem_ping tool." },
            { "role": "assistant", "content": null, "tool_calls": [tool_call] },
            { "role": "tool", "tool_call_id": tool_call_id, "content": "{\"ok\":true}" }
        ]
    });
    let final_response = post_gateway_json(
        client,
        &format!("{gateway_root}/v1/chat/completions"),
        &followup,
    )
    .await?;
    Ok(json!({
        "tool_call": response,
        "followup": final_response,
    }))
}

async fn post_gateway_json(client: &reqwest::Client, url: &str, body: &Value) -> Result<Value> {
    let response = client
        .post(url)
        .json(body)
        .send()
        .await
        .with_context(|| format!("posting {url}"))?;
    let status = response.status();
    let response_body = response.text().await?;
    if !status.is_success() {
        bail!("gateway returned {status} for {url}: {response_body}");
    }
    serde_json::from_str(&response_body).with_context(|| format!("parsing JSON from {url}"))
}

fn default_opencode_config_path() -> Result<PathBuf> {
    if let Ok(path) = env::var("MAYHEM_OPENCODE_CONFIG") {
        if !path.trim().is_empty() {
            return absolutize(PathBuf::from(path));
        }
    }
    if cfg!(target_os = "windows") {
        if let Ok(appdata) = env::var("APPDATA") {
            if !appdata.trim().is_empty() {
                return Ok(PathBuf::from(appdata)
                    .join("opencode")
                    .join("opencode.json"));
            }
        }
    }
    if let Ok(config_home) = env::var("XDG_CONFIG_HOME") {
        if !config_home.trim().is_empty() {
            return Ok(PathBuf::from(config_home)
                .join("opencode")
                .join("opencode.json"));
        }
    }
    Ok(user_home_dir()?
        .join(".config")
        .join("opencode")
        .join("opencode.json"))
}

fn merge_mayhem_opencode_config(
    path: &Path,
    gateway_root: &str,
    models: Option<&[Value]>,
    write_models: bool,
) -> Result<OpencodeMergeReport> {
    let created = !path.exists();
    let mut root = if created {
        json!({ "$schema": OPENCODE_SCHEMA_URL })
    } else {
        let text =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?
    };
    if !root.is_object() {
        bail!("{} must contain a JSON object", path.display());
    }
    let object = root.as_object_mut().expect("checked object");
    object
        .entry("$schema")
        .or_insert_with(|| Value::String(OPENCODE_SCHEMA_URL.to_owned()));
    if !object
        .get("provider")
        .is_some_and(|provider| provider.is_object())
    {
        object.insert("provider".to_owned(), Value::Object(Map::new()));
    }
    let provider = object
        .get_mut("provider")
        .and_then(Value::as_object_mut)
        .expect("provider object");
    let existing_models = provider
        .get(OPENCODE_PROVIDER_ID)
        .and_then(|entry| entry.get("models"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let model_map = if write_models {
        opencode_models_from_gateway(models.unwrap_or(&[]))?
    } else {
        existing_models
    };
    let models_written = model_map.len();
    provider.insert(
        OPENCODE_PROVIDER_ID.to_owned(),
        json!({
            "npm": OPENCODE_PROVIDER_NPM,
            "name": OPENCODE_PROVIDER_NAME,
            "options": {
                "baseURL": gateway_v1_url(gateway_root),
                "apiKey": "mayhem-local",
                "timeout": false,
                "headerTimeout": false,
                "chunkTimeout": 300000
            },
            "models": Value::Object(model_map),
        }),
    );

    let enabled_provider_added = ensure_enabled_provider(object, OPENCODE_PROVIDER_ID);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_vec_pretty(&root)?)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(OpencodeMergeReport {
        path: path.to_path_buf(),
        provider_id: OPENCODE_PROVIDER_ID.to_owned(),
        base_url: gateway_v1_url(gateway_root),
        models_written,
        created,
        enabled_provider_added,
    })
}

fn read_existing_mayhem_opencode_model_count(path: &Path) -> Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let root: Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(root
        .get("provider")
        .and_then(|provider| provider.get(OPENCODE_PROVIDER_ID))
        .and_then(|provider| provider.get("models"))
        .and_then(Value::as_object)
        .map(Map::len)
        .unwrap_or(0))
}

fn opencode_model_exists(path: &Path, model_id: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let root: Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(root
        .get("provider")
        .and_then(|provider| provider.get(OPENCODE_PROVIDER_ID))
        .and_then(|provider| provider.get("models"))
        .and_then(Value::as_object)
        .is_some_and(|models| models.contains_key(model_id)))
}

fn opencode_models_from_gateway(models: &[Value]) -> Result<Map<String, Value>> {
    let mut out = Map::new();
    for model in models {
        let view = gateway_model_view(model)?;
        out.insert(
            view.id.clone(),
            json!({
                "name": view.id,
                "limit": {
                    "context": view.context.max(1),
                    "output": 4096
                },
                "tool_call": view.tools,
                "reasoning": false,
                "options": {
                    "temperature": 0,
                    "top_p": 1
                }
            }),
        );
    }
    Ok(out)
}

fn ensure_enabled_provider(root: &mut Map<String, Value>, provider_id: &str) -> bool {
    match root.get_mut("enabled_providers") {
        Some(Value::Array(providers)) => {
            if providers
                .iter()
                .any(|value| value.as_str() == Some(provider_id))
            {
                false
            } else {
                providers.push(Value::String(provider_id.to_owned()));
                true
            }
        }
        Some(_) => false,
        None => false,
    }
}

fn resolve_opencode_bin(home: &Path, requested: Option<&Path>) -> String {
    if let Some(requested) = requested {
        return requested.display().to_string();
    }
    let home_bin = if cfg!(target_os = "windows") {
        home.join("bin").join("opencode.exe")
    } else {
        home.join("bin").join("opencode")
    };
    if home_bin.exists() {
        home_bin.display().to_string()
    } else {
        "opencode".to_owned()
    }
}

async fn run_opencode_smoke(
    opencode_bin: &str,
    opencode_config_path: &Path,
    model_id: &str,
    timeout: Duration,
) -> Result<OpencodeRunReport> {
    let work_dir = temp_work_dir("mayhem-opencode-test")?;
    let model = format!("{OPENCODE_PROVIDER_ID}/{model_id}");
    let mut command = Command::new(opencode_bin);
    command
        .arg("run")
        .arg("--pure")
        .arg("--model")
        .arg(&model)
        .arg("--format")
        .arg("json")
        .arg("--dir")
        .arg(&work_dir)
        .arg("Use the bash tool once to print mayhem-opencode-tool-ok, then answer with mayhem-opencode-tool-ok.");
    command.env("OPENCODE_DISABLE_AUTOUPDATE", "1");
    let config_home = xdg_config_home_for_opencode_config(opencode_config_path).with_context(
        || {
            format!(
                "opencode only reads config paths shaped like <config-home>/opencode/opencode.json; got {}",
                opencode_config_path.display()
            )
        },
    )?;
    command.env("XDG_CONFIG_HOME", config_home);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| anyhow::anyhow!("opencode run timed out after {}s", timeout.as_secs()))?
        .with_context(|| format!("running {opencode_bin}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        bail!("opencode run failed: {}", stderr.trim());
    }
    let parsed = parse_opencode_run_output(&stdout)?;
    if !parsed.tool_use_seen {
        bail!("opencode run did not exercise a tool call");
    }
    if !parsed.marker_seen {
        bail!("opencode run did not echo {OPENCODE_TEST_MARKER}");
    }
    Ok(OpencodeRunReport {
        binary: opencode_bin.to_owned(),
        model,
        session_id: parsed.session_id,
        tool_use_seen: parsed.tool_use_seen,
        marker_seen: parsed.marker_seen,
        work_dir,
        stdout_lines: parsed.stdout_lines,
    })
}

#[derive(Debug)]
struct ParsedOpencodeOutput {
    session_id: Option<String>,
    tool_use_seen: bool,
    marker_seen: bool,
    stdout_lines: usize,
}

fn parse_opencode_run_output(stdout: &str) -> Result<ParsedOpencodeOutput> {
    let mut parsed = ParsedOpencodeOutput {
        session_id: None,
        tool_use_seen: false,
        marker_seen: stdout.contains(OPENCODE_TEST_MARKER),
        stdout_lines: 0,
    };
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        parsed.stdout_lines += 1;
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if parsed.session_id.is_none() {
            parsed.session_id = value
                .get("sessionID")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        if value.get("type").and_then(Value::as_str) == Some("tool_use") {
            parsed.tool_use_seen = true;
        }
        if value.to_string().contains(OPENCODE_TEST_MARKER) {
            parsed.marker_seen = true;
        }
    }
    Ok(parsed)
}

fn xdg_config_home_for_opencode_config(path: &Path) -> Option<PathBuf> {
    if path.file_name().and_then(|name| name.to_str()) != Some("opencode.json") {
        return None;
    }
    let parent = path.parent()?;
    if parent.file_name().and_then(|name| name.to_str()) != Some("opencode") {
        return None;
    }
    parent.parent().map(Path::to_path_buf)
}

fn latest_gateway_receipt(receipts: &Value) -> Option<Value> {
    receipts
        .get("data")
        .and_then(Value::as_array)
        .and_then(|data| data.last())
        .cloned()
}

fn expected_usage_evidence_key(receipt: &Value) -> Option<String> {
    let body = receipt_body(receipt)?;
    let ts = body.get("ts")?.as_u64()?;
    Some(format!("ev/use/{}", ts / DEFAULT_EPOCH_LENGTH_MILLIS))
}

fn receipt_body(receipt: &Value) -> Option<&Value> {
    let receipt = receipt.get("receipt").unwrap_or(receipt);
    Some(receipt.get("body").unwrap_or(receipt))
}

fn normalize_gateway_root(gateway_url: &str) -> String {
    let trimmed = gateway_url.trim().trim_end_matches('/');
    trimmed
        .strip_suffix("/v1")
        .unwrap_or(trimmed)
        .trim_end_matches('/')
        .to_owned()
}

fn gateway_v1_url(gateway_root: &str) -> String {
    let root = normalize_gateway_root(gateway_root);
    format!("{root}/v1")
}

fn gateway_bind_addr(
    config: Option<&MayhemConfig>,
    bind: Option<&str>,
    port: u16,
) -> Result<SocketAddr> {
    if let Some(bind) = bind {
        let bind = bind.trim();
        if !bind.is_empty() {
            return bind
                .parse()
                .with_context(|| format!("parsing gateway bind address {bind}"));
        }
    }
    let port = config
        .and_then(|config| config.network.as_ref())
        .and_then(|network| network.gateway_url.as_deref())
        .and_then(gateway_port_from_url)
        .unwrap_or(port);
    Ok(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
}

fn gateway_port_from_url(value: &str) -> Option<u16> {
    let root = normalize_gateway_root(value);
    reqwest::Url::parse(&root).ok()?.port()
}

fn gateway_public_url(bind: SocketAddr) -> String {
    match bind {
        SocketAddr::V4(addr) if addr.ip().is_unspecified() => {
            format!("http://127.0.0.1:{}", addr.port())
        }
        SocketAddr::V6(addr) if addr.ip().is_unspecified() => {
            format!("http://[::1]:{}", addr.port())
        }
        bind => format!("http://{bind}"),
    }
}

fn resolve_cli_gateway_url(config: Option<&MayhemConfig>, gateway_url: Option<&str>) -> String {
    if let Some(gateway_url) = gateway_url {
        let gateway_url = gateway_url.trim();
        if !gateway_url.is_empty() {
            return normalize_gateway_root(gateway_url);
        }
    }
    if let Ok(gateway_url) = env::var("MAYHEM_GATEWAY_URL") {
        let gateway_url = gateway_url.trim();
        if !gateway_url.is_empty() {
            return normalize_gateway_root(gateway_url);
        }
    }
    config
        .and_then(|config| config.network.as_ref())
        .and_then(|network| network.gateway_url.as_deref())
        .filter(|value| !value.trim().is_empty())
        .map(normalize_gateway_root)
        .unwrap_or_else(|| DEFAULT_GATEWAY_URL.to_owned())
}

fn temp_work_dir(prefix: &str) -> Result<PathBuf> {
    let path = env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        now_millis_for_path()
    ));
    fs::create_dir_all(&path).with_context(|| format!("creating {}", path.display()))?;
    Ok(path)
}

fn now_millis_for_path() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn print_test_report(report: &Value) {
    let gateway = &report["gateway"];
    println!("Mayhem test OK.");
    println!("Gateway: {}", gateway["url"].as_str().unwrap_or(""));
    println!(
        "Models: {} (selected {})",
        gateway["models"].as_u64().unwrap_or(0),
        gateway["selected_model"]["id"].as_str().unwrap_or("")
    );
    if report["peer"]["skipped"].as_bool().unwrap_or(false) {
        println!("Peer RPC: skipped");
    } else {
        println!("Peer RPC: ok");
    }
    if report["opencode"]["run"]["skipped"]
        .as_bool()
        .unwrap_or(false)
    {
        println!("opencode: skipped");
    } else {
        println!(
            "opencode: ok ({})",
            report["opencode"]["run"]["model"].as_str().unwrap_or("")
        );
    }
    if let Some(session_id) = receipt_body(&report["receipt"])
        .and_then(|body| body.get("session_id"))
        .and_then(Value::as_str)
    {
        println!("Receipt session: {session_id}");
    }
    println!(
        "Expected epoch evidence key: {}",
        report["expected_epoch_evidence_key"].as_str().unwrap_or("")
    );
}

fn print_models_report(report: &Value) -> Result<()> {
    println!("Gateway: {}", report["gateway_url"].as_str().unwrap_or(""));
    println!(
        "{:<52} {:>9} {:>5} {:>17} {:>7} {:>5} {:>5}",
        "MODEL", "PROVIDERS", "ROOMS", "MU/1K IN/OUT", "CTX", "TOOLS", "JSON"
    );
    for model in report["models"]
        .as_array()
        .context("models report missing models[]")?
    {
        let id = model["id"].as_str().unwrap_or("");
        let providers = model["providers_online"].as_u64().unwrap_or(0);
        let rooms = model["rooms"].as_u64().unwrap_or(0);
        let price = format!(
            "{}/{}",
            model["in_per_1k_mu"].as_u64().unwrap_or(0),
            model["out_per_1k_mu"].as_u64().unwrap_or(0)
        );
        let context = model["context"].as_u64().unwrap_or(0);
        let tools = bool_mark(model["tools"].as_bool().unwrap_or(false));
        let json = bool_mark(model["json"].as_bool().unwrap_or(false));
        println!(
            "{:<52} {:>9} {:>5} {:>17} {:>7} {:>5} {:>5}",
            truncate_for_table(id, 52),
            providers,
            rooms,
            price,
            context,
            tools,
            json
        );
    }
    Ok(())
}

fn print_balance_report(report: &Value) {
    let balance = &report["balance"];
    let mu = report["credit"]["mu"].as_u64().unwrap_or(0);
    println!("Mayhem balance");
    println!("Public key: {}", report["who"].as_str().unwrap_or(""));
    println!(
        "Credit: {} USD ({} mu_usd)",
        report["credit"]["usd"].as_str().unwrap_or("0.00"),
        mu
    );
    println!(
        "Updated epoch: {}",
        balance["updated_epoch"].as_u64().unwrap_or(0)
    );
    if let Some(updated_at) = balance.get("updated_at").and_then(Value::as_u64) {
        println!("Updated at: {updated_at}");
    }
    if let Some(status) = report["frozen"].get("status").and_then(Value::as_str) {
        println!("Frozen: {status}");
    } else {
        println!("Frozen: no");
    }
}

fn bool_mark(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn truncate_for_table(value: &str, width: usize) -> String {
    let count = value.chars().count();
    if count <= width {
        return value.to_owned();
    }
    if width <= 1 {
        return ".".repeat(width);
    }
    let mut out = value.chars().take(width - 1).collect::<String>();
    out.push('.');
    out
}

async fn payouts(args: PayoutsArgs) -> Result<()> {
    let rpc_url = resolve_cli_rpc_url(args.home.as_ref(), args.rpc_url.as_deref())?;
    let rpc = PeerRpcClient::new(&rpc_url)?;
    let report = if let Some(epoch) = args.epoch {
        json!({
            "rpc_url": rpc_url,
            "epoch": epoch,
            "pay": read_state_value(&rpc, &format!("ev/pay/{epoch}")).await?,
            "fee": read_state_value(&rpc, &format!("ev/fee/{epoch}")).await?,
        })
    } else {
        let mut pay = read_prefix_entries(&rpc, "ev/pay/").await?;
        pay.sort_by(|a, b| a.key.cmp(&b.key));
        let mut fee_sweeps = read_prefix_entries(&rpc, "ev/fee/")
            .await?
            .into_iter()
            .filter(|entry| {
                entry
                    .value
                    .get("sweep_msb_tx_hash")
                    .and_then(Value::as_str)
                    .is_some()
            })
            .collect::<Vec<_>>();
        fee_sweeps.sort_by(|a, b| a.key.cmp(&b.key));
        json!({
            "rpc_url": rpc_url,
            "pay": pay,
            "fee_sweeps": fee_sweeps,
        })
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_payouts_report(&report);
    }
    Ok(())
}

async fn earnings(args: EarningsArgs) -> Result<()> {
    let rpc_url = resolve_cli_rpc_url(args.home.as_ref(), args.rpc_url.as_deref())?;
    let rpc = PeerRpcClient::new(&rpc_url)?;
    let records = if let Some(provider) = &args.provider {
        read_state_value(&rpc, &format!("earn/{provider}"))
            .await?
            .into_iter()
            .map(|value| serde_json::from_value(value).context("parsing earning record"))
            .collect::<Result<Vec<LedgerEarningRecord>>>()?
    } else {
        read_prefix_values(&rpc, "earn/").await?
    };
    let mut views = records
        .into_iter()
        .map(earning_view)
        .collect::<Result<Vec<_>>>()?;
    views.sort_by(|a, b| a.provider.cmp(&b.provider));
    let report = json!({
        "rpc_url": rpc_url,
        "earnings": views,
    });

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_earnings_report(&report);
    }
    Ok(())
}

async fn receipts_export(args: ReceiptsExportArgs) -> Result<()> {
    let deposits = read_optional_json_array(args.deposits_file.as_deref(), "deposits")?;
    let receipts = read_receipts_for_export(&args).await?;
    let payouts = read_optional_json_array(args.payouts_file.as_deref(), "payouts")?;
    let prior_earnings = read_prior_earnings(args.prior_earnings_file.as_deref())?;
    let bundle = EpochAuditBundle {
        epoch: args.epoch,
        fee_bps: args.fee_bps,
        deposits,
        receipts,
        payouts,
        prior_earnings,
        prior_fee_cum_mu: args.prior_fee_cum_mu,
    };

    let output_path = args
        .output
        .as_ref()
        .map(|path| absolutize(path.clone()))
        .transpose()?;
    if let Some(path) = &output_path {
        write_json_file(path, &bundle)?;
    }

    let mut cleanup_path = None;
    let verifier_input_path = if let Some(path) = &output_path {
        path.clone()
    } else if args.no_verify {
        PathBuf::new()
    } else {
        let path = temp_audit_bundle_path(args.epoch)?;
        write_json_file(&path, &bundle)?;
        cleanup_path = Some(path.clone());
        path
    };

    let mut report = ReceiptsExportReport {
        bundle,
        bundle_path: output_path.clone(),
        recomputed: None,
        evidence: None,
        checks: Vec::new(),
        verified: false,
    };

    if !args.no_verify {
        let verifier_script = args
            .verifier_script
            .clone()
            .map(Ok)
            .unwrap_or_else(|| repo_path("intercom/scripts/recompute-epoch-roots.mjs"))?;
        let verifier_script = absolutize(verifier_script)?;
        let recomputed = run_epoch_recompute_script(&verifier_script, &verifier_input_path).await?;
        let evidence = read_evidence_for_export(&args).await?;
        let checks = verify_epoch_evidence(args.epoch, &recomputed, &evidence);
        let verified = checks.iter().all(|check| check.ok);
        report.recomputed = Some(recomputed);
        report.evidence = Some(evidence);
        report.checks = checks;
        report.verified = verified;
        if !verified {
            bail!(
                "exported receipt bundle did not verify against ev/* records; rerun with --json for details"
            );
        }
    }

    if let Some(path) = cleanup_path {
        let _ = fs::remove_file(path);
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if output_path.is_none() {
        println!("{}", serde_json::to_string_pretty(&report.bundle)?);
    } else if report.verified {
        println!(
            "Receipt audit bundle verified for epoch {}: {} checks matched.",
            args.epoch,
            report.checks.len()
        );
    } else {
        println!("Receipt audit bundle written for epoch {}.", args.epoch);
    }
    Ok(())
}

async fn receipts_publish(args: ReceiptsPublishArgs) -> Result<()> {
    if args.poll_interval_ms == 0 {
        bail!("--poll-interval-ms must be positive");
    }
    let channel = epoch_sidechannel(args.epoch, args.channel.as_deref());
    let gateway_url = args
        .gateway_url
        .as_deref()
        .unwrap_or(DEFAULT_GATEWAY_URL)
        .trim_end_matches('/')
        .to_owned();
    let (sc_bridge_url, sc_bridge_token) = resolve_cli_sc_bridge(
        args.home.as_ref(),
        args.sc_bridge_url.as_deref(),
        args.sc_bridge_token.as_deref(),
    )?;
    let mut bridge = ScBridgeClient::connect(ScBridgeConfig::new(&sc_bridge_url, sc_bridge_token)?)
        .await
        .context("connecting to SC-Bridge for epoch receipt publish")?;
    bridge
        .join(&channel)
        .await
        .with_context(|| format!("joining sidechannel {channel}"))?;

    let deadline = (args.watch && args.timeout_seconds > 0)
        .then(|| Instant::now() + Duration::from_secs(args.timeout_seconds));
    let mut seen = BTreeSet::new();
    let mut published = 0usize;

    loop {
        let receipts = fetch_gateway_receipts(&gateway_url).await?;
        for receipt in receipts {
            let receipt_id = receipt_id(&receipt);
            if !seen.insert(receipt_id.clone()) {
                continue;
            }
            let message = epoch_receipt_message(args.epoch, receipt_id, receipt)?;
            bridge
                .send(&channel, &message)
                .await
                .with_context(|| format!("publishing receipt to {channel}"))?;
            published += 1;
            if args.max_receipts.is_some_and(|max| published >= max) {
                break;
            }
        }

        if !args.watch || args.max_receipts.is_some_and(|max| published >= max) {
            break;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break;
        }
        sleep(Duration::from_millis(args.poll_interval_ms)).await;
    }

    let report = json!({
        "epoch": args.epoch,
        "channel": channel,
        "gateway_url": gateway_url,
        "published": published,
        "seen": seen.len(),
        "watch": args.watch,
    });

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Published {} unique receipt(s) to {}.",
            report["published"].as_u64().unwrap_or_default(),
            report["channel"].as_str().unwrap_or("")
        );
    }
    Ok(())
}

async fn receipts_collect(args: ReceiptsCollectArgs) -> Result<()> {
    let channel = epoch_sidechannel(args.epoch, args.channel.as_deref());
    let (sc_bridge_url, sc_bridge_token) = resolve_cli_sc_bridge(
        args.home.as_ref(),
        args.sc_bridge_url.as_deref(),
        args.sc_bridge_token.as_deref(),
    )?;
    let mut bridge = ScBridgeClient::connect(ScBridgeConfig::new(&sc_bridge_url, sc_bridge_token)?)
        .await
        .context("connecting to SC-Bridge for epoch receipt collection")?;
    bridge
        .subscribe([channel.as_str()])
        .await
        .with_context(|| format!("subscribing to sidechannel {channel}"))?;
    bridge
        .join(&channel)
        .await
        .with_context(|| format!("joining sidechannel {channel}"))?;

    let deadline = Instant::now() + Duration::from_secs(args.timeout_seconds);
    let mut seen = BTreeSet::new();
    let mut receipts = Vec::new();

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match bridge.next_sidechannel_message(remaining).await {
            Ok(event) => {
                let Some((receipt_id, receipt)) =
                    epoch_receipt_from_sidechannel_event(&event, args.epoch, &channel)
                else {
                    continue;
                };
                if seen.insert(receipt_id) {
                    receipts.push(receipt);
                }
                if args.max_receipts.is_some_and(|max| receipts.len() >= max) {
                    break;
                }
            }
            Err(mayhem_bridge::BridgeError::Timeout) => break,
            Err(err) => return Err(err).context("collecting epoch receipt sidechannel message"),
        }
    }

    let output = json!({
        "schema_version": 1,
        "epoch": args.epoch,
        "channel": channel,
        "collected_at_ms": unix_epoch_millis()?,
        "data": receipts,
    });

    let output_path = args
        .output
        .as_ref()
        .map(|path| absolutize(path.clone()))
        .transpose()?;
    if let Some(path) = &output_path {
        write_json_file(path, &output)?;
    }

    let report = json!({
        "epoch": args.epoch,
        "channel": output["channel"],
        "receipts": output["data"].as_array().map(Vec::len).unwrap_or(0),
        "output": output_path,
    });

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if let Some(path) = output_path {
        println!(
            "Collected {} unique receipt(s) from {}.",
            report["receipts"].as_u64().unwrap_or_default(),
            report["channel"].as_str().unwrap_or("")
        );
        println!("Copy/paste receipts file: {}", path.display());
    } else {
        println!("{}", serde_json::to_string_pretty(&output)?);
    }
    Ok(())
}

fn epoch_sidechannel(epoch: u64, requested: Option<&str>) -> String {
    requested
        .filter(|channel| !channel.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("mx/epoch/{epoch}"))
}

fn receipt_id(receipt: &Value) -> String {
    if let Some(body) = receipt_body(receipt) {
        if let (Some(session_id), Some(seq)) = (
            body.get("session_id").and_then(Value::as_str),
            body.get("seq").and_then(Value::as_u64),
        ) {
            return format!("{session_id}:{seq}");
        }
    }
    blake3::hash(&serde_json::to_vec(receipt).unwrap_or_else(|_| Vec::new()))
        .to_hex()
        .to_string()
}

fn epoch_receipt_message(epoch: u64, receipt_id: String, receipt: Value) -> Result<Value> {
    Ok(json!({
        "t": "epoch.receipt",
        "v": 1,
        "epoch": epoch,
        "receipt_id": receipt_id,
        "published_at_ms": unix_epoch_millis()?,
        "receipt": receipt,
    }))
}

fn epoch_receipt_from_sidechannel_event(
    event: &Value,
    epoch: u64,
    channel: &str,
) -> Option<(String, Value)> {
    if event.get("channel").and_then(Value::as_str) != Some(channel) {
        return None;
    }
    let message = event.get("message")?;
    if message.get("t").and_then(Value::as_str) != Some("epoch.receipt") {
        return None;
    }
    if message.get("epoch").and_then(Value::as_u64) != Some(epoch) {
        return None;
    }
    let receipt = message.get("receipt")?.clone();
    let receipt_id = message
        .get("receipt_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| receipt_id(&receipt));
    Some((receipt_id, receipt))
}

fn resolve_cli_sc_bridge(
    home: Option<&PathBuf>,
    sc_bridge_url: Option<&str>,
    sc_bridge_token: Option<&str>,
) -> Result<(String, String)> {
    let home = home.cloned().map(Ok).unwrap_or_else(default_home)?;
    let home = absolutize(home)?;
    let config = read_mayhem_config(&home)?;
    let url = sc_bridge_url
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| env::var("MAYHEM_SC_BRIDGE_URL").ok())
        .or_else(|| {
            config
                .as_ref()
                .and_then(|config| config.network.as_ref())
                .and_then(|network| network.sc_bridge_url.clone())
        })
        .unwrap_or_else(|| DEFAULT_SC_BRIDGE_URL.to_owned());
    let token = sc_bridge_token
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| env::var("MAYHEM_SC_BRIDGE_TOKEN").ok())
        .or_else(|| {
            config
                .as_ref()
                .and_then(|config| config.network.as_ref())
                .and_then(|network| network.sc_bridge_token.clone())
        })
        .context(
            "SC-Bridge token is required; pass --sc-bridge-token or set MAYHEM_SC_BRIDGE_TOKEN",
        )?;
    Ok((url, token))
}

fn resolve_cli_rpc_url(home: Option<&PathBuf>, rpc_url: Option<&str>) -> Result<String> {
    if let Some(rpc_url) = rpc_url {
        if !rpc_url.trim().is_empty() {
            return Ok(rpc_url.to_owned());
        }
    }
    let home = home.cloned().map(Ok).unwrap_or_else(default_home)?;
    let home = absolutize(home)?;
    let config = read_mayhem_config(&home)?;
    Ok(config
        .and_then(|config| config.network)
        .and_then(|network| network.rpc_url)
        .unwrap_or_else(|| DEFAULT_RPC_URL.to_owned()))
}

fn resolve_cli_paygate_url(config: Option<&MayhemConfig>, paygate_url: Option<&str>) -> String {
    if let Some(paygate_url) = paygate_url {
        let paygate_url = paygate_url.trim();
        if !paygate_url.is_empty() {
            return paygate_url.trim_end_matches('/').to_owned();
        }
    }
    if let Ok(paygate_url) = env::var("MAYHEM_PAYGATE_URL") {
        let paygate_url = paygate_url.trim();
        if !paygate_url.is_empty() {
            return paygate_url.trim_end_matches('/').to_owned();
        }
    }
    config
        .and_then(|config| config.network.as_ref())
        .and_then(|network| network.paygate_url.as_deref())
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim_end_matches('/').to_owned())
        .unwrap_or_else(|| DEFAULT_PAYGATE_URL.to_owned())
}

async fn resolve_cli_wallet(
    home: &Path,
    config: Option<&MayhemConfig>,
    peer_store_name: &str,
    password: &str,
) -> Result<WalletInfo> {
    let store_name = config
        .and_then(|config| config.identity.as_ref())
        .and_then(|identity| identity.store_name.as_deref())
        .unwrap_or(peer_store_name);
    let keypair_path = config
        .and_then(|config| config.identity.as_ref())
        .and_then(|identity| identity.keypair_path.as_deref())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            home.join("stores")
                .join(store_name)
                .join("db")
                .join("keypair.json")
        });
    inspect_wallet(&keypair_path, password)
        .await
        .with_context(|| format!("reading wallet {}", keypair_path.display()))
}

#[derive(Debug)]
struct PayCheckout {
    id: String,
    url: String,
    reference: Option<String>,
}

#[derive(Debug)]
struct PayCreditStatus {
    credited: bool,
    before_mu: u64,
    current_mu: u64,
    target_mu: u64,
    waited_ms: u64,
}

fn checkout_handoff_lines(rail: PayRail, amount_mu: u64, url: &str) -> [String; 2] {
    [
        format!(
            "Mayhem {} checkout for {}",
            rail.as_str(),
            mu_to_usd_amount(amount_mu)
        ),
        format!("Copy/paste checkout URL: {url}"),
    ]
}

fn emit_checkout_handoff(
    json_output: bool,
    rail: PayRail,
    amount_mu: u64,
    url: &str,
) -> Result<()> {
    let lines = checkout_handoff_lines(rail, amount_mu, url);
    if json_output {
        let mut stderr = io::stderr().lock();
        for line in lines {
            writeln!(stderr, "{line}")?;
        }
        stderr.flush()?;
    } else {
        let mut stdout = io::stdout().lock();
        for line in lines {
            writeln!(stdout, "{line}")?;
        }
        stdout.flush()?;
    }
    Ok(())
}

async fn create_pay_checkout(
    rail: PayRail,
    paygate_url: &str,
    who: &str,
    amount_mu: u64,
    idempotency_key: Option<&str>,
    success_url: Option<&str>,
    cancel_url: Option<&str>,
) -> Result<PayCheckout> {
    let client = reqwest::Client::new();
    let endpoint = match rail {
        PayRail::Stripe => "v1/stripe/checkout-sessions",
        PayRail::Coinbase => "v1/coinbase/charges",
    };
    let success_url = success_url
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| default_checkout_success_url(paygate_url, rail));
    let cancel_url = cancel_url
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| default_checkout_cancel_url(paygate_url, rail));
    let mut body = json!({
        "who": who,
        "mu": amount_mu,
    });
    match rail {
        PayRail::Stripe => {
            body["success_url"] = Value::String(success_url);
            body["cancel_url"] = Value::String(cancel_url);
            if let Some(idempotency_key) = idempotency_key.filter(|value| !value.is_empty()) {
                body["idempotency_key"] = Value::String(idempotency_key.to_owned());
            }
        }
        PayRail::Coinbase => {
            body["redirect_url"] = Value::String(success_url);
            body["cancel_url"] = Value::String(cancel_url);
        }
    }

    let response = client
        .post(format!(
            "{}/{}",
            paygate_url.trim_end_matches('/'),
            endpoint
        ))
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    let response_body = response.text().await?;
    if !status.is_success() {
        bail!("paygate returned {status}: {response_body}");
    }
    let value: Value = serde_json::from_str(&response_body)?;
    checkout_from_paygate_response(rail, &value)
}

fn checkout_from_paygate_response(rail: PayRail, value: &Value) -> Result<PayCheckout> {
    match rail {
        PayRail::Stripe => {
            let session = value
                .get("checkout_session")
                .ok_or_else(|| anyhow::anyhow!("paygate response missing checkout_session"))?;
            Ok(PayCheckout {
                id: required_json_string(session, "id")?,
                url: required_json_string(session, "url")?,
                reference: session
                    .get("payment_intent")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        }
        PayRail::Coinbase => {
            let charge = value
                .get("charge")
                .ok_or_else(|| anyhow::anyhow!("paygate response missing charge"))?;
            Ok(PayCheckout {
                id: required_json_string(charge, "id")?,
                url: required_json_string(charge, "hosted_url")?,
                reference: charge
                    .get("code")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        }
    }
}

fn required_json_string(value: &Value, field: &str) -> Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("paygate response missing {field}"))
}

fn default_checkout_success_url(paygate_url: &str, rail: PayRail) -> String {
    let base = paygate_url.trim_end_matches('/');
    match rail {
        PayRail::Stripe => {
            format!("{base}/v1/stripe/return?session_id={{CHECKOUT_SESSION_ID}}")
        }
        PayRail::Coinbase => format!("{base}/v1/coinbase/return"),
    }
}

fn default_checkout_cancel_url(paygate_url: &str, rail: PayRail) -> String {
    let base = paygate_url.trim_end_matches('/');
    match rail {
        PayRail::Stripe => format!("{base}/v1/stripe/cancel"),
        PayRail::Coinbase => format!("{base}/v1/coinbase/cancel"),
    }
}

async fn open_checkout_url(url: &str, disabled: bool) -> bool {
    if disabled {
        return false;
    }
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    command.status().await.is_ok_and(|status| status.success())
}

async fn wait_for_credit(
    rpc: &PeerRpcClient,
    who: &str,
    before_mu: u64,
    target_mu: u64,
    timeout: Duration,
    interval: Duration,
) -> Result<PayCreditStatus> {
    let started = Instant::now();
    loop {
        let current_mu = read_user_balance_mu(rpc, who).await?;
        if current_mu >= target_mu {
            return Ok(PayCreditStatus {
                credited: true,
                before_mu,
                current_mu,
                target_mu,
                waited_ms: millis_since(started),
            });
        }
        if started.elapsed() >= timeout {
            return Ok(PayCreditStatus {
                credited: false,
                before_mu,
                current_mu,
                target_mu,
                waited_ms: millis_since(started),
            });
        }
        sleep(interval).await;
    }
}

fn millis_since(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

async fn read_user_balance_mu(rpc: &PeerRpcClient, who: &str) -> Result<u64> {
    read_balance_record(rpc, who)
        .await?
        .get("mu")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("normalized balance record for {who} is missing mu"))
}

async fn read_balance_record(rpc: &PeerRpcClient, who: &str) -> Result<Value> {
    let value = read_state_value(rpc, &format!("bal/{who}")).await?;
    normalize_balance_record(who, value)
}

fn normalize_balance_record(who: &str, value: Option<Value>) -> Result<Value> {
    let mut record = value.unwrap_or_else(|| {
        json!({
            "user": who,
            "denom": "mu_usd",
            "mu": 0,
            "updated_epoch": 0,
            "updated_at": null,
        })
    });
    let object = record
        .as_object_mut()
        .context("balance record must be a JSON object")?;
    match object.get("denom").and_then(Value::as_str) {
        Some("mu_usd") | None => {
            object
                .entry("denom")
                .or_insert_with(|| Value::String("mu_usd".to_owned()));
        }
        Some(denom) => bail!("balance record for {who} has unsupported denomination {denom}"),
    }
    match object.get("user").and_then(Value::as_str) {
        Some(user) if user == who => {}
        Some(user) => bail!("balance record user mismatch: expected {who}, got {user}"),
        None => {
            object.insert("user".to_owned(), Value::String(who.to_owned()));
        }
    }
    if object.get("mu").and_then(Value::as_u64).is_none() {
        bail!("balance record for {who} is missing non-negative integer mu");
    }
    object
        .entry("updated_epoch")
        .or_insert_with(|| Value::Number(0_u64.into()));
    object.entry("updated_at").or_insert(Value::Null);
    Ok(record)
}

fn parse_usd_amount_to_mu(amount: &str) -> Result<u64> {
    let amount = amount.trim();
    if amount.is_empty() {
        bail!("--amount must be positive");
    }
    if amount.starts_with('-') || amount.starts_with('+') {
        bail!("--amount must be positive USD");
    }
    let (dollars, cents) = match amount.split_once('.') {
        Some((dollars, cents)) => {
            if cents.is_empty() || cents.len() > 2 {
                bail!("--amount supports at most two decimal places");
            }
            (dollars, cents)
        }
        None => (amount, ""),
    };
    if dollars.is_empty()
        || !dollars.as_bytes().iter().all(u8::is_ascii_digit)
        || !cents.as_bytes().iter().all(u8::is_ascii_digit)
    {
        bail!("--amount must be a USD decimal, for example 10 or 10.25");
    }
    let dollars = dollars.parse::<u64>()?;
    let cents = match cents.len() {
        0 => 0,
        1 => cents.parse::<u64>()? * 10,
        2 => cents.parse::<u64>()?,
        _ => unreachable!("length checked above"),
    };
    let total_cents = dollars
        .checked_mul(100)
        .and_then(|value| value.checked_add(cents))
        .context("--amount overflowed")?;
    if total_cents == 0 {
        bail!("--amount must be positive");
    }
    total_cents
        .checked_mul(10_000)
        .context("--amount overflowed mu_usd")
}

fn mu_to_usd_amount(mu: u64) -> String {
    let cents = mu / 10_000;
    format!("{}.{:02}", cents / 100, cents % 100)
}

fn earning_view(record: LedgerEarningRecord) -> Result<EarningsView> {
    let locked = record
        .held_mu
        .checked_add(record.paid_cum_mu)
        .context("earning held_mu + paid_cum_mu overflowed")?;
    if record.total_mu < locked {
        bail!(
            "earning record for {} violates total >= held + paid",
            record.provider
        );
    }
    Ok(EarningsView {
        provider: record.provider,
        denom: record.denom,
        total_mu: record.total_mu,
        held_mu: record.held_mu,
        paid_cum_mu: record.paid_cum_mu,
        released_mu: record.total_mu - locked,
        holdbacks: record.holdbacks,
        updated_epoch: record.updated_epoch,
        last_payout_msb_tx_hash: record.last_payout_msb_tx_hash,
    })
}

fn print_payouts_report(report: &Value) {
    if let Some(epoch) = report.get("epoch").and_then(Value::as_u64) {
        println!("Payout evidence for epoch {epoch}");
        print_optional_evidence("pay", report.get("pay"));
        print_optional_evidence("fee", report.get("fee"));
        return;
    }
    println!("Payout evidence");
    for entry in report
        .get("pay")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let key = entry.get("key").and_then(Value::as_str).unwrap_or("");
        let value = entry.get("value").unwrap_or(&Value::Null);
        println!(
            "{key}: count={} mu_total={} root={}",
            value.get("count").and_then(Value::as_u64).unwrap_or(0),
            value.get("mu_total").and_then(Value::as_u64).unwrap_or(0),
            value
                .get("merkle_root")
                .and_then(Value::as_str)
                .unwrap_or("")
        );
    }
    for entry in report
        .get("fee_sweeps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let key = entry.get("key").and_then(Value::as_str).unwrap_or("");
        let value = entry.get("value").unwrap_or(&Value::Null);
        println!(
            "{key}: fee sweep mu={} msb_tx={}",
            value.get("sweep_mu").and_then(Value::as_u64).unwrap_or(0),
            value
                .get("sweep_msb_tx_hash")
                .and_then(Value::as_str)
                .unwrap_or("")
        );
    }
}

fn print_optional_evidence(label: &str, value: Option<&Value>) {
    match value {
        Some(Value::Null) | None => println!("{label}: missing"),
        Some(value) => println!(
            "{label}: {}",
            serde_json::to_string(value).unwrap_or_default()
        ),
    }
}

fn print_earnings_report(report: &Value) {
    println!("Provider earnings");
    for entry in report
        .get("earnings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        println!(
            "{}: total={} held={} paid={} released={} {}",
            entry.get("provider").and_then(Value::as_str).unwrap_or(""),
            entry.get("total_mu").and_then(Value::as_u64).unwrap_or(0),
            entry.get("held_mu").and_then(Value::as_u64).unwrap_or(0),
            entry
                .get("paid_cum_mu")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            entry
                .get("released_mu")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            entry
                .get("denom")
                .and_then(Value::as_str)
                .unwrap_or("mu_usd")
        );
    }
}

async fn read_receipts_for_export(args: &ReceiptsExportArgs) -> Result<Vec<Value>> {
    if let Some(path) = args.receipts_file.as_deref() {
        return read_json_array(path, "receipts");
    }
    let base = args
        .gateway_url
        .as_deref()
        .unwrap_or(DEFAULT_GATEWAY_URL)
        .trim_end_matches('/');
    fetch_gateway_receipts(base).await
}

async fn fetch_gateway_receipts(gateway_url: &str) -> Result<Vec<Value>> {
    let url = format!("{}/mayhem/receipts", gateway_url.trim_end_matches('/'));
    let response: Value = reqwest::get(&url)
        .await
        .with_context(|| format!("fetching {url}"))?
        .error_for_status()
        .with_context(|| format!("gateway returned an error for {url}"))?
        .json()
        .await
        .with_context(|| format!("parsing {url} response"))?;
    response
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .context("gateway /mayhem/receipts response did not include data[]")
}

fn read_optional_json_array(path: Option<&Path>, label: &str) -> Result<Vec<Value>> {
    match path {
        Some(path) => read_json_array(path, label),
        None => Ok(Vec::new()),
    }
}

fn read_json_array(path: &Path, label: &str) -> Result<Vec<Value>> {
    let value = read_json_file(path)?;
    if let Some(array) = value.as_array() {
        return Ok(array.clone());
    }
    if let Some(array) = value.get("data").and_then(Value::as_array) {
        return Ok(array.clone());
    }
    bail!(
        "{label} file {} must be a JSON array or object with data[]",
        path.display()
    );
}

fn read_prior_earnings(path: Option<&Path>) -> Result<BTreeMap<String, u64>> {
    let Some(path) = path else {
        return Ok(BTreeMap::new());
    };
    let value = read_json_file(path)?;
    let object = value
        .as_object()
        .with_context(|| format!("{} must contain a JSON object", path.display()))?;
    let mut out = BTreeMap::new();
    for (provider, value) in object {
        let mu = value
            .as_u64()
            .or_else(|| value.get("total_mu").and_then(Value::as_u64))
            .with_context(|| {
                format!("prior earning for {provider} must be a u64 or record.total_mu")
            })?;
        out.insert(provider.clone(), mu);
    }
    Ok(out)
}

async fn read_evidence_for_export(args: &ReceiptsExportArgs) -> Result<EpochEvidenceSnapshot> {
    if let Some(path) = args.evidence_file.as_deref() {
        return read_evidence_file(path, args.epoch);
    }
    let rpc_url = resolve_cli_rpc_url(args.home.as_ref(), args.rpc_url.as_deref())?;
    let rpc = PeerRpcClient::new(&rpc_url)?;
    Ok(EpochEvidenceSnapshot {
        dep: read_state_value(&rpc, &format!("ev/dep/{}", args.epoch)).await?,
        r#use: read_state_value(&rpc, &format!("ev/use/{}", args.epoch)).await?,
        earn: read_state_value(&rpc, &format!("ev/earn/{}", args.epoch)).await?,
        fee: read_state_value(&rpc, &format!("ev/fee/{}", args.epoch)).await?,
        pay: read_state_value(&rpc, &format!("ev/pay/{}", args.epoch)).await?,
    })
}

fn read_evidence_file(path: &Path, epoch: u64) -> Result<EpochEvidenceSnapshot> {
    let value = read_json_file(path)?;
    Ok(EpochEvidenceSnapshot {
        dep: evidence_record(&value, epoch, "dep"),
        r#use: evidence_record(&value, epoch, "use"),
        earn: evidence_record(&value, epoch, "earn"),
        fee: evidence_record(&value, epoch, "fee"),
        pay: evidence_record(&value, epoch, "pay"),
    })
}

fn evidence_record(value: &Value, epoch: u64, kind: &str) -> Option<Value> {
    value
        .get(kind)
        .cloned()
        .or_else(|| value.get(format!("ev/{kind}/{epoch}")).cloned())
}

fn read_json_file(path: &Path) -> Result<Value> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
}

fn write_json_file(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_vec_pretty(value)?)
        .with_context(|| format!("writing {}", path.display()))
}

fn temp_audit_bundle_path(epoch: u64) -> Result<PathBuf> {
    Ok(env::temp_dir().join(format!(
        "mayhem-receipts-export-{epoch}-{}-{}.json",
        std::process::id(),
        unix_epoch_millis()?
    )))
}

async fn run_epoch_recompute_script(script: &Path, bundle: &Path) -> Result<Value> {
    let output = Command::new("node")
        .arg(script)
        .arg(bundle)
        .output()
        .await
        .with_context(|| format!("running {}", script.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{} failed: {}", script.display(), stderr.trim());
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("parsing {} output", script.display()))
}

fn verify_epoch_evidence(
    epoch: u64,
    recomputed: &Value,
    evidence: &EpochEvidenceSnapshot,
) -> Vec<EvidenceCheck> {
    let roots = &recomputed["roots"];
    let totals = &recomputed["totals"];
    let mut checks = Vec::new();
    push_evidence_check(
        &mut checks,
        &format!("ev/dep/{epoch}.merkle_root"),
        &roots["dep"],
        evidence.dep.as_ref(),
        "merkle_root",
    );
    push_evidence_check(
        &mut checks,
        &format!("ev/dep/{epoch}.count"),
        &totals["dep_count"],
        evidence.dep.as_ref(),
        "count",
    );
    push_evidence_check(
        &mut checks,
        &format!("ev/dep/{epoch}.mu_total"),
        &totals["dep_mu"],
        evidence.dep.as_ref(),
        "mu_total",
    );
    push_evidence_check(
        &mut checks,
        &format!("ev/use/{epoch}.merkle_root"),
        &roots["use"],
        evidence.r#use.as_ref(),
        "merkle_root",
    );
    push_evidence_check(
        &mut checks,
        &format!("ev/use/{epoch}.sessions"),
        &totals["use_count"],
        evidence.r#use.as_ref(),
        "sessions",
    );
    push_evidence_check(
        &mut checks,
        &format!("ev/use/{epoch}.mu_total"),
        &totals["use_mu"],
        evidence.r#use.as_ref(),
        "mu_total",
    );
    push_evidence_check(
        &mut checks,
        &format!("ev/use/{epoch}.providers"),
        &totals["provider_count"],
        evidence.r#use.as_ref(),
        "providers",
    );
    push_evidence_check(
        &mut checks,
        &format!("ev/earn/{epoch}.merkle_root"),
        &roots["earn"],
        evidence.earn.as_ref(),
        "merkle_root",
    );
    push_evidence_check(
        &mut checks,
        &format!("ev/earn/{epoch}.provider_count"),
        &totals["provider_count"],
        evidence.earn.as_ref(),
        "provider_count",
    );
    push_evidence_check(
        &mut checks,
        &format!("ev/earn/{epoch}.mu_cum_total"),
        &totals["earn_mu"],
        evidence.earn.as_ref(),
        "mu_cum_total",
    );
    push_evidence_check(
        &mut checks,
        &format!("ev/fee/{epoch}.merkle_root"),
        &roots["fee"],
        evidence.fee.as_ref(),
        "merkle_root",
    );
    push_evidence_check(
        &mut checks,
        &format!("ev/fee/{epoch}.mu_fee_epoch"),
        &totals["fee_mu"],
        evidence.fee.as_ref(),
        "mu_fee_epoch",
    );
    push_evidence_check(
        &mut checks,
        &format!("ev/fee/{epoch}.mu_fee_cum"),
        &totals["fee_cum_mu"],
        evidence.fee.as_ref(),
        "mu_fee_cum",
    );
    push_evidence_check(
        &mut checks,
        &format!("ev/pay/{epoch}.merkle_root"),
        &roots["pay"],
        evidence.pay.as_ref(),
        "merkle_root",
    );
    push_evidence_check(
        &mut checks,
        &format!("ev/pay/{epoch}.count"),
        &totals["pay_count"],
        evidence.pay.as_ref(),
        "count",
    );
    push_evidence_check(
        &mut checks,
        &format!("ev/pay/{epoch}.mu_total"),
        &totals["pay_mu"],
        evidence.pay.as_ref(),
        "mu_total",
    );
    checks
}

fn push_evidence_check(
    checks: &mut Vec<EvidenceCheck>,
    key: &str,
    expected: &Value,
    record: Option<&Value>,
    field: &str,
) {
    let actual = record
        .and_then(|value| value.get(field))
        .cloned()
        .unwrap_or(Value::Null);
    checks.push(EvidenceCheck {
        key: key.to_owned(),
        ok: &actual == expected,
        expected: expected.clone(),
        actual,
    });
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LedgerEnclave {
    enclave_id: String,
    model_id: String,
    backend: String,
    artifact_root: String,
    manifest_hash: String,
    att_tier: u8,
    binary_hash: String,
    #[serde(default)]
    caps: Value,
    status: String,
    created_by: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LedgerRoom {
    room_id: String,
    sidechannel: String,
    model_id: String,
    #[serde(default)]
    label: String,
    status: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LedgerRoomServe {
    room_id: String,
    provider: String,
    enclave_id: String,
    model_id: String,
    status: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LedgerProvider {
    provider: String,
    status: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LedgerPriceSchedule {
    enclave_id: String,
    model_id: String,
    denom: String,
    current: Option<LedgerPriceRecord>,
    pending: Option<LedgerPriceRecord>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LedgerPriceRecord {
    ver: u64,
    denom: String,
    in_per_1k_mu: u64,
    out_per_1k_mu: u64,
    per_req_mu: u64,
    min_session_mu: u64,
    effective_at: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PrefixStateResponse {
    values: Vec<PrefixStateEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PrefixStateEntry {
    key: String,
    value: Value,
}

#[derive(Debug, Clone)]
struct ContractCatalog {
    enclaves: Vec<LedgerEnclave>,
    rooms: Vec<LedgerRoom>,
    roomserve: Vec<LedgerRoomServe>,
    providers: Vec<LedgerProvider>,
    prices: Vec<LedgerPriceSchedule>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LedgerHoldbackBucket {
    epoch: u64,
    mu: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LedgerEarningRecord {
    provider: String,
    denom: String,
    total_mu: u64,
    held_mu: u64,
    paid_cum_mu: u64,
    #[serde(default)]
    holdbacks: Vec<LedgerHoldbackBucket>,
    #[serde(default)]
    updated_epoch: u64,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    last_holdback_release_epoch: Option<u64>,
    #[serde(default)]
    last_payout_rate_ts: Option<u64>,
    #[serde(default)]
    last_payout_msb_tx_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct EarningsView {
    provider: String,
    denom: String,
    total_mu: u64,
    held_mu: u64,
    paid_cum_mu: u64,
    released_mu: u64,
    holdbacks: Vec<LedgerHoldbackBucket>,
    updated_epoch: u64,
    last_payout_msb_tx_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EpochAuditBundle {
    epoch: u64,
    fee_bps: u64,
    #[serde(default)]
    deposits: Vec<Value>,
    #[serde(default)]
    receipts: Vec<Value>,
    #[serde(default)]
    payouts: Vec<Value>,
    #[serde(default)]
    prior_earnings: BTreeMap<String, u64>,
    #[serde(default)]
    prior_fee_cum_mu: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct EpochEvidenceSnapshot {
    #[serde(default)]
    dep: Option<Value>,
    #[serde(default)]
    r#use: Option<Value>,
    #[serde(default)]
    earn: Option<Value>,
    #[serde(default)]
    fee: Option<Value>,
    #[serde(default)]
    pay: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
struct EvidenceCheck {
    key: String,
    ok: bool,
    expected: Value,
    actual: Value,
}

#[derive(Debug, Clone, Serialize)]
struct ReceiptsExportReport {
    bundle: EpochAuditBundle,
    bundle_path: Option<PathBuf>,
    recomputed: Option<Value>,
    evidence: Option<EpochEvidenceSnapshot>,
    checks: Vec<EvidenceCheck>,
    verified: bool,
}

#[derive(Debug, Clone)]
struct ProviderCandidate {
    enclave: LedgerEnclave,
    model: catalog::CatalogModel,
    artifact_name: String,
    artifact: catalog::CatalogArtifact,
    verdict: BackendVerdict,
    price: Option<LedgerPriceSchedule>,
}

struct HeartbeatContext<'a> {
    args: &'a ProviderStartArgs,
    config: &'a Option<MayhemConfig>,
    keypair_path: &'a Path,
    password: &'a str,
    wallet: &'a WalletInfo,
    selected: &'a ProviderCandidate,
    rooms: &'a [LedgerRoom],
    attestation: &'a Tier1AttestationReport,
    attestation_head: &'a str,
}

struct ProviderSessionContext<'a> {
    args: &'a ProviderStartArgs,
    keypair_path: &'a Path,
    password: &'a str,
    wallet: &'a WalletInfo,
    selected: &'a ProviderCandidate,
    artifact_path: &'a Path,
    rooms: &'a [LedgerRoom],
    attestation: &'a Tier1AttestationReport,
    attestation_head: &'a str,
    rules: &'a RulesRef,
}

#[derive(Clone, Debug)]
struct ActiveProviderSession {
    user: String,
    session_id: String,
}

#[derive(Clone, Debug)]
struct ProviderSessionTerms {
    provider: String,
    enclave_id: String,
    model_id: String,
    price_ver: u64,
    rules_ver: u64,
    ctx: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ProviderSessionDecision {
    Accept,
    Reject { code: &'static str, reason: String },
}

trait ProviderSessionResponder {
    fn mode(&self) -> &'static str;
    fn respond(
        &mut self,
        terms: &ProviderSessionTerms,
        body: &Value,
    ) -> Result<ProviderSessionOutput>;
}

struct DeterministicProviderSessionResponder;

impl ProviderSessionResponder for DeterministicProviderSessionResponder {
    fn mode(&self) -> &'static str {
        "deterministic-dev-shim"
    }

    fn respond(
        &mut self,
        terms: &ProviderSessionTerms,
        body: &Value,
    ) -> Result<ProviderSessionOutput> {
        Ok(provider_session_response(terms, body))
    }
}

struct EngineProviderSessionResponder {
    backend: Box<dyn EngineBackend>,
}

impl ProviderSessionResponder for EngineProviderSessionResponder {
    fn mode(&self) -> &'static str {
        "mayhem-engine"
    }

    fn respond(
        &mut self,
        _terms: &ProviderSessionTerms,
        body: &Value,
    ) -> Result<ProviderSessionOutput> {
        provider_engine_session_response(self.backend.as_mut(), body)
    }
}

async fn provider_start(mut args: ProviderStartArgs) -> Result<()> {
    let home = args.home.clone().map(Ok).unwrap_or_else(default_home)?;
    let home = absolutize(home)?;
    let config = read_mayhem_config(&home)?;
    if args.engine_backend == "auto" {
        if let Some(config_backend) = config
            .as_ref()
            .and_then(|config| config.provider.as_ref())
            .and_then(|provider| provider.engine_backend.clone())
            .filter(|backend| !backend.trim().is_empty())
        {
            args.engine_backend = config_backend;
        }
    }
    let rpc_url = args
        .rpc_url
        .clone()
        .or_else(|| {
            config
                .as_ref()
                .and_then(|config| config.network.as_ref())
                .and_then(|network| network.rpc_url.clone())
        })
        .unwrap_or_else(|| DEFAULT_RPC_URL.to_owned());
    let store_name = config
        .as_ref()
        .and_then(|config| config.identity.as_ref())
        .and_then(|identity| identity.store_name.clone())
        .unwrap_or_else(|| "main".to_owned());
    let keypair_path = config
        .as_ref()
        .and_then(|config| config.identity.as_ref())
        .and_then(|identity| identity.keypair_path.as_ref())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            home.join("stores")
                .join(&store_name)
                .join("db")
                .join("keypair.json")
        });
    let password = args.wallet_password.clone().unwrap_or_default();
    let wallet = inspect_wallet(&keypair_path, &password).await?;
    let rpc = PeerRpcClient::new(&rpc_url)?;
    let rules = resolve_rules(None, None, &rpc, None).await?;

    provider_log(
        &args,
        "Reading admin catalog and canonical rooms from contract state",
    );
    let contract = read_contract_catalog(&rpc).await?;
    let catalog_path = args
        .catalog_path
        .clone()
        .map(Ok)
        .unwrap_or_else(|| repo_path("catalog/models.json"))?;
    let catalog_path = absolutize(catalog_path)?;
    let catalog_hash = verify_or_hash_catalog(&args, &catalog_path)?;
    let catalog_doc = catalog::load_document(&catalog_path)?;

    provider_log(&args, "Running hwprobe and selecting a backend");
    let hardware = provider_hwprobe(&args)?;
    let candidates = build_provider_candidates(&contract, &catalog_doc, &hardware, &args)?;
    let selected = select_provider_candidate(&candidates, args.enclave.as_deref())?;
    let rooms = select_provider_rooms(&contract.rooms, &selected.enclave, &args.rooms)?;
    if rooms.is_empty() {
        bail!(
            "no open admin-created canonical rooms found for {}; ask the admin to open one",
            selected.enclave.model_id
        );
    }

    provider_log(
        &args,
        &format!(
            "Selected {} via {} ({})",
            selected.enclave.model_id, selected.enclave.backend, selected.artifact_name
        ),
    );
    let provider_secret = derive_provider_secret(&keypair_path, &password, &wallet).await?;
    let downloads_dir = args
        .downloads_dir
        .clone()
        .unwrap_or_else(|| home.join("downloads"));
    let downloads_dir = absolutize(downloads_dir)?;
    let artifact_path = download_provider_artifact(&args, &downloads_dir, &selected).await?;

    provider_log(
        &args,
        "Verifying, sealing, and boot-checking the enclave artifact",
    );
    let sealed_store = home
        .join("enclaves")
        .join(safe_path_component(&selected.enclave.model_id))
        .join(&selected.enclave.enclave_id);
    let key_context = KeyContext {
        provider_id: wallet.public_key.clone(),
        enclave_id: selected.enclave.enclave_id.clone(),
        artifact_root: selected.enclave.artifact_root.clone(),
        manifest_hash: selected.enclave.manifest_hash.clone(),
    };
    let seal_report = seal_provider_artifact(
        &artifact_path,
        &sealed_store,
        &key_context,
        &provider_secret,
        args.chunk_size,
    )?;
    let boot_report = boot_sealed_store(&BootOptions {
        store_dir: sealed_store.clone(),
        key_context: key_context.clone(),
        provider_secret: provider_secret.clone(),
        output_path: None,
        expected_merkle_root: Some(selected.enclave.artifact_root.clone()),
    })?;

    provider_log(&args, "Preparing Tier-1 attestation");
    let runtime_context = RuntimeKeyContext {
        provider_id: wallet.public_key.clone(),
        enclave_id: selected.enclave.enclave_id.clone(),
    };
    let runtime_keypair = load_or_create_runtime_keypair_store(&RuntimeKeypairStoreOptions::new(
        sealed_store.join("runtime-keypair.json"),
        runtime_context,
        provider_secret.clone(),
    ))?;
    let binary_path = args
        .enclave_binary
        .clone()
        .map(absolutize)
        .transpose()?
        .unwrap_or(std::env::current_exe()?);
    let binary_hash = measure_binary(&binary_path)?;
    if binary_hash != selected.enclave.binary_hash {
        bail!(
            "measured enclave binary hash {} does not match admin enclave record {}; rebuild or ask the admin to update the enclave",
            binary_hash,
            selected.enclave.binary_hash
        );
    }
    let now = unix_epoch_seconds()?;
    let nonce_u = blake3::hash(
        format!(
            "mayhem-provider-start:{}:{}:{}",
            wallet.public_key, selected.enclave.enclave_id, now
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string();
    let draft = prepare_tier1_attestation_report(&Tier1ExternalProviderAttestationOptions {
        identity: CatalogEnclaveIdentity {
            admin_pubkey: selected.enclave.created_by.clone(),
            model_id: selected.enclave.model_id.clone(),
            artifact_root: selected.enclave.artifact_root.clone(),
            manifest_hash: selected.enclave.manifest_hash.clone(),
            binary_hash,
        },
        runtime_keypair,
        provider_pubkey: wallet.public_key.clone(),
        binary_path,
        boot_epoch: now,
        report_ts: now,
        nonce_u,
    })?;
    let provider_attestation_sig = sign_hex(
        &keypair_path,
        &password,
        &draft.provider_signing_message_hex,
    )
    .await?;
    let attestation = finalize_tier1_attestation_report(draft, provider_attestation_sig)?;
    if attestation.report.enclave_id != selected.enclave.enclave_id {
        bail!(
            "admin enclave id {} is not bound to the measured identity {}; providers cannot serve unbound enclaves",
            selected.enclave.enclave_id,
            attestation.report.enclave_id
        );
    }

    let session_responder = if args.serve_sessions {
        Some(provider_session_responder(&ProviderSessionContext {
            args: &args,
            keypair_path: &keypair_path,
            password: &password,
            wallet: &wallet,
            selected: &selected,
            artifact_path: &artifact_path,
            rooms: &rooms,
            attestation: &attestation,
            attestation_head: &attestation.report_head,
            rules: &rules,
        })?)
    } else {
        None
    };

    provider_log(&args, "Submitting provider opt-in transactions");
    let provider_tx =
        ensure_provider_registered(&rpc, &keypair_path, &password, &wallet, args.sim).await?;
    let serve_tx = ensure_joined_enclave(
        &rpc,
        &keypair_path,
        &password,
        &wallet,
        &selected.enclave,
        args.sim,
    )
    .await?;
    let room_txs = ensure_joined_rooms(
        &rpc,
        &keypair_path,
        &password,
        &wallet,
        &selected.enclave,
        &rooms,
        args.sim,
    )
    .await?;

    let heartbeats = if args.no_heartbeat {
        Vec::new()
    } else {
        emit_provider_heartbeats(HeartbeatContext {
            args: &args,
            config: &config,
            keypair_path: &keypair_path,
            password: &password,
            wallet: &wallet,
            selected: &selected,
            rooms: &rooms,
            attestation: &attestation,
            attestation_head: &attestation.report_head,
        })
        .await?
    };
    let heartbeat_status = if heartbeats.is_empty() {
        "joined_no_heartbeat"
    } else {
        "heartbeats_flowing"
    };
    let report = json!({
        "status": heartbeat_status,
        "home": home,
        "provider": wallet.public_key.clone(),
        "catalog": {
            "path": catalog_path,
            "hash": catalog_hash,
        },
        "hardware": {
            "source": hardware.source,
            "selected_backend": hardware.selected_backend,
            "summary": hardware.summary,
        },
        "enclave": selected.enclave.clone(),
        "artifact": {
            "name": selected.artifact_name.clone(),
            "engine": selected.artifact.engine.clone(),
            "path": artifact_path,
            "root": selected.enclave.artifact_root.clone(),
        },
        "sealed_store": {
            "path": sealed_store,
            "sealed": seal_report,
            "boot": boot_report,
        },
        "attestation": {
            "head": attestation.report_head,
            "enclave_pubkey": attestation.report.enclave_pubkey,
            "att_tier": attestation.report.att_tier,
        },
        "rules": &rules,
        "rooms": rooms.clone(),
        "transactions": {
            "provider": provider_tx,
            "serve": serve_tx,
            "rooms": room_txs,
        },
        "heartbeats": heartbeats,
        "self_test": {
            "ok": true,
            "kind": "sealed-boot-attestation-heartbeat",
        },
    });

    if args.print_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if heartbeat_status == "heartbeats_flowing" {
        println!("Provider start complete: heartbeats flowing.");
        println!("Enclave: {}", selected.enclave.enclave_id);
        println!("Model: {}", selected.enclave.model_id);
        println!(
            "Rooms: {}",
            rooms
                .iter()
                .map(|room| room.room_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    } else {
        println!("Provider start complete: joined canonical rooms; heartbeat bridge was not used.");
    }

    if let Some(responder) = session_responder {
        serve_provider_sessions(
            ProviderSessionContext {
                args: &args,
                keypair_path: &keypair_path,
                password: &password,
                wallet: &wallet,
                selected: &selected,
                artifact_path: &artifact_path,
                rooms: &rooms,
                attestation: &attestation,
                attestation_head: &attestation.report_head,
                rules: &rules,
            },
            responder,
        )
        .await?;
    }

    Ok(())
}

fn provider_log(args: &ProviderStartArgs, message: &str) {
    if !args.print_json {
        println!("-> {message}");
    }
}

fn verify_or_hash_catalog(args: &ProviderStartArgs, catalog_path: &Path) -> Result<String> {
    if args.dev_skip_catalog_verify {
        let bytes = fs::read(catalog_path)
            .with_context(|| format!("reading {}", catalog_path.display()))?;
        return Ok(blake3::hash(&bytes).to_hex().to_string());
    }

    let signature_path = args
        .signature_path
        .clone()
        .map(Ok)
        .unwrap_or_else(|| repo_path("catalog/signatures/models.json.sig"))?;
    let keys_dir = args
        .keys_dir
        .clone()
        .map(Ok)
        .unwrap_or_else(|| repo_path("catalog/keys"))?;
    let canaries_dir = args
        .canaries_dir
        .clone()
        .map(Ok)
        .unwrap_or_else(|| repo_path("catalog/canaries"))?;
    let report = catalog::verify(catalog::VerifyOptions {
        catalog_path: catalog_path.to_path_buf(),
        signature_path: absolutize(signature_path)?,
        keys_dir: absolutize(keys_dir)?,
        canaries_dir: absolutize(canaries_dir)?,
        check_dev_downloads: false,
        hf_token_file: None,
    })?;
    if !report.ok {
        bail!("catalog verification failed: {}", report.errors.join("; "));
    }
    Ok(report.catalog_hash)
}

fn provider_hwprobe(args: &ProviderStartArgs) -> Result<HardwareReport> {
    let fixture = args
        .fixture
        .as_deref()
        .map(|value| {
            FixtureProfile::parse(value).with_context(|| {
                format!(
                    "unknown fixture {value}; expected apple-silicon, linux-nvidia, or cpu-only"
                )
            })
        })
        .transpose()?;
    let mut options = ProbeOptions::default();
    if let Some(path) = &args.disk_path {
        options.disk_path = absolutize(path.clone())?;
    }
    options.run_disk_bench = !args.skip_disk_bench;
    options.fixture = fixture;
    Ok(probe(options))
}

async fn read_contract_catalog(rpc: &PeerRpcClient) -> Result<ContractCatalog> {
    let prices = read_prefix_entries(rpc, "price/")
        .await?
        .into_iter()
        .filter(|entry| {
            entry
                .key
                .strip_prefix("price/")
                .is_some_and(|tail| !tail.contains('/'))
        })
        .map(|entry| serde_json::from_value(entry.value).context("parsing price schedule"))
        .collect::<Result<Vec<LedgerPriceSchedule>>>()?;
    Ok(ContractCatalog {
        enclaves: read_prefix_values(rpc, "enclave/").await?,
        rooms: read_prefix_values(rpc, "room/").await?,
        roomserve: read_prefix_values(rpc, "roomserve/").await?,
        providers: read_prefix_values(rpc, "prov/").await?,
        prices,
    })
}

async fn read_prefix_values<T>(rpc: &PeerRpcClient, prefix: &str) -> Result<Vec<T>>
where
    T: DeserializeOwned,
{
    read_prefix_entries(rpc, prefix)
        .await?
        .into_iter()
        .map(|entry| serde_json::from_value(entry.value).context("parsing contract state record"))
        .collect()
}

async fn read_prefix_entries(rpc: &PeerRpcClient, prefix: &str) -> Result<Vec<PrefixStateEntry>> {
    let response = rpc
        .state_prefix(prefix, Some(false), Some(1000))
        .await
        .with_context(|| format!("reading {prefix}* from peer RPC"))?;
    let parsed: PrefixStateResponse = serde_json::from_value(response)
        .with_context(|| format!("parsing {prefix}* state response"))?;
    Ok(parsed
        .values
        .into_iter()
        .filter(|entry| entry.key.starts_with(prefix))
        .collect())
}

fn gateway_models_from_contract(contract: &ContractCatalog) -> Result<Vec<GatewayModel>> {
    let active_enclaves = contract
        .enclaves
        .iter()
        .filter(|enclave| enclave.status == "active")
        .map(|enclave| (enclave.enclave_id.clone(), enclave))
        .collect::<BTreeMap<_, _>>();
    let open_rooms = contract
        .rooms
        .iter()
        .filter(|room| room.status == "open")
        .collect::<Vec<_>>();
    let active_providers = contract
        .providers
        .iter()
        .filter(|provider| provider.status == "active")
        .map(|provider| provider.provider.as_str())
        .collect::<BTreeSet<_>>();
    let mut room_ids = BTreeSet::new();
    let mut rooms_by_model: BTreeMap<String, u32> = BTreeMap::new();
    for room in &open_rooms {
        room_ids.insert(room.room_id.clone());
        let count = rooms_by_model.entry(room.model_id.clone()).or_default();
        *count = count.saturating_add(1);
    }

    let mut prices_by_enclave = BTreeMap::new();
    for schedule in &contract.prices {
        let Some(price) = schedule.current.as_ref() else {
            continue;
        };
        if schedule.denom != "mu_usd" || price.denom != "mu_usd" {
            bail!(
                "price schedule for enclave {} uses unsupported denomination",
                schedule.enclave_id
            );
        }
        prices_by_enclave.insert(schedule.enclave_id.clone(), price.clone());
    }

    let mut caps_by_model: BTreeMap<String, ModelCaps> = BTreeMap::new();
    for enclave in active_enclaves.values() {
        if rooms_by_model.get(&enclave.model_id).copied().unwrap_or(0) == 0 {
            continue;
        }
        if !prices_by_enclave.contains_key(&enclave.enclave_id) {
            continue;
        };
        caps_by_model
            .entry(enclave.model_id.clone())
            .and_modify(|caps| merge_model_caps(caps, &gateway_caps_from_contract(&enclave.caps)))
            .or_insert_with(|| gateway_caps_from_contract(&enclave.caps));
    }

    let mut served_price_by_model: BTreeMap<String, LedgerPriceRecord> = BTreeMap::new();
    for serving in contract
        .roomserve
        .iter()
        .filter(|serving| serving.status == "active")
    {
        if !active_providers.contains(serving.provider.as_str()) {
            continue;
        }
        if !room_ids.contains(&serving.room_id) {
            continue;
        }
        let Some(enclave) = active_enclaves.get(&serving.enclave_id) else {
            continue;
        };
        let Some(serving_price) = prices_by_enclave.get(&serving.enclave_id) else {
            continue;
        };
        if enclave.model_id != serving.model_id {
            continue;
        }
        served_price_by_model
            .entry(enclave.model_id.clone())
            .and_modify(|existing| {
                if price_sort_key(serving_price) < price_sort_key(existing) {
                    *existing = serving_price.clone();
                }
            })
            .or_insert_with(|| serving_price.clone());
    }

    let mut providers_by_model: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut tiers_by_model: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    let mut route_candidates_by_model: BTreeMap<String, BTreeMap<String, GatewayRouteCandidate>> =
        BTreeMap::new();
    for serving in contract
        .roomserve
        .iter()
        .filter(|serving| serving.status == "active")
    {
        if !active_providers.contains(serving.provider.as_str()) {
            continue;
        }
        if !room_ids.contains(&serving.room_id) {
            continue;
        }
        let Some(enclave) = active_enclaves.get(&serving.enclave_id) else {
            continue;
        };
        let Some(selected_price) = served_price_by_model.get(&enclave.model_id) else {
            continue;
        };
        let Some(serving_price) = prices_by_enclave.get(&serving.enclave_id) else {
            continue;
        };
        if enclave.model_id != serving.model_id
            || !same_gateway_price_terms(serving_price, selected_price)
        {
            continue;
        }
        providers_by_model
            .entry(enclave.model_id.clone())
            .or_default()
            .insert(serving.provider.clone());
        route_candidates_by_model
            .entry(enclave.model_id.clone())
            .or_default()
            .insert(
                format!(
                    "{}:{}:{}",
                    serving.provider, serving.room_id, serving.enclave_id
                ),
                GatewayRouteCandidate {
                    provider: serving.provider.clone(),
                    enclave_id: serving.enclave_id.clone(),
                    room_id: serving.room_id.clone(),
                    price_ver: serving_price.ver,
                    att_tier: enclave.att_tier,
                },
            );
        let tier = format!("T{}", enclave.att_tier);
        tiers_by_model
            .entry(enclave.model_id.clone())
            .or_default()
            .entry(tier)
            .or_default()
            .insert(serving.provider.clone());
    }

    let mut models = Vec::new();
    for (model_id, price) in served_price_by_model {
        let rooms = rooms_by_model.get(&model_id).copied().unwrap_or(0);
        let route_candidates = route_candidates_by_model
            .remove(&model_id)
            .unwrap_or_default()
            .into_values()
            .collect::<Vec<_>>();
        let providers_online = providers_by_model
            .get(&model_id)
            .map(|providers| usize_to_u32(providers.len()))
            .unwrap_or(0);
        if rooms == 0 || providers_online == 0 {
            continue;
        }
        models.push(GatewayModel {
            id: model_id.clone(),
            created: 1_782_950_400,
            owned_by: "mayhem".to_owned(),
            mayhem: MayhemModelInfo {
                providers_online,
                rooms,
                price_ref_mu: PriceRefMu {
                    denom: "mu_usd".to_owned(),
                    ver: price.ver,
                    in_per_1k: price.in_per_1k_mu,
                    out_per_1k: price.out_per_1k_mu,
                },
                attestation_tiers: tiers_by_model
                    .remove(&model_id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(tier, providers)| (tier, usize_to_u32(providers.len())))
                    .collect(),
                caps: caps_by_model
                    .remove(&model_id)
                    .unwrap_or_else(empty_gateway_caps),
                source: "contract".to_owned(),
                route_candidates,
            },
        });
    }
    if models.is_empty() {
        bail!(
            "no canonical contract-backed models found; ask the admin to register an enclave, set a current mu_usd price, open a room, and confirm an active provider joined it, or use --dev-embedded-catalog for local smoke only"
        );
    }
    Ok(models)
}

fn price_sort_key(price: &LedgerPriceRecord) -> (u64, u64, u64, u64) {
    (
        price.in_per_1k_mu.saturating_add(price.out_per_1k_mu),
        price.in_per_1k_mu,
        price.out_per_1k_mu,
        price.ver,
    )
}

fn same_gateway_price_terms(left: &LedgerPriceRecord, right: &LedgerPriceRecord) -> bool {
    left.ver == right.ver
        && left.denom == right.denom
        && left.in_per_1k_mu == right.in_per_1k_mu
        && left.out_per_1k_mu == right.out_per_1k_mu
}

fn empty_gateway_caps() -> ModelCaps {
    ModelCaps {
        tools: false,
        json: false,
        ctx: 0,
        vision: false,
    }
}

fn gateway_caps_from_contract(caps: &Value) -> ModelCaps {
    ModelCaps {
        tools: caps.get("tools").and_then(Value::as_bool).unwrap_or(false),
        json: caps.get("json").and_then(Value::as_bool).unwrap_or(false),
        ctx: caps
            .get("ctx")
            .or_else(|| caps.get("ctx_max"))
            .and_then(Value::as_u64)
            .and_then(|ctx| u32::try_from(ctx).ok())
            .unwrap_or(0),
        vision: caps.get("vision").and_then(Value::as_bool).unwrap_or(false),
    }
}

fn merge_model_caps(target: &mut ModelCaps, next: &ModelCaps) {
    target.tools |= next.tools;
    target.json |= next.json;
    target.vision |= next.vision;
    target.ctx = target.ctx.max(next.ctx);
}

fn usize_to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn build_provider_candidates(
    contract: &ContractCatalog,
    catalog_doc: &catalog::CatalogDocument,
    hardware: &HardwareReport,
    args: &ProviderStartArgs,
) -> Result<Vec<ProviderCandidate>> {
    let requested_backend = requested_backend(args, hardware)?;
    let mut candidates = Vec::new();
    for enclave in contract
        .enclaves
        .iter()
        .filter(|enclave| enclave.status == "active")
    {
        if let Some(requested) = &requested_backend {
            if &enclave.backend != requested {
                continue;
            }
        }
        let Some(model) = catalog_doc
            .models
            .iter()
            .find(|model| model.model_id == enclave.model_id)
        else {
            continue;
        };
        let Some(verdict) = hardware
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == enclave.backend)
            .filter(|verdict| verdict.status != VerdictStatus::Insufficient)
        else {
            continue;
        };
        let Some((artifact_name, artifact)) =
            select_catalog_artifact(model, &enclave.backend, hardware)
        else {
            continue;
        };
        if artifact.artifact_root != enclave.artifact_root {
            continue;
        }
        let price = contract
            .prices
            .iter()
            .find(|price| price.enclave_id == enclave.enclave_id)
            .cloned();
        candidates.push(ProviderCandidate {
            enclave: enclave.clone(),
            model: model.clone(),
            artifact_name,
            artifact,
            verdict: verdict.clone(),
            price,
        });
    }
    if candidates.is_empty() {
        bail!(
            "no feasible active admin-created enclaves found in contract state; providers can only join enclaves the admin already registered"
        );
    }
    Ok(candidates)
}

fn requested_backend(
    args: &ProviderStartArgs,
    hardware: &HardwareReport,
) -> Result<Option<String>> {
    let requested = args.engine_backend.trim();
    if requested.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    let valid = hardware
        .backend_verdicts
        .iter()
        .any(|verdict| verdict.backend == requested);
    if !valid {
        bail!("unknown backend {requested}; expected auto, trt-llm, mlx, or llama.cpp");
    }
    Ok(Some(requested.to_owned()))
}

fn select_catalog_artifact(
    model: &catalog::CatalogModel,
    backend: &str,
    hardware: &HardwareReport,
) -> Option<(String, catalog::CatalogArtifact)> {
    let mut artifacts = model
        .artifacts
        .iter()
        .filter(|(_, artifact)| artifact.engine == backend)
        .collect::<Vec<_>>();
    if backend == "trt-llm" {
        let supports_nvfp4 = hardware.gpus.iter().any(|gpu| gpu.supports_nvfp4);
        let preferred = if supports_nvfp4 { "nvfp4" } else { "trt-fp8" };
        if let Some((name, artifact)) = artifacts
            .iter()
            .find(|(name, _)| name.as_str() == preferred)
        {
            return Some(((*name).clone(), (*artifact).clone()));
        }
    }
    artifacts
        .pop()
        .map(|(name, artifact)| (name.clone(), artifact.clone()))
}

fn select_provider_candidate(
    candidates: &[ProviderCandidate],
    requested: Option<&str>,
) -> Result<ProviderCandidate> {
    if let Some(requested) = requested {
        let by_id = is_32_byte_hex(requested);
        return candidates
            .iter()
            .find(|candidate| {
                if by_id {
                    candidate.enclave.enclave_id == requested
                } else {
                    candidate.enclave.model_id == requested
                }
            })
            .cloned()
            .with_context(|| {
                format!(
                    "requested enclave {requested} is not an active feasible admin-created enclave"
                )
            });
    }

    candidates
        .iter()
        .max_by_key(|candidate| backend_rank(&candidate.enclave.backend))
        .cloned()
        .context("no provider candidate available")
}

fn backend_rank(backend: &str) -> u8 {
    match backend {
        "trt-llm" => 3,
        "mlx" => 2,
        "llama.cpp" => 1,
        _ => 0,
    }
}

fn select_provider_rooms(
    rooms: &[LedgerRoom],
    enclave: &LedgerEnclave,
    requested: &str,
) -> Result<Vec<LedgerRoom>> {
    if requested.trim().eq_ignore_ascii_case("auto") {
        let mut selected = rooms
            .iter()
            .filter(|room| room.status == "open" && room.model_id == enclave.model_id)
            .cloned()
            .collect::<Vec<_>>();
        selected.sort_by(|a, b| a.room_id.cmp(&b.room_id));
        return Ok(selected);
    }

    let requested_ids = requested
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if requested_ids.is_empty() {
        bail!("--rooms must be auto or a comma-separated list of room ids");
    }
    let mut selected = Vec::new();
    for room_id in requested_ids {
        let room = rooms
            .iter()
            .find(|room| room.room_id == room_id)
            .with_context(|| format!("room {room_id} is not in contract state"))?;
        if room.status != "open" {
            bail!("room {room_id} is not open");
        }
        if room.model_id != enclave.model_id {
            bail!(
                "room {room_id} is for model {}, not {}",
                room.model_id,
                enclave.model_id
            );
        }
        selected.push(room.clone());
    }
    Ok(selected)
}

async fn derive_provider_secret(
    keypair_path: &Path,
    password: &str,
    wallet: &WalletInfo,
) -> Result<Vec<u8>> {
    let sig = sign_message(
        keypair_path,
        password,
        &format!("mayhem-provider-sealing-v1:{}", wallet.public_key),
    )
    .await?;
    Ok(blake3::hash(sig.as_bytes()).as_bytes().to_vec())
}

async fn download_provider_artifact(
    args: &ProviderStartArgs,
    downloads_dir: &Path,
    selected: &ProviderCandidate,
) -> Result<PathBuf> {
    let artifact_file = format!(
        "{}-{}",
        safe_path_component(&selected.enclave.enclave_id),
        safe_path_component(&selected.artifact_name)
    );
    let destination = downloads_dir.join(artifact_file);
    if destination.exists() {
        let merkle = build_merkle_manifest(&destination, args.chunk_size)?;
        if merkle.root == selected.enclave.artifact_root {
            return Ok(destination);
        }
    }

    let source = if let Some(path) = &args.artifact {
        DownloadSource::File(absolutize(path.clone())?)
    } else if selected.artifact.source.kind == "huggingface" {
        DownloadSource::Http {
            url: format!(
                "https://huggingface.co/{}/resolve/{}/{}",
                selected.artifact.source.repo,
                selected.artifact.source.revision,
                selected.artifact.path
            ),
            bearer_token: read_optional_token(args.hf_token_file.as_deref())?,
        }
    } else {
        bail!(
            "unsupported artifact source kind {}; pass --artifact with a local copy that matches the admin enclave artifact_root",
            selected.artifact.source.kind
        );
    };

    let mut request = DownloadRequest::new(source, destination.clone());
    request.chunk_size = args.chunk_size;
    request.expected_merkle_root = Some(selected.enclave.artifact_root.clone());
    download_resumable(&request).with_context(|| {
        format!(
            "downloading and verifying artifact root {}",
            selected.enclave.artifact_root
        )
    })?;
    Ok(destination)
}

fn read_optional_token(path: Option<&Path>) -> Result<Option<String>> {
    if let Some(path) = path {
        return fs::read_to_string(path)
            .with_context(|| format!("reading token file {}", path.display()))
            .map(|value| Some(value.trim().to_owned()));
    }
    Ok(env::var("HF_TOKEN")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty()))
}

fn seal_provider_artifact(
    artifact_path: &Path,
    sealed_store: &Path,
    key_context: &KeyContext,
    provider_secret: &[u8],
    chunk_size: usize,
) -> Result<DownloadReport> {
    if sealed_store.join(SEALED_STORE_MANIFEST).exists() {
        let merkle = build_merkle_manifest(artifact_path, chunk_size)?;
        return Ok(DownloadReport {
            destination: artifact_path.to_path_buf(),
            resumed_from: 0,
            bytes_written: 0,
            total_bytes: merkle.total_bytes,
            merkle,
        });
    }

    let mut options = SealOptions::new(
        artifact_path,
        sealed_store,
        key_context.clone(),
        provider_secret.to_vec(),
    );
    options.chunk_size = chunk_size;
    options.expected_merkle_root = Some(key_context.artifact_root.clone());
    let report = seal_artifact(&options)?;
    Ok(DownloadReport {
        destination: report.store_dir,
        resumed_from: 0,
        bytes_written: report.total_bytes,
        total_bytes: report.total_bytes,
        merkle: build_merkle_manifest(artifact_path, chunk_size)?,
    })
}

async fn ensure_provider_registered(
    rpc: &PeerRpcClient,
    keypair_path: &Path,
    password: &str,
    wallet: &WalletInfo,
    sim: bool,
) -> Result<Value> {
    let key = format!("prov/{}", wallet.public_key);
    if let Some(existing) = read_state_value(rpc, &key).await? {
        let status = existing.get("status").and_then(Value::as_str).unwrap_or("");
        if status == "active" {
            return Ok(
                json!({ "skipped": true, "reason": "already_registered", "state": existing }),
            );
        }
        bail!("provider registration exists but is not active: {existing}");
    }

    let submitted = submit_contract_command(
        rpc,
        keypair_path,
        password,
        wallet,
        "registerProvider",
        json!({ "op": "register_provider" }),
        sim,
    )
    .await?;
    if sim {
        return Ok(submitted);
    }
    let state = wait_for_state(rpc, &key, |value| {
        value.get("status").and_then(Value::as_str) == Some("active")
    })
    .await?;
    Ok(json!({ "skipped": false, "tx": submitted, "state": state }))
}

async fn ensure_joined_enclave(
    rpc: &PeerRpcClient,
    keypair_path: &Path,
    password: &str,
    wallet: &WalletInfo,
    enclave: &LedgerEnclave,
    sim: bool,
) -> Result<Value> {
    let key = format!("serve/{}/{}", wallet.public_key, enclave.enclave_id);
    if let Some(existing) = read_state_value(rpc, &key).await? {
        if existing.get("status").and_then(Value::as_str) == Some("active") {
            return Ok(
                json!({ "skipped": true, "reason": "already_joined_enclave", "state": existing }),
            );
        }
    }
    let submitted = submit_contract_command(
        rpc,
        keypair_path,
        password,
        wallet,
        "joinEnclave",
        json!({
            "op": "join_enclave",
            "enclave_id": enclave.enclave_id,
        }),
        sim,
    )
    .await?;
    if sim {
        return Ok(submitted);
    }
    let state = wait_for_state(rpc, &key, |value| {
        value.get("status").and_then(Value::as_str) == Some("active")
    })
    .await?;
    Ok(json!({ "skipped": false, "tx": submitted, "state": state }))
}

async fn ensure_joined_rooms(
    rpc: &PeerRpcClient,
    keypair_path: &Path,
    password: &str,
    wallet: &WalletInfo,
    enclave: &LedgerEnclave,
    rooms: &[LedgerRoom],
    sim: bool,
) -> Result<Vec<Value>> {
    let mut reports = Vec::new();
    for room in rooms {
        let key = format!(
            "roomserve/{}/{}/{}",
            room.room_id, wallet.public_key, enclave.enclave_id
        );
        if let Some(existing) = read_state_value(rpc, &key).await? {
            if existing.get("status").and_then(Value::as_str) == Some("active") {
                reports.push(json!({
                    "room_id": room.room_id,
                    "skipped": true,
                    "reason": "already_joined_room",
                    "state": existing,
                }));
                continue;
            }
        }
        let submitted = submit_contract_command(
            rpc,
            keypair_path,
            password,
            wallet,
            "joinRoom",
            json!({
                "op": "join_room",
                "room_id": room.room_id,
                "enclave_id": enclave.enclave_id,
            }),
            sim,
        )
        .await?;
        if sim {
            reports.push(json!({ "room_id": room.room_id, "tx": submitted }));
            continue;
        }
        let state = wait_for_state(rpc, &key, |value| {
            value.get("status").and_then(Value::as_str) == Some("active")
        })
        .await?;
        reports.push(
            json!({ "room_id": room.room_id, "skipped": false, "tx": submitted, "state": state }),
        );
    }
    Ok(reports)
}

async fn submit_contract_command(
    rpc: &PeerRpcClient,
    keypair_path: &Path,
    password: &str,
    wallet: &WalletInfo,
    tx_type: &str,
    value: Value,
    sim: bool,
) -> Result<Value> {
    let prepared_command = json!({
        "type": tx_type,
        "value": value,
    });
    let nonce_response = rpc
        .contract_nonce()
        .await
        .context("requesting contract nonce")?;
    let nonce = nonce_response
        .get("nonce")
        .and_then(Value::as_str)
        .context("RPC nonce response did not include nonce")?;
    let prepared = rpc
        .prepare_tx(json!({
            "prepared_command": prepared_command.clone(),
            "address": wallet.public_key,
            "nonce": nonce,
        }))
        .await
        .with_context(|| format!("preparing {tx_type} tx"))?;
    let tx = prepared
        .get("tx")
        .and_then(Value::as_str)
        .context("RPC prepare response did not include tx")?
        .to_owned();
    let command_hash = prepared
        .get("command_hash")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let signature = sign_hex(keypair_path, password, &tx).await?;
    let submitted = rpc
        .submit_tx(json!({
            "tx": tx,
            "prepared_command": prepared_command.clone(),
            "address": wallet.public_key,
            "signature": signature,
            "nonce": nonce,
            "sim": sim,
        }))
        .await
        .with_context(|| format!("submitting {tx_type} tx"))?;
    let result = submitted
        .get("result")
        .cloned()
        .unwrap_or_else(|| submitted.clone());
    let accepted = result.get("ok").and_then(Value::as_bool) == Some(true)
        || (result.get("local").and_then(Value::as_bool) == Some(true)
            && result.get("txo").is_some());
    if !accepted {
        bail!("contract {tx_type} rejected provider command: {result}");
    }
    Ok(json!({
        "tx": tx,
        "command_hash": command_hash,
        "result": result,
    }))
}

async fn read_state_value(rpc: &PeerRpcClient, key: &str) -> Result<Option<Value>> {
    let state = rpc
        .state(Some(key), Some(false))
        .await
        .with_context(|| format!("reading {key} from peer RPC"))?;
    Ok(state.get("value").cloned().filter(|value| !value.is_null()))
}

async fn wait_for_state<F>(rpc: &PeerRpcClient, key: &str, predicate: F) -> Result<Value>
where
    F: Fn(&Value) -> bool,
{
    let mut last = Value::Null;
    for _ in 0..120 {
        last = read_state_value(rpc, key).await?.unwrap_or(Value::Null);
        if predicate(&last) {
            return Ok(last);
        }
        sleep(Duration::from_millis(500)).await;
    }
    bail!("timed out waiting for {key}; last value: {last}");
}

async fn emit_provider_heartbeats(ctx: HeartbeatContext<'_>) -> Result<Vec<Value>> {
    let sc_bridge_url = ctx.args.sc_bridge_url.clone().or_else(|| {
        ctx.config
            .as_ref()
            .and_then(|config| config.network.as_ref())
            .and_then(|network| network.sc_bridge_url.clone())
    });
    let sc_bridge_token = ctx.args.sc_bridge_token.clone().or_else(|| {
        ctx.config
            .as_ref()
            .and_then(|config| config.network.as_ref())
            .and_then(|network| network.sc_bridge_token.clone())
    });
    let (Some(url), Some(token)) = (sc_bridge_url, sc_bridge_token) else {
        provider_log(
            ctx.args,
            "SC-Bridge URL/token not configured; skipping live heartbeat send",
        );
        return Ok(Vec::new());
    };
    let mut bridge = ScBridgeClient::connect(ScBridgeConfig::new(url, token)?)
        .await
        .context("connecting to SC-Bridge for provider heartbeats")?;
    let mut sent = Vec::new();
    let count = u64::from(ctx.args.heartbeat_count.max(1));
    for seq in 0..count {
        sent.extend(send_provider_heartbeat_round(&mut bridge, &ctx, seq, seq == 0).await?);
    }
    Ok(sent)
}

async fn send_provider_heartbeat_round(
    bridge: &mut ScBridgeClient,
    ctx: &HeartbeatContext<'_>,
    seq: u64,
    join_rooms: bool,
) -> Result<Vec<Value>> {
    let mut sent = Vec::new();
    for room in ctx.rooms {
        if join_rooms {
            bridge
                .join(&room.sidechannel)
                .await
                .with_context(|| format!("joining sidechannel {}", room.sidechannel))?;
        }
        let ts = unix_epoch_millis()?;
        let mut heartbeat = json!({
            "t": "hb",
            "v": 1,
            "provider": ctx.wallet.public_key,
            "enclave_id": ctx.selected.enclave.enclave_id,
            "model_id": ctx.selected.enclave.model_id,
            "room_id": room.room_id,
            "sat": 0.0,
            "slots": {
                "active": 0,
                "max": ctx.selected.verdict.max_sessions,
            },
            "q": {
                "depth": 0,
                "est_wait_ms": 0,
            },
            "perf": {
                "tok_s": ctx.selected.verdict.est_tok_s,
                "ttft_ms": 0,
            },
            "price_ver": ctx.selected
                .price
                .as_ref()
                .and_then(|price| price.current.as_ref())
                .map(|price| price.ver)
                .unwrap_or(0),
            "caps": {
                "tools": ctx.selected.model.caps.tools,
                "json": ctx.selected.model.caps.json,
                "ctx": ctx.selected.model.caps.ctx_max,
                "vision": ctx.selected.model.caps.vision,
            },
            "att": {
                "epoch": ctx.attestation.report.boot_epoch,
                "head": ctx.attestation_head,
            },
            "ts": ts,
            "nonce": blake3::hash(format!("{}:{}:{}:{}", room.room_id, ctx.wallet.public_key, ts, seq).as_bytes()).to_hex().to_string(),
        });
        let signing_payload = String::from_utf8(heartbeat_signing_payload(&heartbeat)?)
            .context("heartbeat signing payload was not UTF-8")?;
        let sig = sign_message(ctx.keypair_path, ctx.password, &signing_payload).await?;
        heartbeat["sig"] = json!(sig);
        bridge
            .send(&room.sidechannel, &heartbeat)
            .await
            .with_context(|| format!("sending heartbeat to {}", room.sidechannel))?;
        sent.push(json!({
            "room_id": room.room_id,
            "sidechannel": room.sidechannel,
            "seq": seq,
        }));
    }
    Ok(sent)
}

async fn serve_provider_sessions(
    ctx: ProviderSessionContext<'_>,
    mut responder: Box<dyn ProviderSessionResponder>,
) -> Result<()> {
    let terms = provider_session_terms(&ctx)?;
    let (sc_bridge_url, sc_bridge_token) = resolve_cli_sc_bridge(
        ctx.args.home.as_ref(),
        ctx.args.sc_bridge_url.as_deref(),
        ctx.args.sc_bridge_token.as_deref(),
    )?;
    let mut bridge = ScBridgeClient::connect(ScBridgeConfig::new(&sc_bridge_url, sc_bridge_token)?)
        .await
        .context("connecting to SC-Bridge for provider session serving")?;
    let subscription = bridge
        .session_subscribe_all()
        .await
        .context("subscribing to all direct session frames")?;
    if !subscription
        .get("all_sessions")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        bail!("SC-Bridge does not support all-session subscription; update the local Intercom app");
    }

    provider_log(
        ctx.args,
        &format!(
            "Provider session server listening on {} for enclave {} across {} canonical room(s) with {}",
            sc_bridge_url,
            terms.enclave_id,
            ctx.rooms.len(),
            responder.mode()
        ),
    );

    let no_config = None;
    let heartbeat_ctx = HeartbeatContext {
        args: ctx.args,
        config: &no_config,
        keypair_path: ctx.keypair_path,
        password: ctx.password,
        wallet: ctx.wallet,
        selected: ctx.selected,
        rooms: ctx.rooms,
        attestation: ctx.attestation,
        attestation_head: ctx.attestation_head,
    };
    let heartbeat_enabled = !ctx.args.no_heartbeat && !ctx.rooms.is_empty();
    let mut heartbeat_seq = 0_u64;
    let mut heartbeat_rooms_joined = false;
    let mut next_heartbeat_at = Instant::now();
    let deadline = (ctx.args.serve_sessions_seconds > 0)
        .then(|| Instant::now() + Duration::from_secs(ctx.args.serve_sessions_seconds));
    let mut sessions = HashMap::new();
    loop {
        let now = Instant::now();
        if heartbeat_enabled && now >= next_heartbeat_at {
            send_provider_heartbeat_round(
                &mut bridge,
                &heartbeat_ctx,
                heartbeat_seq,
                !heartbeat_rooms_joined,
            )
            .await?;
            heartbeat_rooms_joined = true;
            heartbeat_seq = heartbeat_seq.saturating_add(1);
            next_heartbeat_at = Instant::now() + Duration::from_secs(2);
        }
        let mut wait = deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .filter(|remaining| !remaining.is_zero())
            .unwrap_or_else(|| Duration::from_secs(1));
        if heartbeat_enabled {
            let heartbeat_wait = next_heartbeat_at.saturating_duration_since(Instant::now());
            if heartbeat_wait < wait {
                wait = heartbeat_wait;
            }
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break;
        }
        if wait.is_zero() {
            continue;
        }
        match bridge.next_session_frame(wait).await {
            Ok(event) => {
                handle_provider_session_frame(
                    &mut bridge,
                    &mut sessions,
                    &terms,
                    ctx.attestation,
                    responder.as_mut(),
                    event,
                )
                .await?;
            }
            Err(BridgeError::Timeout) => {
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    break;
                }
            }
            Err(err) => return Err(err).context("reading provider session frame"),
        }
    }
    Ok(())
}

async fn handle_provider_session_frame(
    bridge: &mut ScBridgeClient,
    sessions: &mut HashMap<String, ActiveProviderSession>,
    terms: &ProviderSessionTerms,
    attestation: &Tier1AttestationReport,
    responder: &mut dyn ProviderSessionResponder,
    event: Value,
) -> Result<()> {
    let frame = event.get("frame").cloned().unwrap_or(Value::Null);
    let session_id = event
        .get("session_id")
        .and_then(Value::as_str)
        .or_else(|| frame.get("session_id").and_then(Value::as_str))
        .unwrap_or("")
        .to_owned();
    let remote = event
        .get("remote")
        .and_then(Value::as_str)
        .filter(|remote| !remote.is_empty())
        .context("provider session frame missing remote peer")?
        .to_owned();
    match frame.get("t").and_then(Value::as_str) {
        Some("s.open") => match provider_session_open_decision(&frame, terms) {
            ProviderSessionDecision::Accept => {
                sessions.insert(
                    session_id.clone(),
                    ActiveProviderSession {
                        user: remote.clone(),
                        session_id: session_id.clone(),
                    },
                );
                bridge
                    .session_send(
                        &remote,
                        &session_id,
                        json!({
                            "t": "s.accept",
                            "v": 1,
                            "session_id": session_id,
                            "att_report": attestation.report,
                            "engine": {
                                "ctx": terms.ctx,
                                "mode": "provider-session-server-v1",
                            },
                            "ts": unix_epoch_millis()?,
                            "nonce": stable_value_hash(&json!({
                                "session_id": frame.get("session_id"),
                                "provider": terms.provider,
                                "kind": "accept",
                            })),
                            "sig": attestation.report.sig_provider,
                        }),
                    )
                    .await
                    .context("sending s.accept")?;
            }
            ProviderSessionDecision::Reject { code, reason } => {
                bridge
                    .session_send(
                        &remote,
                        &session_id,
                        json!({
                            "t": "s.reject",
                            "v": 1,
                            "session_id": session_id,
                            "code": code,
                            "reason": reason,
                            "retry_after_ms": 0,
                            "alt_rooms": [],
                        }),
                    )
                    .await
                    .context("sending s.reject")?;
            }
        },
        Some("s.req") => {
            let Some(active) = sessions.get(&session_id).cloned() else {
                send_provider_session_close(bridge, &remote, &session_id, "err:unknown_session")
                    .await?;
                return Ok(());
            };
            let request_id = frame
                .get("rid")
                .and_then(Value::as_str)
                .filter(|rid| !rid.is_empty())
                .unwrap_or("missing-rid");
            let body = frame.get("body").cloned().unwrap_or(Value::Null);
            let output = responder.respond(terms, &body)?;
            send_provider_session_output(
                bridge,
                &active.user,
                &active.session_id,
                request_id,
                &output,
            )
            .await?;
            send_provider_session_close(bridge, &active.user, &active.session_id, "done").await?;
            sessions.remove(&session_id);
        }
        Some("s.close") => {
            sessions.remove(&session_id);
        }
        _ => {}
    }
    Ok(())
}

async fn send_provider_session_output(
    bridge: &mut ScBridgeClient,
    remote: &str,
    session_id: &str,
    request_id: &str,
    output: &ProviderSessionOutput,
) -> Result<()> {
    if let Some(tool) = &output.tool {
        bridge
            .session_send(
                remote,
                session_id,
                json!({
                    "t": "s.delta",
                    "rid": request_id,
                    "i": 0,
                    "d": "",
                    "tool": tool,
                    "fin": output.finish_reason,
                    "usage": { "in": output.prompt_tokens, "out": output.completion_tokens },
                }),
            )
            .await
            .context("sending tool-call s.delta")?;
        return Ok(());
    }

    let mut index = 0_u64;
    for part in provider_stream_parts(&output.content) {
        bridge
            .session_send(
                remote,
                session_id,
                json!({
                    "t": "s.delta",
                    "rid": request_id,
                    "i": index,
                    "d": part,
                    "tool": null,
                    "fin": null,
                }),
            )
            .await
            .context("sending content s.delta")?;
        index = index.saturating_add(1);
    }
    bridge
        .session_send(
            remote,
            session_id,
            json!({
                "t": "s.delta",
                "rid": request_id,
                "i": index,
                "d": "",
                "tool": null,
                "fin": output.finish_reason,
                "usage": { "in": output.prompt_tokens, "out": output.completion_tokens },
            }),
        )
        .await
        .context("sending final s.delta")?;
    Ok(())
}

async fn send_provider_session_close(
    bridge: &mut ScBridgeClient,
    remote: &str,
    session_id: &str,
    reason: &str,
) -> Result<()> {
    bridge
        .session_send(
            remote,
            session_id,
            json!({
                "t": "s.close",
                "v": 1,
                "session_id": session_id,
                "reason": reason,
            }),
        )
        .await
        .with_context(|| format!("sending s.close for {session_id}"))?;
    let _ = bridge.session_close(remote, session_id).await;
    Ok(())
}

fn provider_session_responder(
    ctx: &ProviderSessionContext<'_>,
) -> Result<Box<dyn ProviderSessionResponder>> {
    if ctx.args.dev_session_shim {
        if !ctx.args.dev_skip_catalog_verify {
            bail!("--dev-session-shim requires --dev-skip-catalog-verify and is never canonical");
        }
        return Ok(Box::new(DeterministicProviderSessionResponder));
    }

    provider_log(
        ctx.args,
        &format!(
            "Loading {} session engine from verified admin artifact {}",
            ctx.selected.artifact.engine,
            ctx.artifact_path.display()
        ),
    );
    let load_config = provider_engine_load_config(ctx.selected, ctx.artifact_path)?;
    match ctx.selected.artifact.engine.as_str() {
        "llama.cpp" => {
            let mut backend = mayhem_engine::LlamaCppBackend::new()
                .context("initializing llama.cpp provider session engine")?;
            backend
                .load(load_config)
                .context("loading llama.cpp provider session engine")?;
            Ok(Box::new(EngineProviderSessionResponder {
                backend: Box::new(backend),
            }))
        }
        "mlx" => {
            let mut backend = mayhem_engine::MlxBackend::new()
                .context("initializing MLX provider session engine")?;
            backend
                .load(load_config)
                .context("loading MLX provider session engine")?;
            Ok(Box::new(EngineProviderSessionResponder {
                backend: Box::new(backend),
            }))
        }
        other => bail!(
            "provider session engine for {other} is not wired locally yet; do not serve this enclave until its admin-approved engine adapter is available"
        ),
    }
}

fn provider_engine_load_config(
    selected: &ProviderCandidate,
    artifact_path: &Path,
) -> Result<LoadConfig> {
    let ctx_size = u32::try_from(selected.model.caps.ctx_max).with_context(|| {
        format!(
            "catalog ctx_max {} for {} exceeds engine ctx_size range",
            selected.model.caps.ctx_max, selected.model.model_id
        )
    })?;
    let artifact = match selected.artifact.engine.as_str() {
        "llama.cpp" => ModelArtifact::gguf(artifact_path),
        "mlx" => ModelArtifact::mlx_safetensors(artifact_path),
        other => bail!("unsupported local provider session engine {other}"),
    };
    let artifact = if let Some(sha256) = &selected.artifact.source_sha256 {
        artifact.with_sha256(sha256.clone())
    } else {
        artifact
    };
    let mut config = match selected.artifact.engine.as_str() {
        "llama.cpp" => LoadConfig::gguf(artifact_path),
        "mlx" => LoadConfig::mlx_safetensors(artifact_path),
        other => bail!("unsupported local provider session engine {other}"),
    };
    config.artifact = artifact;
    config.ctx_size = ctx_size.max(1);
    config.gpu_layers = selected.verdict.n_layers_gpu;
    Ok(config)
}

fn provider_session_terms(ctx: &ProviderSessionContext<'_>) -> Result<ProviderSessionTerms> {
    let price = ctx
        .selected
        .price
        .as_ref()
        .and_then(|schedule| schedule.current.as_ref())
        .context("selected provider enclave has no current admin price")?;
    Ok(ProviderSessionTerms {
        provider: ctx.wallet.public_key.clone(),
        enclave_id: ctx.selected.enclave.enclave_id.clone(),
        model_id: ctx.selected.enclave.model_id.clone(),
        price_ver: price.ver,
        rules_ver: ctx.rules.ver,
        ctx: ctx.selected.model.caps.ctx_max,
    })
}

fn provider_session_open_decision(
    frame: &Value,
    terms: &ProviderSessionTerms,
) -> ProviderSessionDecision {
    let reject = |code, reason: String| ProviderSessionDecision::Reject { code, reason };
    if frame.get("t").and_then(Value::as_str) != Some("s.open") {
        return reject("SCHEMA", "session open frame must have t=s.open".to_owned());
    }
    let session_id = frame
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !is_hex_len(session_id, 64) {
        return reject("SCHEMA", "session_id must be 32 bytes of hex".to_owned());
    }
    if frame.get("enclave_id").and_then(Value::as_str) != Some(terms.enclave_id.as_str()) {
        return reject(
            "ENCLAVE",
            "session enclave_id is not this admin-created enclave".to_owned(),
        );
    }
    if frame.get("price_ver").and_then(Value::as_u64) != Some(terms.price_ver) {
        return reject(
            "PRICE_VER",
            "session price_ver does not match the current admin price".to_owned(),
        );
    }
    if frame.get("rules_ver").and_then(Value::as_u64) != Some(terms.rules_ver) {
        return reject(
            "CONSENT",
            "session rules_ver does not match current rules".to_owned(),
        );
    }
    let voucher = frame.get("voucher").unwrap_or(&Value::Null);
    if voucher.get("session_id").and_then(Value::as_str) != Some(session_id) {
        return reject("VOUCHER", "voucher session_id mismatch".to_owned());
    }
    if voucher.get("enclave_id").and_then(Value::as_str) != Some(terms.enclave_id.as_str()) {
        return reject("VOUCHER", "voucher enclave_id mismatch".to_owned());
    }
    if voucher.get("price_ver").and_then(Value::as_u64) != Some(terms.price_ver) {
        return reject("VOUCHER", "voucher price_ver mismatch".to_owned());
    }
    if voucher
        .get("max_spend_mu")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        == 0
    {
        return reject(
            "BALANCE",
            "voucher max_spend_mu must be positive".to_owned(),
        );
    }
    ProviderSessionDecision::Accept
}

#[derive(Clone, Debug, PartialEq)]
struct ProviderSessionOutput {
    content: String,
    tool: Option<Value>,
    finish_reason: String,
    prompt_tokens: u64,
    completion_tokens: u64,
}

fn provider_engine_session_response(
    backend: &mut dyn EngineBackend,
    body: &Value,
) -> Result<ProviderSessionOutput> {
    let request = provider_engine_request_from_body(body)?;
    let wants_tool = request
        .grammar
        .as_ref()
        .is_some_and(|grammar| matches!(grammar, GrammarSpec::ToolCall { .. }));
    let mut sink = mayhem_engine::NoopTokenSink;
    let output = backend
        .generate(request, &mut sink)
        .context("generating provider session response with mayhem-engine")?;
    let tool = if wants_tool {
        Some(
            provider_engine_tool_call_output(&output.text).with_context(|| {
                format!(
                    "provider engine did not return valid tool-call JSON: {}",
                    output.text.trim()
                )
            })?,
        )
    } else {
        None
    };
    Ok(ProviderSessionOutput {
        content: if tool.is_some() {
            String::new()
        } else {
            output.text
        },
        tool,
        finish_reason: if wants_tool {
            "tool_calls".to_owned()
        } else {
            output.finish_reason.to_string()
        },
        prompt_tokens: u64::from(output.usage.prompt_tokens),
        completion_tokens: u64::from(output.usage.completion_tokens),
    })
}

fn provider_engine_request_from_body(body: &Value) -> Result<GenerateRequest> {
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let prompt = provider_engine_prompt(messages);
    let mut request = GenerateRequest::new(prompt);
    if let Some(max_tokens) = body
        .get("max_tokens")
        .or_else(|| body.get("max_completion_tokens"))
        .and_then(Value::as_u64)
    {
        request.max_new_tokens = u32::try_from(max_tokens)
            .context("max_tokens exceeds provider engine request range")?;
    }
    if let Some(temperature) = body.get("temperature").and_then(Value::as_f64) {
        request.temperature = Some(temperature as f32);
    }
    if let Some(top_p) = body.get("top_p").and_then(Value::as_f64) {
        request.top_p = Some(top_p as f32);
    }
    if let Some(seed) = body.get("seed").and_then(Value::as_u64) {
        request.seed = Some(u32::try_from(seed).context("seed exceeds u32")?);
    }
    if body.get("tool_choice").and_then(Value::as_str) != Some("none") {
        let tools = provider_engine_tool_specs(body)?;
        if !tools.is_empty() {
            request.grammar = Some(GrammarSpec::ToolCall { tools });
            return Ok(request);
        }
    }
    if provider_wants_json(body) {
        request.grammar = Some(GrammarSpec::JsonSchema {
            schema: provider_response_json_schema(body),
        });
    }
    Ok(request)
}

fn provider_engine_prompt(messages: &[Value]) -> String {
    let mut prompt = String::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let content = provider_message_to_text(message);
        let _ = writeln!(prompt, "{role}: {content}");
    }
    prompt.push_str("assistant:");
    prompt
}

fn provider_engine_tool_specs(body: &Value) -> Result<Vec<ToolSpec>> {
    let chosen = body
        .get("tool_choice")
        .and_then(|choice| choice.get("function"))
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str);
    let tools = body
        .get("tools")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut specs = Vec::new();
    for tool in tools {
        let Some(function) = tool.get("function") else {
            continue;
        };
        let Some(name) = function.get("name").and_then(Value::as_str) else {
            continue;
        };
        if chosen.is_some_and(|chosen| chosen != name) {
            continue;
        }
        let parameters = function
            .get("parameters")
            .cloned()
            .unwrap_or_else(|| json!({ "type": "object", "additionalProperties": true }));
        let mut spec = ToolSpec::new(name, parameters);
        spec.description = function
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned);
        specs.push(spec);
    }
    Ok(specs)
}

fn provider_response_json_schema(body: &Value) -> Value {
    body.get("response_format")
        .and_then(|format| format.get("json_schema"))
        .and_then(|json_schema| json_schema.get("schema"))
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object", "additionalProperties": true }))
}

fn provider_engine_tool_call_output(text: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    let name = value
        .get("tool")
        .or_else(|| value.get("name"))
        .and_then(Value::as_str)?
        .to_owned();
    let arguments = value
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}))
        .to_string();
    Some(json!({
        "id": format!("call-{}", stable_value_hash(&json!({ "tool": name, "arguments": arguments }))),
        "name": name,
        "arguments": arguments,
    }))
}

fn provider_session_response(terms: &ProviderSessionTerms, body: &Value) -> ProviderSessionOutput {
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let prompt = messages
        .iter()
        .map(provider_message_to_text)
        .collect::<Vec<_>>()
        .join("\n");
    let prompt_tokens = rough_text_tokens(&prompt);
    if let Some(tool_result) = messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("tool"))
        .map(provider_message_to_text)
    {
        let content = format!("Tool result received: {tool_result}");
        return ProviderSessionOutput {
            completion_tokens: rough_text_tokens(&content),
            content,
            tool: None,
            finish_reason: "stop".to_owned(),
            prompt_tokens,
        };
    }
    if let Some(tool_name) = provider_requested_tool_name(body) {
        return ProviderSessionOutput {
            content: String::new(),
            tool: Some(json!({
                "id": format!("call-{}", stable_value_hash(&json!({ "tool": tool_name, "prompt": prompt }))),
                "name": tool_name,
                "arguments": provider_tool_arguments(&tool_name),
            })),
            finish_reason: "tool_calls".to_owned(),
            prompt_tokens,
            completion_tokens: 1,
        };
    }
    let content = if provider_wants_json(body) {
        json!({
            "ok": true,
            "model": &terms.model_id,
            "provider": &terms.provider,
        })
        .to_string()
    } else {
        format!(
            "Mayhem provider response from {}: {}",
            terms.model_id,
            provider_last_user_text(messages)
        )
    };
    ProviderSessionOutput {
        completion_tokens: rough_text_tokens(&content),
        content,
        tool: None,
        finish_reason: "stop".to_owned(),
        prompt_tokens,
    }
}

fn provider_requested_tool_name(body: &Value) -> Option<String> {
    if body.get("tool_choice").and_then(Value::as_str) == Some("none") {
        return None;
    }
    if let Some(name) = body
        .get("tool_choice")
        .and_then(|choice| choice.get("function"))
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
    {
        return Some(name.to_owned());
    }
    body.get("tools")?.as_array()?.iter().find_map(|tool| {
        tool.get("function")?
            .get("name")?
            .as_str()
            .map(str::to_owned)
    })
}

fn provider_tool_arguments(name: &str) -> String {
    match name {
        "bash" => json!({ "command": "printf mayhem-opencode-tool-ok" }).to_string(),
        "write" => {
            json!({ "filePath": "mayhem-opencode-tool-ok.txt", "content": "mayhem-opencode-tool-ok" })
                .to_string()
        }
        _ => "{}".to_owned(),
    }
}

fn provider_wants_json(body: &Value) -> bool {
    matches!(
        body.get("response_format")
            .and_then(|format| format.get("type"))
            .and_then(Value::as_str),
        Some("json_object" | "json_schema")
    )
}

fn provider_last_user_text(messages: &[Value]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .map(provider_message_to_text)
        .unwrap_or_default()
}

fn provider_message_to_text(message: &Value) -> String {
    provider_content_to_text(message.get("content").unwrap_or(&Value::Null))
}

fn provider_content_to_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| provider_content_to_text(part))
            })
            .collect::<Vec<_>>()
            .join(""),
        other => other.to_string(),
    }
}

fn provider_stream_parts(content: &str) -> Vec<String> {
    let mut parts = content
        .split_inclusive(' ')
        .map(str::to_owned)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() && !content.is_empty() {
        parts.push(content.to_owned());
    }
    parts
}

fn rough_text_tokens(text: &str) -> u64 {
    if text.trim().is_empty() {
        0
    } else {
        text.split_whitespace().count() as u64
    }
}

fn is_hex_len(value: &str, len: usize) -> bool {
    value.len() == len && value.as_bytes().iter().all(|byte| byte.is_ascii_hexdigit())
}

fn safe_path_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn is_32_byte_hex(value: &str) -> bool {
    value.len() == 64 && value.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

fn unix_epoch_seconds() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before Unix epoch")?
        .as_secs())
}

fn unix_epoch_millis() -> Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before Unix epoch")?
        .as_millis();
    u64::try_from(millis).context("Unix epoch milliseconds overflowed u64")
}

fn default_home() -> Result<PathBuf> {
    if let Ok(home) = env::var("MAYHEM_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return Ok(PathBuf::from(home));
        }
    }

    let user_home = env::var("HOME").context("HOME is not set; pass --home")?;
    Ok(PathBuf::from(user_home).join(".mayhem"))
}

fn user_home_dir() -> Result<PathBuf> {
    if let Ok(home) = env::var("HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return Ok(PathBuf::from(home));
        }
    }
    if let Ok(home) = env::var("USERPROFILE") {
        let home = home.trim();
        if !home.is_empty() {
            return Ok(PathBuf::from(home));
        }
    }
    bail!("HOME/USERPROFILE is not set; pass an explicit path")
}

fn repo_path(relative: &str) -> Result<PathBuf> {
    let path = absolutize(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(relative),
    )?;
    Ok(fs::canonicalize(&path).unwrap_or(path))
}

fn default_rules_path() -> Result<PathBuf> {
    let repo_rules = repo_path("RULES.md")?;
    if repo_rules.exists() {
        return absolutize(repo_rules);
    }
    absolutize(PathBuf::from("RULES.md"))
}

fn absolutize(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(env::current_dir()?.join(path))
}

fn read_mayhem_config(home: &Path) -> Result<Option<MayhemConfig>> {
    let config_path = home.join("config.toml");
    if !config_path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let config =
        toml::from_str(&text).with_context(|| format!("parsing {}", config_path.display()))?;
    Ok(Some(config))
}

fn read_rules_doc(path: Option<&Path>) -> Result<RulesDoc> {
    let path = match path {
        Some(path) => absolutize(path.to_path_buf())?,
        None => default_rules_path()?,
    };
    let path = fs::canonicalize(&path).unwrap_or(path);
    let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let text = String::from_utf8(bytes.clone())
        .with_context(|| format!("{} must be valid UTF-8", path.display()))?;
    let hash = blake3::hash(&bytes).to_hex().to_string();
    Ok(RulesDoc {
        path,
        text,
        hash,
        bytes: bytes.len(),
    })
}

fn rules_state_dir(home: &Path) -> PathBuf {
    home.join("rules")
}

fn read_current_accepted_rules(home: &Path) -> Result<Option<String>> {
    let path = rules_state_dir(home).join("current.md");
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    Ok(Some(text))
}

fn persist_rules_acceptance(home: &Path, rules_doc: &RulesDoc, rules: &RulesRef) -> Result<()> {
    let dir = rules_state_dir(home);
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let versioned_path = dir.join(format!("accepted-{}-{}.md", rules.ver, rules.hash));
    fs::write(&versioned_path, &rules_doc.text)
        .with_context(|| format!("writing {}", versioned_path.display()))?;
    fs::write(dir.join("current.md"), &rules_doc.text)
        .with_context(|| format!("writing {}", dir.join("current.md").display()))?;
    fs::write(
        dir.join("current.json"),
        serde_json::to_vec_pretty(&json!({
            "ver": rules.ver,
            "hash": rules.hash,
            "path": rules_doc.path,
            "bytes": rules_doc.bytes,
            "accepted_text": versioned_path,
        }))?,
    )
    .with_context(|| format!("writing {}", dir.join("current.json").display()))?;
    Ok(())
}

fn print_rules_review(
    home: &Path,
    rules_doc: &RulesDoc,
    rules: &RulesRef,
    previous_rules: Option<&str>,
    prior_consent: Option<&Value>,
) -> Result<()> {
    println!("Mayhem rules review");
    println!("Home: {}", home.display());
    println!("Rules file: {}", rules_doc.path.display());
    println!("Rules version: {}", rules.ver);
    println!("BLAKE3(RULES.md): {}", rules_doc.hash);
    if let Some(consent) = prior_consent {
        let ver = consent.get("ver").and_then(Value::as_u64).unwrap_or(0);
        let hash = consent.get("hash").and_then(Value::as_str).unwrap_or("");
        println!("Existing consent: v{ver} {hash}");
    } else {
        println!("Existing consent: none");
    }

    match previous_rules {
        Some(previous) if previous == rules_doc.text => {
            println!();
            println!("Local accepted rules text is unchanged.");
        }
        Some(previous) => {
            println!();
            println!("Diff from locally accepted rules:");
            println!("{}", render_line_diff(previous, &rules_doc.text));
        }
        None => {
            println!();
            println!("No locally accepted rules text was found. Current rules text follows.");
            println!();
            println!("{}", rules_doc.text);
        }
    }
    Ok(())
}

fn render_line_diff(old: &str, new: &str) -> String {
    let old_lines = old.lines().collect::<Vec<_>>();
    let new_lines = new.lines().collect::<Vec<_>>();
    let max_len = old_lines.len().max(new_lines.len());
    let mut out = String::new();
    for i in 0..max_len {
        match (old_lines.get(i), new_lines.get(i)) {
            (Some(old), Some(new)) if old == new => {
                let _ = writeln!(out, "  {old}");
            }
            (Some(old), Some(new)) => {
                let _ = writeln!(out, "- {old}");
                let _ = writeln!(out, "+ {new}");
            }
            (Some(old), None) => {
                let _ = writeln!(out, "- {old}");
            }
            (None, Some(new)) => {
                let _ = writeln!(out, "+ {new}");
            }
            (None, None) => {}
        }
    }
    out
}

fn confirm_rules_acceptance(yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        bail!("Pass --yes to accept and sign the current RULES.md in non-interactive mode.");
    }

    eprint!("Type 'agree' to sign consent for these rules: ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if answer.trim() != "agree" {
        bail!("Rules consent was not signed.");
    }
    Ok(())
}

fn select_role(role: Option<Role>) -> Result<Role> {
    if let Some(role) = role {
        return Ok(role);
    }

    if !io::stdin().is_terminal() {
        bail!("Pass --role provider, --role user, or --role both for non-interactive setup.");
    }

    loop {
        print!("Choose role [provider/user/both]: ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        match answer.trim().to_lowercase().as_str() {
            "provider" | "prov" | "p" => return Ok(Role::Provider),
            "user" | "u" => return Ok(Role::User),
            "both" | "b" => return Ok(Role::Both),
            _ => eprintln!("Please enter provider, user, or both."),
        }
    }
}

async fn materialize_wallet(
    args: &SetupArgs,
    keypair_path: &Path,
    password: &str,
) -> Result<WalletInfo> {
    let exists = keypair_path.exists();
    match args.wallet {
        WalletMode::Auto => {
            if exists {
                inspect_wallet(keypair_path, password).await
            } else {
                create_wallet(keypair_path, password, args.mnemonic.as_deref(), false).await
            }
        }
        WalletMode::Create => {
            if exists && !args.force {
                bail!(
                    "{} already exists; use --wallet reuse or pass --force.",
                    keypair_path.display()
                );
            }
            create_wallet(keypair_path, password, args.mnemonic.as_deref(), args.force).await
        }
        WalletMode::Import => {
            if exists && !args.force {
                bail!(
                    "{} already exists; use --wallet reuse or pass --force.",
                    keypair_path.display()
                );
            }
            let mnemonic = args
                .mnemonic
                .as_deref()
                .context("--wallet import requires --mnemonic")?;
            create_wallet(keypair_path, password, Some(mnemonic), true).await
        }
        WalletMode::Reuse => {
            if !exists {
                bail!(
                    "{} does not exist; use --wallet create/import.",
                    keypair_path.display()
                );
            }
            inspect_wallet(keypair_path, password).await
        }
    }
}

async fn create_wallet(
    keypair_path: &Path,
    password: &str,
    mnemonic: Option<&str>,
    force: bool,
) -> Result<WalletInfo> {
    let mut args = vec![
        "create".to_owned(),
        "--keypair".to_owned(),
        keypair_path.display().to_string(),
    ];
    if !password.is_empty() {
        args.extend(["--password".to_owned(), password.to_owned()]);
    }
    if let Some(mnemonic) = mnemonic {
        args.extend(["--mnemonic".to_owned(), mnemonic.to_owned()]);
    }
    if force {
        args.push("--force".to_owned());
    }
    run_wallet_helper(args).await
}

async fn inspect_wallet(keypair_path: &Path, password: &str) -> Result<WalletInfo> {
    let mut args = vec![
        "inspect".to_owned(),
        "--keypair".to_owned(),
        keypair_path.display().to_string(),
    ];
    if !password.is_empty() {
        args.extend(["--password".to_owned(), password.to_owned()]);
    }
    run_wallet_helper(args).await
}

async fn sign_message(keypair_path: &Path, password: &str, message: &str) -> Result<String> {
    let mut args = vec![
        "sign".to_owned(),
        "--keypair".to_owned(),
        keypair_path.display().to_string(),
        "--message".to_owned(),
        message.to_owned(),
    ];
    if !password.is_empty() {
        args.extend(["--password".to_owned(), password.to_owned()]);
    }
    let output: SignOutput = run_wallet_helper(args).await?;
    Ok(output.signature)
}

async fn sign_hex(keypair_path: &Path, password: &str, message_hex: &str) -> Result<String> {
    let mut args = vec![
        "sign".to_owned(),
        "--keypair".to_owned(),
        keypair_path.display().to_string(),
        "--message-hex".to_owned(),
        message_hex.to_owned(),
    ];
    if !password.is_empty() {
        args.extend(["--password".to_owned(), password.to_owned()]);
    }
    let output: SignOutput = run_wallet_helper(args).await?;
    Ok(output.signature)
}

async fn run_wallet_helper<T>(args: Vec<String>) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/wallet-helper.mjs");
    let output = Command::new("node")
        .arg(&helper)
        .args(args)
        .output()
        .await
        .with_context(|| format!("running wallet helper {}", helper.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("wallet helper failed: {}", stderr.trim());
    }

    serde_json::from_slice(&output.stdout).context("parsing wallet helper JSON output")
}

fn write_config(
    home: &Path,
    store_path: &Path,
    wallet: &WalletInfo,
    role: Role,
    store_name: &str,
    rpc_url: &str,
) -> Result<PathBuf> {
    let config_path = home.join("config.toml");
    let address = wallet.address.as_deref().unwrap_or("");
    let derivation_path = wallet.derivation_path.as_deref().unwrap_or("");
    let contents = format!(
        concat!(
            "[identity]\n",
            "public_key = {}\n",
            "address = {}\n",
            "derivation_path = {}\n",
            "keypair_path = {}\n",
            "store_name = {}\n",
            "store_path = {}\n\n",
            "[role]\n",
            "mode = {}\n\n",
            "[network]\n",
            "rpc_url = {}\n",
            "gateway_url = {}\n",
            "paygate_url = {}\n"
        ),
        toml_string(&wallet.public_key),
        toml_string(address),
        toml_string(derivation_path),
        toml_string(&wallet.keypair_path),
        toml_string(store_name),
        toml_string(&store_path.display().to_string()),
        toml_string(role.as_str()),
        toml_string(rpc_url),
        toml_string(DEFAULT_GATEWAY_URL),
        toml_string(DEFAULT_PAYGATE_URL),
    );
    fs::write(&config_path, contents)
        .with_context(|| format!("writing {}", config_path.display()))?;
    Ok(config_path)
}

fn toml_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

async fn resolve_rules(
    rules_ver: Option<u64>,
    rules_hash: Option<&str>,
    rpc: &PeerRpcClient,
    rules_doc: Option<&RulesDoc>,
) -> Result<RulesRef> {
    let rules = match (rules_ver, rules_hash) {
        (Some(ver), Some(hash)) => {
            validate_rules_hash(hash)?;
            RulesRef {
                ver,
                hash: hash.to_owned(),
            }
        }
        (None, None) => {
            let state = rpc
                .state(Some("rules/current"), Some(false))
                .await
                .context("reading rules/current from peer RPC")?;
            let value = state
                .get("value")
                .filter(|value| !value.is_null())
                .context("rules/current is not set; ask the admin peer to set rules first")?;
            let ver = value
                .get("ver")
                .and_then(Value::as_u64)
                .context("rules/current.ver is missing or invalid")?;
            let hash = value
                .get("hash")
                .and_then(Value::as_str)
                .context("rules/current.hash is missing or invalid")?
                .to_owned();
            validate_rules_hash(&hash)?;
            RulesRef { ver, hash }
        }
        _ => bail!("--rules-ver and --rules-hash must be provided together."),
    };

    if let Some(rules_doc) = rules_doc {
        if rules.hash != rules_doc.hash {
            bail!(
                "contract rules hash {} does not match BLAKE3({}) {}",
                rules.hash,
                rules_doc.path.display(),
                rules_doc.hash
            );
        }
    }

    Ok(rules)
}

fn validate_rules_hash(hash: &str) -> Result<()> {
    if hash.len() != 64 || !hash.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        bail!("rules hash must be a 32-byte hex BLAKE3 digest (64 characters).");
    }
    Ok(())
}

async fn read_consent_state(rpc: &PeerRpcClient, public_key: &str) -> Result<Option<Value>> {
    let key = format!("consent/{public_key}");
    let state = rpc
        .state(Some(&key), Some(false))
        .await
        .with_context(|| format!("reading {key} from peer RPC"))?;
    Ok(state.get("value").cloned().filter(|value| !value.is_null()))
}

fn consent_matches(consent: Option<&Value>, rules: &RulesRef) -> bool {
    consent.is_some_and(|state| {
        state.get("ver").and_then(Value::as_u64) == Some(rules.ver)
            && state.get("hash").and_then(Value::as_str) == Some(rules.hash.as_str())
    })
}

async fn submit_consent(
    rpc: &PeerRpcClient,
    keypair_path: &Path,
    password: &str,
    wallet: &WalletInfo,
    rules: RulesRef,
    sim: bool,
) -> Result<ConsentReport> {
    let consent_message = format!("mayhem-consent{}{}", rules.ver, rules.hash);
    let consent_sig = sign_message(keypair_path, password, &consent_message).await?;
    let prepared_command = json!({
        "type": "consent",
        "value": {
            "op": "consent",
            "ver": rules.ver,
            "hash": rules.hash,
            "sig": consent_sig,
        }
    });

    let nonce_response = rpc
        .contract_nonce()
        .await
        .context("requesting contract nonce")?;
    let nonce = nonce_response
        .get("nonce")
        .and_then(Value::as_str)
        .context("RPC nonce response did not include nonce")?;

    let prepared = rpc
        .prepare_tx(json!({
            "prepared_command": prepared_command.clone(),
            "address": wallet.public_key,
            "nonce": nonce,
        }))
        .await
        .context("preparing consent tx")?;
    let tx = prepared
        .get("tx")
        .and_then(Value::as_str)
        .context("RPC prepare response did not include tx")?
        .to_owned();
    let command_hash = prepared
        .get("command_hash")
        .and_then(Value::as_str)
        .map(str::to_owned);

    let signature = sign_hex(keypair_path, password, &tx).await?;
    let submitted = rpc
        .submit_tx(json!({
            "tx": tx,
            "prepared_command": prepared_command.clone(),
            "address": wallet.public_key,
            "signature": signature,
            "nonce": nonce,
            "sim": sim,
        }))
        .await
        .context("submitting consent tx")?;
    let result = Some(
        submitted
            .get("result")
            .cloned()
            .unwrap_or_else(|| submitted.clone()),
    );

    let state = if sim {
        None
    } else {
        Some(wait_for_consent_state(rpc, &wallet.public_key, &rules).await?)
    };

    Ok(ConsentReport {
        skipped: false,
        simulated: sim,
        rules: Some(rules),
        tx: Some(tx),
        command_hash,
        result,
        state,
    })
}

async fn wait_for_consent_state(
    rpc: &PeerRpcClient,
    public_key: &str,
    rules: &RulesRef,
) -> Result<Value> {
    let key = format!("consent/{public_key}");
    let mut last = Value::Null;
    for _ in 0..120 {
        let state = rpc
            .state(Some(&key), Some(false))
            .await
            .with_context(|| format!("reading {key} from peer RPC"))?;
        last = state.get("value").cloned().unwrap_or(Value::Null);
        if last.get("ver").and_then(Value::as_u64) == Some(rules.ver)
            && last.get("hash").and_then(Value::as_str) == Some(rules.hash.as_str())
        {
            return Ok(last);
        }
        sleep(Duration::from_millis(500)).await;
    }

    bail!("timed out waiting for persisted consent state; last value: {last}");
}

fn print_human_report(report: &Value) -> Result<()> {
    println!("Mayhem setup complete.");
    println!("Home: {}", report["home"].as_str().unwrap_or(""));
    println!("Role: {}", report["role"].as_str().unwrap_or(""));
    println!("Config: {}", report["config_path"].as_str().unwrap_or(""));
    println!(
        "Wallet: {}",
        if report["wallet"]["created"].as_bool().unwrap_or(false) {
            "created"
        } else {
            "reused"
        }
    );
    println!(
        "Public key: {}",
        report["wallet"]["public_key"].as_str().unwrap_or("")
    );
    if let Some(address) = report["wallet"]["address"].as_str() {
        println!("Address: {address}");
    }
    if let Some(mnemonic) = report["wallet"]["mnemonic"].as_str() {
        println!("Mnemonic (shown once): {mnemonic}");
    }

    let consent = &report["consent"];
    if consent["skipped"].as_bool().unwrap_or(false) {
        println!("Consent: skipped");
    } else if consent["simulated"].as_bool().unwrap_or(false) {
        println!(
            "Consent: simulated for rules v{}",
            consent["rules"]["ver"].as_u64().unwrap_or(0)
        );
    } else {
        println!(
            "Consent: submitted and observed for rules v{}",
            consent["rules"]["ver"].as_u64().unwrap_or(0)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct FakeEngineBackend {
        output: mayhem_engine::GenerateOutput,
        last_request: Option<GenerateRequest>,
    }

    impl FakeEngineBackend {
        fn new(text: &str) -> Self {
            Self {
                output: mayhem_engine::GenerateOutput {
                    text: text.to_owned(),
                    usage: mayhem_engine::UsageCounters::new(4, 2),
                    finish_reason: mayhem_engine::FinishReason::Stop,
                },
                last_request: None,
            }
        }
    }

    impl EngineBackend for FakeEngineBackend {
        fn backend_id(&self) -> &'static str {
            "fake"
        }

        fn load(
            &mut self,
            config: LoadConfig,
        ) -> mayhem_engine::Result<mayhem_engine::LoadedModelInfo> {
            Ok(mayhem_engine::LoadedModelInfo {
                backend: self.backend_id().to_owned(),
                artifact: config.artifact,
                ctx_size: config.ctx_size,
                n_ctx_train: config.ctx_size,
                n_vocab: 0,
            })
        }

        fn tokenize(&self, text: &str) -> mayhem_engine::Result<mayhem_engine::Tokenization> {
            Ok(mayhem_engine::Tokenization {
                token_ids: text.bytes().map(i32::from).collect(),
            })
        }

        fn generate(
            &mut self,
            request: GenerateRequest,
            _sink: &mut dyn mayhem_engine::TokenSink,
        ) -> mayhem_engine::Result<mayhem_engine::GenerateOutput> {
            self.last_request = Some(request);
            Ok(self.output.clone())
        }
    }

    #[test]
    fn toml_string_escapes_quotes_and_backslashes() {
        assert_eq!(toml_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    #[test]
    fn rules_hash_allows_hex_only() {
        assert!(validate_rules_hash(&"a".repeat(64)).is_ok());
        assert!(validate_rules_hash("not-hex").is_err());
        assert!(validate_rules_hash("aabb001122").is_err());
        assert!(validate_rules_hash("").is_err());
    }

    #[test]
    fn provider_candidates_require_admin_enclave_and_matching_catalog_artifact() {
        let root = "aa".repeat(32);
        let catalog = test_catalog(&root);
        let hardware = test_hardware(FixtureProfile::CpuOnly);
        let args = test_provider_start_args();
        let contract = test_contract(&root);

        let candidates = build_provider_candidates(&contract, &catalog, &hardware, &args).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].enclave.model_id, "test/model@4bit");
        assert_eq!(candidates[0].artifact_name, "gguf-q4_k_m");

        let mut retired = contract.clone();
        retired.enclaves[0].status = "retired".to_owned();
        assert!(build_provider_candidates(&retired, &catalog, &hardware, &args).is_err());

        let mut mismatched = contract;
        mismatched.enclaves[0].artifact_root = "bb".repeat(32);
        assert!(build_provider_candidates(&mismatched, &catalog, &hardware, &args).is_err());
    }

    #[test]
    fn gateway_models_are_built_from_canonical_contract_state() {
        let root = "aa".repeat(32);
        let mut contract = test_contract(&root);
        let models = gateway_models_from_contract(&contract).unwrap();

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "test/model@4bit");
        assert_eq!(models[0].owned_by, "mayhem");
        assert_eq!(models[0].mayhem.source, "contract");
        assert_eq!(models[0].mayhem.providers_online, 1);
        assert_eq!(models[0].mayhem.rooms, 1);
        assert_eq!(models[0].mayhem.price_ref_mu.denom, "mu_usd");
        assert_eq!(models[0].mayhem.price_ref_mu.ver, 1);
        assert_eq!(models[0].mayhem.price_ref_mu.in_per_1k, 1);
        assert_eq!(models[0].mayhem.price_ref_mu.out_per_1k, 2);
        assert_eq!(models[0].mayhem.attestation_tiers["T1"], 1);
        assert!(models[0].mayhem.caps.tools);
        assert_eq!(models[0].mayhem.caps.ctx, 8192);
        assert_eq!(models[0].mayhem.route_candidates.len(), 1);
        assert_eq!(
            models[0].mayhem.route_candidates[0].provider,
            "55".repeat(32)
        );
        assert_eq!(
            models[0].mayhem.route_candidates[0].enclave_id,
            "11".repeat(32)
        );
        assert_eq!(models[0].mayhem.route_candidates[0].room_id, "room-a");
        assert_eq!(models[0].mayhem.route_candidates[0].price_ver, 1);

        contract.prices.clear();
        let err = gateway_models_from_contract(&contract)
            .expect_err("missing current contract price should hide model");
        assert!(format!("{err:#}").contains("no canonical contract-backed models"));

        let mut contract = test_contract(&root);
        contract.roomserve.clear();
        let err = gateway_models_from_contract(&contract)
            .expect_err("missing active room participation should hide model");
        assert!(format!("{err:#}").contains("no canonical contract-backed models"));

        let mut contract = test_contract(&root);
        contract.providers[0].status = "banned".to_owned();
        let err =
            gateway_models_from_contract(&contract).expect_err("banned provider should hide model");
        assert!(format!("{err:#}").contains("no canonical contract-backed models"));
    }

    #[test]
    fn provider_session_open_enforces_admin_terms() {
        let terms = test_provider_session_terms();
        let frame = test_session_open_frame(&terms);
        assert_eq!(
            provider_session_open_decision(&frame, &terms),
            ProviderSessionDecision::Accept
        );

        let mut wrong_enclave = frame.clone();
        wrong_enclave["enclave_id"] = json!("22".repeat(32));
        assert!(matches!(
            provider_session_open_decision(&wrong_enclave, &terms),
            ProviderSessionDecision::Reject {
                code: "ENCLAVE",
                ..
            }
        ));

        let mut wrong_price = frame.clone();
        wrong_price["price_ver"] = json!(terms.price_ver + 1);
        assert!(matches!(
            provider_session_open_decision(&wrong_price, &terms),
            ProviderSessionDecision::Reject {
                code: "PRICE_VER",
                ..
            }
        ));

        let mut stale_rules = frame.clone();
        stale_rules["rules_ver"] = json!(terms.rules_ver + 1);
        assert!(matches!(
            provider_session_open_decision(&stale_rules, &terms),
            ProviderSessionDecision::Reject {
                code: "CONSENT",
                ..
            }
        ));

        let mut bad_voucher = frame;
        bad_voucher["voucher"]["price_ver"] = json!(terms.price_ver + 1);
        assert!(matches!(
            provider_session_open_decision(&bad_voucher, &terms),
            ProviderSessionDecision::Reject {
                code: "VOUCHER",
                ..
            }
        ));
    }

    #[test]
    fn provider_session_response_preserves_tool_call_shape() {
        let terms = test_provider_session_terms();
        let body = json!({
            "messages": [{ "role": "user", "content": "write a file" }],
            "tools": [{
                "type": "function",
                "function": { "name": "write", "parameters": { "type": "object" } }
            }],
            "tool_choice": "auto",
            "stream": true
        });
        let output = provider_session_response(&terms, &body);

        assert_eq!(output.finish_reason, "tool_calls");
        let tool = output.tool.expect("tool call");
        assert_eq!(tool["name"], "write");
        assert!(tool["arguments"]
            .as_str()
            .expect("arguments")
            .contains("mayhem-opencode-tool-ok"));
    }

    #[test]
    fn provider_engine_request_uses_tool_grammar_from_openai_body() {
        let body = json!({
            "messages": [
                { "role": "system", "content": "be precise" },
                { "role": "user", "content": "write a file" }
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "write",
                    "description": "write a file",
                    "parameters": {
                        "type": "object",
                        "properties": { "filePath": { "type": "string" } }
                    }
                }
            }],
            "tool_choice": { "type": "function", "function": { "name": "write" } },
            "max_tokens": 12,
            "temperature": 0,
            "seed": 42
        });
        let request = provider_engine_request_from_body(&body).unwrap();

        assert!(request.prompt.contains("system: be precise"));
        assert!(request.prompt.contains("user: write a file"));
        assert_eq!(request.max_new_tokens, 12);
        assert_eq!(request.seed, Some(42));
        let Some(GrammarSpec::ToolCall { tools }) = request.grammar else {
            panic!("expected tool-call grammar");
        };
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "write");
        assert_eq!(tools[0].description.as_deref(), Some("write a file"));
    }

    #[test]
    fn provider_engine_request_uses_json_schema_for_json_mode() {
        let body = json!({
            "messages": [{ "role": "user", "content": "return json" }],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "schema": {
                        "type": "object",
                        "required": ["ok"],
                        "properties": { "ok": { "type": "boolean" } }
                    }
                }
            }
        });
        let request = provider_engine_request_from_body(&body).unwrap();

        let Some(GrammarSpec::JsonSchema { schema }) = request.grammar else {
            panic!("expected json schema grammar");
        };
        assert_eq!(schema["required"][0], "ok");
    }

    #[test]
    fn provider_engine_session_response_uses_backend_output() {
        let mut backend = FakeEngineBackend::new("engine says hi");
        let body = json!({
            "messages": [{ "role": "user", "content": "hello" }],
            "max_tokens": 8
        });
        let output = provider_engine_session_response(&mut backend, &body).unwrap();

        assert_eq!(output.content, "engine says hi");
        assert!(output.tool.is_none());
        assert_eq!(output.finish_reason, "stop");
        assert_eq!(output.prompt_tokens, 4);
        assert_eq!(output.completion_tokens, 2);
        let request = backend.last_request.expect("engine request");
        assert!(request.prompt.contains("user: hello"));
        assert_eq!(request.max_new_tokens, 8);
        assert!(request.grammar.is_none());
    }

    #[test]
    fn provider_engine_session_response_requires_valid_tool_json() {
        let mut backend = FakeEngineBackend::new("not json");
        let body = json!({
            "messages": [{ "role": "user", "content": "write a file" }],
            "tools": [{
                "type": "function",
                "function": { "name": "write", "parameters": { "type": "object" } }
            }],
            "tool_choice": "auto"
        });
        let err = provider_engine_session_response(&mut backend, &body)
            .expect_err("tool mode requires valid tool-call JSON");

        assert!(
            format!("{err:#}").contains("provider engine did not return valid tool-call JSON"),
            "{err:#}"
        );
        assert!(matches!(
            backend
                .last_request
                .expect("engine request")
                .grammar
                .expect("tool grammar"),
            GrammarSpec::ToolCall { .. }
        ));
    }

    #[test]
    fn provider_engine_load_config_uses_admin_artifact_shape() {
        let root = "aa".repeat(32);
        let catalog = test_catalog(&root);
        let contract = test_contract(&root);
        let hardware = test_hardware(FixtureProfile::CpuOnly);
        let args = test_provider_start_args();
        let selected = build_provider_candidates(&contract, &catalog, &hardware, &args)
            .unwrap()
            .remove(0);
        let config =
            provider_engine_load_config(&selected, Path::new("/tmp/admin-approved.gguf")).unwrap();

        assert_eq!(
            config.artifact.path,
            PathBuf::from("/tmp/admin-approved.gguf")
        );
        assert_eq!(config.artifact.format, mayhem_engine::ArtifactFormat::Gguf);
        assert_eq!(config.ctx_size, 8192);
        assert_eq!(config.gpu_layers, Some(0));
    }

    #[test]
    fn provider_engine_tool_call_output_maps_engine_json_to_openai_shape() {
        let tool = provider_engine_tool_call_output(
            r#"{ "tool": "write", "arguments": { "filePath": "ok.txt" } }"#,
        )
        .expect("tool call");

        assert_eq!(tool["name"], "write");
        assert_eq!(tool["arguments"], r#"{"filePath":"ok.txt"}"#);
        assert!(tool["id"].as_str().unwrap().starts_with("call-"));
    }

    #[tokio::test]
    async fn provider_local_artifact_must_match_admin_root() {
        let temp = env::temp_dir().join(format!(
            "mayhem-cli-local-artifact-{}-{}",
            std::process::id(),
            unix_epoch_millis().unwrap()
        ));
        fs::create_dir_all(&temp).unwrap();
        let source = temp.join("artifact.gguf");
        fs::write(&source, b"admin-approved artifact bytes").unwrap();
        let root = build_merkle_manifest(&source, 8).unwrap().root;
        let catalog = test_catalog(&root);
        let hardware = test_hardware(FixtureProfile::CpuOnly);
        let contract = test_contract(&root);
        let mut args = test_provider_start_args();
        args.artifact = Some(source.clone());
        args.chunk_size = 8;
        let selected =
            build_provider_candidates(&contract, &catalog, &hardware, &args).unwrap()[0].clone();

        let accepted = download_provider_artifact(&args, &temp.join("downloads-ok"), &selected)
            .await
            .unwrap();
        assert!(accepted.exists());
        assert_eq!(
            build_merkle_manifest(&accepted, 8).unwrap().root,
            selected.enclave.artifact_root
        );

        let mut wrong = selected.clone();
        wrong.enclave.artifact_root = "bb".repeat(32);
        let err = download_provider_artifact(&args, &temp.join("downloads-bad"), &wrong)
            .await
            .expect_err("wrong local artifact root must be rejected");
        assert!(
            format!("{err:#}").contains("artifact merkle root mismatch"),
            "{err:#}"
        );

        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn provider_rooms_auto_selects_only_open_matching_admin_rooms() {
        let root = "aa".repeat(32);
        let contract = test_contract(&root);
        let enclave = &contract.enclaves[0];
        let rooms = select_provider_rooms(&contract.rooms, enclave, "auto").unwrap();

        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].room_id, "room-a");
        assert!(select_provider_rooms(&contract.rooms, enclave, "room-other").is_err());
    }

    #[test]
    fn epoch_receipt_messages_round_trip_gateway_receipts() {
        let receipt = json!({
            "voucher": { "session_id": "s1" },
            "receipt": {
                "schema_version": 1,
                "session_id": "s1",
                "seq": 2,
                "final": true,
                "user": "u",
                "provider": "p",
                "mu_owed_cum": 100
            },
            "receipt_ack": { "session_id": "s1", "seq": 2, "user_sig": "sig" }
        });
        let id = receipt_id(&receipt);
        assert_eq!(id, "s1:2");

        let message = epoch_receipt_message(7, id.clone(), receipt.clone()).unwrap();
        assert_eq!(message["t"], "epoch.receipt");
        assert_eq!(message["epoch"], 7);
        assert_eq!(message["receipt_id"], id);

        let event = json!({
            "type": "sidechannel_message",
            "channel": "mx/epoch/7",
            "message": message,
        });
        let (extracted_id, extracted) =
            epoch_receipt_from_sidechannel_event(&event, 7, "mx/epoch/7").unwrap();
        assert_eq!(extracted_id, "s1:2");
        assert_eq!(extracted, receipt);
    }

    #[test]
    fn epoch_receipt_collector_filters_wrong_channel_epoch_and_type() {
        let receipt = json!({ "receipt": { "session_id": "s1", "seq": 1 } });
        let message = epoch_receipt_message(7, receipt_id(&receipt), receipt).unwrap();
        let event = json!({
            "type": "sidechannel_message",
            "channel": "mx/epoch/7",
            "message": message,
        });
        assert!(epoch_receipt_from_sidechannel_event(&event, 7, "mx/epoch/7").is_some());
        assert!(epoch_receipt_from_sidechannel_event(&event, 8, "mx/epoch/7").is_none());
        assert!(epoch_receipt_from_sidechannel_event(&event, 7, "mx/epoch/8").is_none());

        let wrong_type = json!({
            "type": "sidechannel_message",
            "channel": "mx/epoch/7",
            "message": { "t": "hb", "epoch": 7, "receipt": {} },
        });
        assert!(epoch_receipt_from_sidechannel_event(&wrong_type, 7, "mx/epoch/7").is_none());
    }

    #[test]
    fn earning_view_computes_released_mu_after_holdback_and_paid() {
        let view = earning_view(LedgerEarningRecord {
            provider: "provider-a".to_owned(),
            denom: "mu_usd".to_owned(),
            total_mu: 10_000,
            held_mu: 2_500,
            paid_cum_mu: 3_000,
            holdbacks: vec![LedgerHoldbackBucket {
                epoch: 7,
                mu: 2_500,
            }],
            updated_epoch: 7,
            updated_at: None,
            last_holdback_release_epoch: Some(7),
            last_payout_rate_ts: None,
            last_payout_msb_tx_hash: Some("aa".repeat(32)),
        })
        .unwrap();

        assert_eq!(view.released_mu, 4_500);
        assert_eq!(view.holdbacks[0].epoch, 7);

        assert!(earning_view(LedgerEarningRecord {
            total_mu: 1,
            held_mu: 1,
            paid_cum_mu: 1,
            provider: "bad-provider".to_owned(),
            denom: "mu_usd".to_owned(),
            holdbacks: Vec::new(),
            updated_epoch: 0,
            updated_at: None,
            last_holdback_release_epoch: None,
            last_payout_rate_ts: None,
            last_payout_msb_tx_hash: None,
        })
        .is_err());
    }

    #[test]
    fn evidence_checks_cover_all_epoch_roots_and_totals() {
        let recomputed = json!({
            "roots": {
                "dep": "a".repeat(64),
                "use": "b".repeat(64),
                "earn": "c".repeat(64),
                "fee": "d".repeat(64),
                "pay": "e".repeat(64),
            },
            "totals": {
                "dep_count": 1,
                "dep_mu": 2,
                "use_count": 3,
                "use_mu": 4,
                "provider_count": 5,
                "earn_mu": 6,
                "fee_mu": 7,
                "fee_cum_mu": 8,
                "pay_count": 9,
                "pay_mu": 10,
            }
        });
        let evidence = EpochEvidenceSnapshot {
            dep: Some(json!({
                "merkle_root": "a".repeat(64),
                "count": 1,
                "mu_total": 2,
            })),
            r#use: Some(json!({
                "merkle_root": "b".repeat(64),
                "sessions": 3,
                "mu_total": 4,
                "providers": 5,
            })),
            earn: Some(json!({
                "merkle_root": "c".repeat(64),
                "provider_count": 5,
                "mu_cum_total": 6,
            })),
            fee: Some(json!({
                "merkle_root": "d".repeat(64),
                "mu_fee_epoch": 7,
                "mu_fee_cum": 8,
            })),
            pay: Some(json!({
                "merkle_root": "e".repeat(64),
                "count": 9,
                "mu_total": 10,
            })),
        };

        let checks = verify_epoch_evidence(4, &recomputed, &evidence);
        assert_eq!(checks.len(), 16);
        assert!(checks.iter().all(|check| check.ok));

        let mismatched = EpochEvidenceSnapshot {
            pay: Some(json!({
                "merkle_root": "e".repeat(64),
                "count": 9,
                "mu_total": 11,
            })),
            ..evidence
        };
        let checks = verify_epoch_evidence(4, &recomputed, &mismatched);
        assert!(checks.iter().any(|check| {
            check.key == "ev/pay/4.mu_total" && !check.ok && check.actual == json!(11)
        }));
    }

    #[test]
    fn pay_amount_parser_uses_integer_micro_usd() {
        assert_eq!(parse_usd_amount_to_mu("10").unwrap(), 10_000_000);
        assert_eq!(parse_usd_amount_to_mu("10.25").unwrap(), 10_250_000);
        assert_eq!(parse_usd_amount_to_mu("0.01").unwrap(), 10_000);
        assert!(parse_usd_amount_to_mu("0").is_err());
        assert!(parse_usd_amount_to_mu("1.001").is_err());
        assert!(parse_usd_amount_to_mu("-1").is_err());
    }

    #[test]
    fn balance_record_defaults_missing_contract_key_to_zero_mu_usd() {
        let record = normalize_balance_record("user", None).unwrap();

        assert_eq!(record["user"], "user");
        assert_eq!(record["denom"], "mu_usd");
        assert_eq!(record["mu"], 0);
        assert_eq!(record["updated_epoch"], 0);
        assert!(record["updated_at"].is_null());
    }

    #[test]
    fn balance_record_validates_denom_user_and_mu() {
        let record = normalize_balance_record(
            "user",
            Some(json!({
                "user": "user",
                "denom": "mu_usd",
                "mu": 42,
                "updated_epoch": 3,
                "updated_at": 7
            })),
        )
        .unwrap();
        assert_eq!(record["mu"], 42);

        assert!(normalize_balance_record(
            "user",
            Some(json!({ "user": "user", "denom": "provider_coin", "mu": 1 }))
        )
        .is_err());
        assert!(normalize_balance_record(
            "user",
            Some(json!({ "user": "other", "denom": "mu_usd", "mu": 1 }))
        )
        .is_err());
        assert!(normalize_balance_record(
            "user",
            Some(json!({ "user": "user", "denom": "mu_usd" }))
        )
        .is_err());
    }

    #[test]
    fn pay_checkout_extraction_requires_hosted_urls() {
        let stripe = checkout_from_paygate_response(
            PayRail::Stripe,
            &json!({
                "checkout_session": {
                    "id": "cs_test",
                    "url": "https://checkout.stripe.com/c/pay/cs_test",
                    "payment_intent": "pi_test"
                }
            }),
        )
        .unwrap();
        assert_eq!(stripe.id, "cs_test");
        assert_eq!(stripe.reference.as_deref(), Some("pi_test"));

        let coinbase = checkout_from_paygate_response(
            PayRail::Coinbase,
            &json!({
                "charge": {
                    "id": "charge_test",
                    "code": "CBTEST",
                    "hosted_url": "https://commerce.coinbase.com/charges/CBTEST"
                }
            }),
        )
        .unwrap();
        assert_eq!(coinbase.id, "charge_test");
        assert_eq!(coinbase.reference.as_deref(), Some("CBTEST"));

        assert!(checkout_from_paygate_response(
            PayRail::Coinbase,
            &json!({ "charge": { "id": "charge_test" } })
        )
        .is_err());
    }

    #[test]
    fn pay_checkout_handoff_includes_copy_paste_url() {
        let lines = checkout_handoff_lines(
            PayRail::Stripe,
            10_000_000,
            "https://checkout.stripe.com/c/pay/cs_test",
        );

        assert_eq!(lines[0], "Mayhem stripe checkout for 10.00");
        assert_eq!(
            lines[1],
            "Copy/paste checkout URL: https://checkout.stripe.com/c/pay/cs_test"
        );
    }

    #[test]
    fn use_gateway_bind_addr_defaults_to_loopback_port() {
        let bind = gateway_bind_addr(None, None, 31_435).unwrap();

        assert_eq!(bind.to_string(), "127.0.0.1:31435");
        assert_eq!(gateway_public_url(bind), "http://127.0.0.1:31435");
        assert_eq!(
            gateway_v1_url(&gateway_public_url(bind)),
            "http://127.0.0.1:31435/v1"
        );
    }

    #[test]
    fn use_gateway_bind_addr_honors_config_port_and_explicit_bind() {
        let config = MayhemConfig {
            identity: None,
            network: Some(ConfigNetwork {
                rpc_url: None,
                sc_bridge_url: None,
                sc_bridge_token: None,
                gateway_url: Some("http://127.0.0.1:4242/v1".to_owned()),
                paygate_url: None,
            }),
            provider: None,
            role: None,
        };

        let from_config = gateway_bind_addr(Some(&config), None, 11_435).unwrap();
        assert_eq!(from_config.to_string(), "127.0.0.1:4242");

        let explicit = gateway_bind_addr(Some(&config), Some("0.0.0.0:5252"), 11_435).unwrap();
        assert_eq!(explicit.to_string(), "0.0.0.0:5252");
        assert_eq!(gateway_public_url(explicit), "http://127.0.0.1:5252");
    }

    #[test]
    fn model_summaries_extract_gateway_mayhem_fields() {
        let summaries = gateway_model_summaries(&[json!({
            "id": "mayhem/test",
            "mayhem": {
                "providers_online": 3,
                "rooms": 2,
                "price_ref_mu": {
                    "denom": "mu_usd",
                    "in_per_1k": 20,
                    "out_per_1k": 60
                },
                "attestation_tiers": { "T1": 2, "T2": 1 },
                "caps": { "tools": true, "json": false, "ctx": 8192 }
            }
        })])
        .unwrap();

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "mayhem/test");
        assert_eq!(summaries[0].providers_online, 3);
        assert_eq!(summaries[0].rooms, 2);
        assert_eq!(summaries[0].denom, "mu_usd");
        assert_eq!(summaries[0].in_per_1k_mu, 20);
        assert_eq!(summaries[0].out_per_1k_mu, 60);
        assert!(summaries[0].tools);
        assert!(!summaries[0].json);
        assert_eq!(summaries[0].context, 8192);
        assert_eq!(summaries[0].attestation_tiers["T2"], 1);
    }

    #[test]
    fn canary_probe_request_uses_normal_chat_completion_shape() {
        let prompt = CanaryPrompt {
            id: "shape".to_owned(),
            messages: vec![json!({ "role": "user", "content": "return ok" })],
            tools: Some(vec![json!({
                "type": "function",
                "function": {
                    "name": "write",
                    "parameters": { "type": "object" }
                }
            })]),
            temperature: Some(0.2),
            max_tokens: Some(16),
        };

        let request = canary_probe_request("admin/model", &prompt);

        assert_eq!(request["model"], "admin/model");
        assert_eq!(
            request["messages"],
            json!([{ "role": "user", "content": "return ok" }])
        );
        assert_eq!(request["tools"], json!(prompt.tools.as_ref().unwrap()));
        assert_eq!(request["temperature"], 0.2);
        assert_eq!(request["max_tokens"], 16);
        assert_eq!(request["stream"], false);
        assert!(request.get("canary").is_none());
    }

    #[test]
    fn canary_text_match_normalizes_whitespace_and_scores_mismatch() {
        let exact = evaluate_text_match("alpha\n beta", "alpha beta", 10_000);
        assert!(exact.pass);
        assert_eq!(exact.match_bps, 10_000);
        assert_eq!(exact.total_positions, 10);

        let mismatch = evaluate_text_match("abcdef", "abcxyz", 9_000);
        assert!(!mismatch.pass);
        assert_eq!(mismatch.matched_positions, 3);
        assert_eq!(mismatch.match_bps, 5_000);
    }

    #[test]
    fn canary_token_fingerprint_matches_catalog_format_vectors() {
        assert_eq!(
            canary_token_fingerprint([]),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
        assert_eq!(
            canary_token_fingerprint([1, 2, 3]),
            "04a03410338d287acb82ba338ec1aea060eac0650f256eddc814f743c731cf33"
        );
        assert_eq!(
            canary_token_fingerprint([-1]),
            "650e93bacca01942a5a787f2f3ec4ce560998eb7c250733601a880d7f0c11178"
        );
    }

    #[test]
    fn aggregate_canary_fingerprint_is_prompt_order_and_id_bound() {
        let first = test_calibration_prompt("p1", "aa".repeat(32));
        let second = test_calibration_prompt("p2", "bb".repeat(32));
        let same_digest_different_id = test_calibration_prompt("p3", "aa".repeat(32));

        assert_eq!(
            aggregate_canary_fingerprint(&[first.clone(), second.clone()]),
            aggregate_canary_fingerprint(&[first.clone(), second.clone()])
        );
        assert_ne!(
            aggregate_canary_fingerprint(&[first.clone(), second.clone()]),
            aggregate_canary_fingerprint(&[second, first.clone()])
        );
        assert_ne!(
            aggregate_canary_fingerprint(&[first]),
            aggregate_canary_fingerprint(&[same_digest_different_id])
        );
    }

    #[test]
    fn stable_value_hash_is_object_key_order_independent() {
        let a = json!({ "b": 2, "a": { "d": 4, "c": 3 } });
        let b = json!({ "a": { "c": 3, "d": 4 }, "b": 2 });

        assert_eq!(stable_value_hash(&a), stable_value_hash(&b));
    }

    #[test]
    fn canary_probe_command_omits_absent_optional_receipt_hash() {
        let without_receipt = canary_probe_command(test_canary_probe_command_input(None));
        assert!(without_receipt.get("session_receipt_hash").is_none());
        assert_eq!(without_receipt["op"], "probe_result");
        assert_eq!(without_receipt["enclave_id"], "enclave");

        let with_receipt =
            canary_probe_command(test_canary_probe_command_input(Some("rr".repeat(32))));
        assert_eq!(with_receipt["session_receipt_hash"], "rr".repeat(32));
    }

    #[test]
    fn safe_path_component_removes_model_path_separators() {
        assert_eq!(
            safe_path_component("meta/llama-3.1:8b@4bit"),
            "meta_llama-3.1_8b_4bit"
        );
    }

    #[test]
    fn opencode_merge_preserves_existing_config_and_adds_mayhem_provider() {
        let path = env::temp_dir().join(format!(
            "mayhem-opencode-merge-{}-{}.json",
            std::process::id(),
            now_millis_for_path()
        ));
        fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "$schema": OPENCODE_SCHEMA_URL,
                "provider": {
                    "other": {
                        "npm": "@ai-sdk/openai-compatible",
                        "name": "Other",
                        "models": {}
                    }
                },
                "model": "other/model",
                "enabled_providers": ["other"]
            }))
            .unwrap(),
        )
        .unwrap();
        let models = vec![json!({
            "id": "mayhem/test-model",
            "mayhem": {
                "caps": { "tools": true, "json": true, "ctx": 16384 }
            }
        })];

        let report =
            merge_mayhem_opencode_config(&path, "http://127.0.0.1:11435", Some(&models), true)
                .unwrap();
        let merged: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

        assert!(!report.created);
        assert_eq!(report.models_written, 1);
        assert_eq!(merged["model"], "other/model");
        assert!(merged["provider"]["other"].is_object());
        assert_eq!(
            merged["provider"]["mayhem"]["options"]["baseURL"],
            "http://127.0.0.1:11435/v1"
        );
        assert_eq!(
            merged["provider"]["mayhem"]["models"]["mayhem/test-model"]["tool_call"],
            true
        );
        assert_eq!(
            merged["enabled_providers"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|value| value.as_str() == Some("mayhem"))
                .count(),
            1
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn opencode_model_config_maps_gateway_caps() {
        let models = opencode_models_from_gateway(&[json!({
            "id": "mayhem/no-tools",
            "mayhem": {
                "caps": { "tools": false, "json": true, "ctx": 4096 }
            }
        })])
        .unwrap();
        let model = models.get("mayhem/no-tools").unwrap();
        assert_eq!(model["tool_call"], false);
        assert_eq!(model["limit"]["context"], 4096);
        assert_eq!(model["limit"]["output"], 4096);
    }

    #[test]
    fn opencode_config_home_requires_standard_config_shape() {
        let root = PathBuf::from("/tmp/mayhem-config-home");
        assert_eq!(
            xdg_config_home_for_opencode_config(&root.join("opencode").join("opencode.json")),
            Some(root)
        );
        assert!(
            xdg_config_home_for_opencode_config(Path::new("/tmp/mayhem-opencode.json")).is_none()
        );
    }

    #[test]
    fn receipt_body_accepts_wrapped_and_flattened_receipts() {
        let wrapped = json!({ "receipt": { "body": { "session_id": "s1", "ts": 42 } } });
        let flattened = json!({ "receipt": { "session_id": "s2", "ts": 42 } });

        assert_eq!(
            receipt_body(&wrapped)
                .and_then(|body| body.get("session_id"))
                .and_then(Value::as_str),
            Some("s1")
        );
        assert_eq!(
            receipt_body(&flattened)
                .and_then(|body| body.get("session_id"))
                .and_then(Value::as_str),
            Some("s2")
        );
    }

    fn test_provider_start_args() -> ProviderStartArgs {
        ProviderStartArgs {
            home: None,
            enclave: None,
            rooms: "auto".to_owned(),
            rpc_url: None,
            sc_bridge_url: None,
            sc_bridge_token: None,
            wallet_password: None,
            catalog_path: None,
            signature_path: None,
            keys_dir: None,
            canaries_dir: None,
            artifact: None,
            downloads_dir: None,
            hf_token_file: None,
            engine_backend: "auto".to_owned(),
            fixture: None,
            disk_path: None,
            skip_disk_bench: true,
            chunk_size: DEFAULT_CHUNK_SIZE,
            enclave_binary: None,
            sim: false,
            no_heartbeat: true,
            heartbeat_count: 1,
            serve_sessions: false,
            serve_sessions_seconds: 0,
            dev_session_shim: false,
            print_json: true,
            dev_skip_catalog_verify: true,
        }
    }

    fn test_provider_session_terms() -> ProviderSessionTerms {
        ProviderSessionTerms {
            provider: "55".repeat(32),
            enclave_id: "11".repeat(32),
            model_id: "test/model@4bit".to_owned(),
            price_ver: 7,
            rules_ver: 3,
            ctx: 8192,
        }
    }

    fn test_session_open_frame(terms: &ProviderSessionTerms) -> Value {
        let session_id = "aa".repeat(32);
        json!({
            "t": "s.open",
            "v": 1,
            "session_id": session_id,
            "user": "66".repeat(32),
            "enclave_id": &terms.enclave_id,
            "price_ver": terms.price_ver,
            "rules_ver": terms.rules_ver,
            "voucher": {
                "session_id": session_id,
                "enclave_id": &terms.enclave_id,
                "price_ver": terms.price_ver,
                "max_spend_mu": 1000,
                "checkpoint_every": { "tokens": 8192, "ms": 30000 },
                "user_sig": "77".repeat(64)
            },
            "att_nonce": "88".repeat(32),
            "ts": 1,
            "nonce": "99".repeat(32),
            "sig": "77".repeat(64)
        })
    }

    fn test_hardware(fixture: FixtureProfile) -> HardwareReport {
        probe(ProbeOptions {
            disk_path: PathBuf::from("."),
            run_disk_bench: false,
            disk_bench_mib: 1,
            fixture: Some(fixture),
        })
    }

    fn test_calibration_prompt(
        prompt_id: &str,
        fingerprint: String,
    ) -> CanaryCalibrationPromptReport {
        CanaryCalibrationPromptReport {
            prompt_id: prompt_id.to_owned(),
            max_tokens: 8,
            prompt_tokens: 1,
            completion_tokens: 1,
            token_count: 1,
            fingerprint,
            output_text: None,
        }
    }

    fn test_catalog(root: &str) -> catalog::CatalogDocument {
        let mut artifacts = BTreeMap::new();
        artifacts.insert(
            "gguf-q4_k_m".to_owned(),
            catalog::CatalogArtifact {
                engine: "llama.cpp".to_owned(),
                source: catalog::SourceRef {
                    kind: "huggingface".to_owned(),
                    repo: "test/model".to_owned(),
                    revision: "1".repeat(40),
                    publisher_key: None,
                },
                path: "model.gguf".to_owned(),
                artifact_root: root.to_owned(),
                artifact_root_kind: "blake3_merkle_v1".to_owned(),
                weights_bytes: 42,
                source_sha256: None,
                tokenizer_sha256: None,
                chat_template_sha256: None,
                min_compute_cap: None,
                download_check: false,
                notes: None,
            },
        );
        catalog::CatalogDocument {
            schema_version: 1,
            catalog_id: "test".to_owned(),
            generated_at: "2026-07-02T00:00:00Z".to_owned(),
            models: vec![catalog::CatalogModel {
                model_id: "test/model@4bit".to_owned(),
                family: "test".to_owned(),
                params_b: 1.0,
                tier: "dev".to_owned(),
                provenance: catalog::Provenance {
                    source: catalog::SourceRef {
                        kind: "huggingface".to_owned(),
                        repo: "test/source".to_owned(),
                        revision: "2".repeat(40),
                        publisher_key: None,
                    },
                    conversion: Vec::new(),
                    license: "test".to_owned(),
                    license_sha256: "3".repeat(64),
                },
                artifacts,
                caps: catalog::CatalogCaps {
                    tools: true,
                    json: true,
                    ctx_max: 8192,
                    vision: false,
                },
                requirements: catalog::CatalogRequirements {
                    min_ram_gb: 1,
                    min_vram_gb_full_offload: 0,
                    cpu_flags: Vec::new(),
                    backends: vec!["llama.cpp".to_owned()],
                },
                canary: catalog::CanaryRef {
                    set_id: "test-canary".to_owned(),
                    match_min: 0.9,
                    fingerprints: BTreeMap::new(),
                },
                price_ref_mu: catalog::PriceRef {
                    denom: "mu_usd".to_owned(),
                    in_per_1k: 1,
                    out_per_1k: 2,
                },
            }],
        }
    }

    fn test_contract(root: &str) -> ContractCatalog {
        let enclave = LedgerEnclave {
            enclave_id: "11".repeat(32),
            model_id: "test/model@4bit".to_owned(),
            backend: "llama.cpp".to_owned(),
            artifact_root: root.to_owned(),
            manifest_hash: "22".repeat(32),
            att_tier: 1,
            binary_hash: "33".repeat(32),
            caps: json!({ "tools": true, "json": true, "ctx": 8192 }),
            status: "active".to_owned(),
            created_by: "44".repeat(32),
        };
        ContractCatalog {
            enclaves: vec![enclave],
            rooms: vec![
                LedgerRoom {
                    room_id: "room-a".to_owned(),
                    sidechannel: "mx/room/room-a".to_owned(),
                    model_id: "test/model@4bit".to_owned(),
                    label: "test".to_owned(),
                    status: "open".to_owned(),
                },
                LedgerRoom {
                    room_id: "room-closed".to_owned(),
                    sidechannel: "mx/room/room-closed".to_owned(),
                    model_id: "test/model@4bit".to_owned(),
                    label: "test".to_owned(),
                    status: "closed".to_owned(),
                },
                LedgerRoom {
                    room_id: "room-other".to_owned(),
                    sidechannel: "mx/room/room-other".to_owned(),
                    model_id: "other/model".to_owned(),
                    label: "test".to_owned(),
                    status: "open".to_owned(),
                },
            ],
            roomserve: vec![LedgerRoomServe {
                room_id: "room-a".to_owned(),
                provider: "55".repeat(32),
                enclave_id: "11".repeat(32),
                model_id: "test/model@4bit".to_owned(),
                status: "active".to_owned(),
            }],
            providers: vec![LedgerProvider {
                provider: "55".repeat(32),
                status: "active".to_owned(),
            }],
            prices: vec![LedgerPriceSchedule {
                enclave_id: "11".repeat(32),
                model_id: "test/model@4bit".to_owned(),
                denom: "mu_usd".to_owned(),
                current: Some(LedgerPriceRecord {
                    ver: 1,
                    denom: "mu_usd".to_owned(),
                    in_per_1k_mu: 1,
                    out_per_1k_mu: 2,
                    per_req_mu: 0,
                    min_session_mu: 0,
                    effective_at: 0,
                }),
                pending: None,
            }],
        }
    }

    fn test_canary_probe_command_input(
        session_receipt_hash: Option<String>,
    ) -> CanaryProbeCommandInput {
        CanaryProbeCommandInput {
            probe_id: "probe".to_owned(),
            provider: "provider".to_owned(),
            enclave_id: "enclave".to_owned(),
            epoch: 7,
            at: 42,
            canary_set: "canary-dev-v1".to_owned(),
            match_bps: 9_700,
            pass: true,
            session_receipt_hash,
            evidence_hash: "ee".repeat(32),
        }
    }
}
