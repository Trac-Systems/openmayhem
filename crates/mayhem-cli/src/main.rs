#![forbid(unsafe_code)]

mod catalog;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use mayhem_bridge::{
    BridgeError, PeerRpcClient, ScBridgeClient, ScBridgeConfig, DEFAULT_RPC_URL,
    DEFAULT_SC_BRIDGE_URL,
};
use mayhem_enclave::{
    boot_sealed_store, build_merkle_manifest, download_resumable,
    finalize_tier1_attestation_report, load_or_create_runtime_keypair_store, measure_binary,
    prepare_tier1_attestation_report, seal_artifact, BootOptions, DownloadReport, DownloadRequest,
    DownloadSource, KeyContext, RuntimeKeyContext, RuntimeKeypair, RuntimeKeypairStoreOptions,
    SealOptions, Tier1AttestationDraft, Tier1AttestationReport,
    Tier1ExternalProviderAttestationOptions, DEFAULT_CHUNK_SIZE, SEALED_STORE_MANIFEST,
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
use mayhem_proto::{
    receipt_signing_bytes, session_accept_signing_bytes, session_frame_head,
    CatalogEnclaveIdentity, ReceiptAck, ReceiptBody, ReceiptUsage, SESSION_RECEIPT_SCHEMA_VERSION,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tokio::time::sleep;

const DEFAULT_GATEWAY_URL: &str = "http://127.0.0.1:11435";
const DEFAULT_PAYGATE_URL: &str = "http://127.0.0.1:11436";
const TNK_E18: u128 = 1_000_000_000_000_000_000;
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
    /// Admin-only canonical contract control-plane commands.
    Admin {
        #[command(subcommand)]
        command: Box<AdminCommands>,
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
    Start(Box<ProviderStartArgs>),
    /// Register if needed, then join an existing admin-created enclave and canonical rooms.
    Join(ProviderJoinArgs),
    /// Leave canonical rooms, then leave an existing admin-created enclave.
    Leave(ProviderLeaveArgs),
    /// Leave every active canonical room and enclave for this provider wallet.
    Stop(ProviderStopArgs),
    /// Join or leave canonical admin-created rooms for a served enclave.
    Rooms {
        #[command(subcommand)]
        command: ProviderRoomsCommands,
    },
}

#[derive(Debug, Subcommand)]
enum ProviderRoomsCommands {
    /// Join one canonical room with an already-served enclave.
    Join(ProviderRoomJoinArgs),
    /// Leave one canonical room.
    Leave(ProviderRoomLeaveArgs),
}

#[derive(Debug, Subcommand)]
enum AdminCommands {
    /// Set the active rules version and hash.
    SetRules(AdminSetRulesArgs),
    /// Schedule admin-owned contract parameters.
    SetParams(AdminSetParamsArgs),
    /// Set the admin model reference price used to bound enclave prices.
    SetModelRef(AdminSetModelRefArgs),
    /// Register an admin-created and attested canonical enclave.
    RegisterEnclave(AdminRegisterEnclaveArgs),
    /// Retire an admin-created enclave.
    RetireEnclave(AdminRetireEnclaveArgs),
    /// Open a canonical admin room for a model.
    OpenRoom(AdminOpenRoomArgs),
    /// Close a canonical admin room.
    CloseRoom(AdminCloseRoomArgs),
    /// Set a forward-facing admin price schedule for an enclave.
    SetPrice(AdminSetPriceArgs),
    /// Set an admin-approved provider payout target.
    SetProviderPayout(AdminSetProviderPayoutArgs),
    /// Ban a provider and tombstone its active serving rows.
    BanProvider(AdminBanProviderArgs),
    /// Accredit an auditor key for probe submission.
    AuditorRegister(AdminAuditorRegisterArgs),
    /// Post a fresh TNK/USD oracle rate for payment and payout conversions.
    RateOracle(AdminRateOracleArgs),
    /// Confirm a memo-bound TNK deposit into the canonical credit ledger.
    TnkDeposit(AdminTnkDepositArgs),
    /// Confirm a fiat checkout deposit into the canonical credit ledger.
    FiatDeposit(AdminFiatDepositArgs),
    /// Record a fiat chargeback clawback and account freeze.
    FiatChargeback(AdminFiatChargebackArgs),
    /// Confirm an executed provider payout or router fee sweep.
    PayoutConfirm(AdminPayoutConfirmArgs),
    /// Anchor recomputed epoch roots permissionlessly.
    EpochCommit(AdminEpochCommitArgs),
    /// Apply admin-verified epoch debits/earnings and ev/* roots.
    EpochApply(AdminEpochApplyArgs),
}

#[derive(Debug, Subcommand)]
enum PayCommands {
    /// Prepare a memo-bound TNK treasury deposit.
    Tnk(PayTnkArgs),
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
    /// Audit per-backend canary fingerprint coverage for launch artifacts.
    CanaryMatrix(CatalogCanaryMatrixArgs),
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

    /// Seconds to wait for provider s.accept/s.reject after opening a direct session.
    #[arg(long)]
    session_open_timeout_seconds: Option<u64>,

    /// Seconds to wait between provider session frames after s.accept.
    #[arg(long)]
    session_frame_timeout_seconds: Option<u64>,

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

    /// Admin-set fee split in basis points for independent root recompute.
    #[arg(long)]
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
    /// Fiat amount to buy, for example 10 or 10.25.
    #[arg(long)]
    amount: String,

    /// Fiat checkout currency for Stripe. Defaults to USD.
    #[arg(long, default_value = "usd")]
    currency: String,

    /// Hosted checkout locale. Stripe beta supports English checkout.
    #[arg(long, default_value = "en")]
    locale: String,

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
struct PayTnkArgs {
    /// USD amount of Mayhem credit to target, for example 10 or 10.25.
    #[arg(long)]
    amount: String,

    /// Network treasury address that receives the TNK transfer.
    #[arg(long)]
    treasury_address: Option<String>,

    /// Override TNK/USD rate in integer micro-USD per 1 TNK. Defaults to contract rate/latest.
    #[arg(long)]
    tnk_usd_e6: Option<u64>,

    /// 32-byte hex nonce used with the wallet public key to derive the memo hash.
    #[arg(long)]
    nonce: Option<String>,

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

    /// Sign and submit the depositTnk intent through peer RPC.
    #[arg(long)]
    submit_intent: bool,

    /// Broadcast the matching MSB transfer from this local wallet. Requires --submit-intent.
    #[arg(long)]
    submit_transfer: bool,

    /// MSB network for --submit-transfer. Defaults to testnet1 for testtrac1... treasury addresses and mainnet for trac1...
    #[arg(long)]
    msb_network: Option<String>,

    /// Maximum seconds to wait for MSB account sync and validator connection when --submit-transfer is used.
    #[arg(long, default_value_t = 180)]
    msb_transfer_timeout_seconds: u64,

    /// Submit with peer RPC sim mode when --submit-intent is used.
    #[arg(long)]
    sim: bool,

    /// Wait for the contract ledger balance to reflect the TNK credit.
    #[arg(long)]
    wait: bool,

    /// Maximum seconds to wait for ledger credit when --wait is used.
    #[arg(long, default_value_t = 900)]
    timeout_seconds: u64,

    /// Poll interval in milliseconds while waiting for ledger credit.
    #[arg(long, default_value_t = 2_000)]
    poll_interval_ms: u64,

    /// Print a machine-readable TNK payment report.
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

    /// Skip the direct gateway tool-call smoke before opencode.
    #[arg(long)]
    skip_direct_tool_smoke: bool,

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

    /// Exit nonzero when the calibrated fingerprint is absent or differs from the catalog.
    #[arg(long)]
    require_match: bool,

    /// Print a machine-readable calibration report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct CatalogCanaryMatrixArgs {
    /// Path to catalog/models.json. Defaults to the repo catalog.
    #[arg(long, value_name = "PATH")]
    catalog_path: Option<PathBuf>,

    /// Directory containing canary set JSON files.
    #[arg(long, value_name = "PATH")]
    canaries_dir: Option<PathBuf>,

    /// Include dev-tier models in addition to launch-tier models.
    #[arg(long)]
    include_dev: bool,

    /// Print a machine-readable coverage report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
enum AdminPayoutMethod {
    Tnk,
    Stripe,
    Coinbase,
}

impl AdminPayoutMethod {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tnk => "tnk",
            Self::Stripe => "stripe",
            Self::Coinbase => "coinbase",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
enum AdminFiatRail {
    Stripe,
    Coinbase,
}

impl AdminFiatRail {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stripe => "stripe",
            Self::Coinbase => "coinbase",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
enum AdminRateSource {
    GateSpot,
    MexcSpot,
}

impl AdminRateSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::GateSpot => "gate-spot",
            Self::MexcSpot => "mexc-spot",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
enum AdminPayoutConfirmKind {
    Provider,
    FeeSweep,
}

impl AdminPayoutConfirmKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::FeeSweep => "fee_sweep",
        }
    }
}

#[derive(Debug, Parser)]
struct AdminTxArgs {
    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Peer JSON-RPC base URL, including /v1. Defaults to config.toml or local dev-net.
    #[arg(long)]
    rpc_url: Option<String>,

    /// Intercom peer store name under <home>/stores when config.toml has no identity store.
    #[arg(long, default_value = "main")]
    peer_store_name: String,

    /// Password for the encrypted admin keypair.json. Empty by default.
    #[arg(long)]
    wallet_password: Option<String>,

    /// Sign and submit the command through peer RPC. Otherwise only print copy/paste commands.
    #[arg(long)]
    submit: bool,

    /// Submit with peer RPC sim mode when --submit is used.
    #[arg(long)]
    sim: bool,

    /// Print a machine-readable report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct AdminSetRulesArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    /// Monotonically increasing rules version.
    #[arg(long)]
    ver: u64,

    /// Hash of the rules document.
    #[arg(long)]
    hash: String,
}

#[derive(Debug, Parser)]
struct AdminSetParamsArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    /// Contract timestamp/slot when the change is submitted.
    #[arg(long)]
    submitted_at: u64,

    /// Contract timestamp/slot when the change becomes active. Must be at least 24h later.
    #[arg(long)]
    effective_at: u64,

    /// JSON object containing parameter keys and values.
    #[arg(long)]
    values_json: Option<String>,

    /// Path to a JSON file containing parameter keys and values.
    #[arg(long, value_name = "PATH")]
    values_file: Option<PathBuf>,
}

#[derive(Debug, Parser)]
struct AdminSetModelRefArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    /// Catalog model id.
    #[arg(long)]
    model: String,

    /// Reference input price in integer micro-USD per 1k tokens.
    #[arg(long)]
    in_per_1k_mu: u64,

    /// Reference output price in integer micro-USD per 1k tokens.
    #[arg(long)]
    out_per_1k_mu: u64,

    /// Optional source hash for the catalog/price reference evidence.
    #[arg(long)]
    source_hash: Option<String>,
}

#[derive(Debug, Parser)]
struct AdminRegisterEnclaveArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    #[arg(long)]
    enclave_id: String,

    #[arg(long)]
    model: String,

    #[arg(long)]
    backend: String,

    #[arg(long)]
    artifact_root: String,

    #[arg(long, default_value = "blake3_merkle_v1")]
    artifact_root_kind: String,

    /// Hugging Face artifact repo in namespace/name form.
    #[arg(long)]
    artifact_repo: String,

    /// Hugging Face artifact git commit revision.
    #[arg(long)]
    artifact_revision: String,

    /// Hugging Face artifact path inside the repo.
    #[arg(long)]
    artifact_path: String,

    /// Optional SHA-256 of the admin-approved artifact file.
    #[arg(long)]
    source_sha256: Option<String>,

    #[arg(long)]
    manifest_hash: String,

    #[arg(long)]
    binary_hash: String,

    #[arg(long, default_value_t = 1)]
    att_tier: u8,

    /// JSON object for enclave caps, e.g. '{"tools":true,"json":true,"ctx":8192}'.
    #[arg(long)]
    caps_json: Option<String>,

    /// Path to a JSON file containing enclave caps.
    #[arg(long, value_name = "PATH")]
    caps_file: Option<PathBuf>,
}

#[derive(Debug, Parser)]
struct AdminRetireEnclaveArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    #[arg(long)]
    enclave_id: String,
}

#[derive(Debug, Parser)]
struct AdminOpenRoomArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    #[arg(long)]
    enclave_id: String,

    /// Optional catalog model id. If provided, it must match the admin enclave.
    #[arg(long)]
    model: Option<String>,

    /// Room nonce used with the enclave id to derive a stable room id.
    #[arg(long)]
    nonce: String,

    #[arg(long)]
    label: String,

    /// JSON object for room policy. Defaults to {}.
    #[arg(long)]
    policy_json: Option<String>,

    /// Path to a JSON file containing room policy.
    #[arg(long, value_name = "PATH")]
    policy_file: Option<PathBuf>,
}

#[derive(Debug, Parser)]
struct AdminCloseRoomArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    #[arg(long)]
    room_id: String,
}

#[derive(Debug, Parser)]
struct AdminSetPriceArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    #[arg(long)]
    enclave_id: String,

    /// Input price in integer micro-USD per 1k tokens.
    #[arg(long)]
    in_per_1k_mu: u64,

    /// Output price in integer micro-USD per 1k tokens.
    #[arg(long)]
    out_per_1k_mu: u64,

    /// Fixed per-request price in integer micro-USD.
    #[arg(long, default_value_t = 0)]
    per_req_mu: u64,

    /// Minimum session price in integer micro-USD.
    #[arg(long, default_value_t = 0)]
    min_session_mu: u64,

    /// Contract timestamp/slot at which the new price becomes active.
    #[arg(long)]
    effective_at: u64,
}

#[derive(Debug, Parser)]
struct AdminSetProviderPayoutArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    #[arg(long)]
    provider: String,

    #[arg(long, value_enum)]
    payout_method: AdminPayoutMethod,

    #[arg(long)]
    payout_addr: String,

    /// Provider payout currency for fiat payout rails.
    #[arg(long)]
    payout_currency: Option<String>,
}

#[derive(Debug, Parser)]
struct AdminBanProviderArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    #[arg(long)]
    provider: String,

    /// Precomputed reason hash.
    #[arg(long)]
    reason_hash: Option<String>,

    /// Plaintext reason to hash locally with BLAKE3.
    #[arg(long)]
    reason: Option<String>,
}

#[derive(Debug, Parser)]
struct AdminAuditorRegisterArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    /// Auditor public key to accredit.
    #[arg(long)]
    auditor: String,

    /// Registration age timestamp used by the contract.
    #[arg(long, default_value_t = 0)]
    registered_at_seconds: u64,
}

#[derive(Debug, Parser)]
struct AdminRateOracleArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    /// TNK/USD rate in integer micro-USD per 1 TNK.
    #[arg(long)]
    tnk_usd_e6: u64,

    /// Oracle source label accepted by the contract.
    #[arg(long, value_enum)]
    source: AdminRateSource,

    /// Source observation timestamp in Unix seconds.
    #[arg(long)]
    ts: u64,
}

#[derive(Debug, Parser)]
struct AdminTnkDepositArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    #[arg(long)]
    memo_hash: String,

    /// Deposited TNK amount as 18-decimal integer string.
    #[arg(long)]
    tnk_e18: String,

    #[arg(long)]
    msb_tx_hash: String,

    #[arg(long)]
    epoch: u64,

    #[arg(long)]
    at: u64,
}

#[derive(Debug, Parser)]
struct AdminFiatDepositArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    #[arg(long, value_enum)]
    rail: AdminFiatRail,

    /// User public key to credit.
    #[arg(long)]
    who: String,

    /// Credited amount in integer micro-USD.
    #[arg(long)]
    mu: u64,

    /// Hash of the external checkout/payment reference.
    #[arg(long)]
    ext_ref_hash: String,

    /// Fiat checkout currency recorded as ledger evidence.
    #[arg(long)]
    fiat_currency: String,

    /// Fiat amount in minor units recorded as ledger evidence.
    #[arg(long)]
    fiat_amount_minor: u64,

    #[arg(long)]
    epoch: u64,

    #[arg(long)]
    at: u64,
}

#[derive(Debug, Parser)]
struct AdminFiatChargebackArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    #[arg(long, value_enum)]
    rail: AdminFiatRail,

    /// User public key to claw back and freeze.
    #[arg(long)]
    who: String,

    /// Disputed amount in integer micro-USD.
    #[arg(long)]
    mu: u64,

    /// Hash of the original external checkout/payment reference.
    #[arg(long)]
    ext_ref_hash: String,

    /// Hash of the external dispute/chargeback reference.
    #[arg(long)]
    dispute_ref_hash: String,

    /// Fiat dispute currency recorded as ledger evidence.
    #[arg(long)]
    fiat_currency: String,

    /// Fiat dispute amount in minor units recorded as ledger evidence.
    #[arg(long)]
    fiat_amount_minor: u64,

    #[arg(long)]
    epoch: u64,

    #[arg(long)]
    at: u64,
}

#[derive(Debug, Parser)]
struct AdminPayoutConfirmArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    /// Payout confirmation kind.
    #[arg(long, value_enum, default_value_t = AdminPayoutConfirmKind::Provider)]
    kind: AdminPayoutConfirmKind,

    /// Executed payout rail.
    #[arg(long, value_enum, default_value_t = AdminPayoutMethod::Tnk)]
    rail: AdminPayoutMethod,

    #[arg(long)]
    epoch: u64,

    /// Provider public key, or treasury for fee sweeps.
    #[arg(long)]
    who: String,

    /// Confirmed amount in integer micro-USD.
    #[arg(long)]
    mu: u64,

    /// TNK payout amount as 18-decimal integer string. Required for TNK payouts.
    #[arg(long)]
    tnk_e18: Option<String>,

    /// MSB transfer hash. Required for TNK payouts.
    #[arg(long)]
    msb_tx_hash: Option<String>,

    /// External fiat transfer reference. Required for Stripe/Coinbase payouts.
    #[arg(long)]
    external_ref: Option<String>,

    /// Fiat payout currency. Required for Stripe/Coinbase payouts.
    #[arg(long)]
    fiat_currency: Option<String>,

    /// Fiat payout amount in minor units. Required for Stripe/Coinbase payouts.
    #[arg(long)]
    fiat_amount_minor: Option<u64>,

    #[arg(long)]
    at: u64,
}

#[derive(Debug, Parser)]
struct AdminEpochCommitArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    /// Epoch number. Defaults to recomputed_file.epoch when omitted.
    #[arg(long)]
    epoch: Option<u64>,

    #[arg(long)]
    at: u64,

    /// Output from intercom/scripts/recompute-epoch-roots.mjs.
    #[arg(long, value_name = "PATH")]
    recomputed_file: Option<PathBuf>,

    /// JSON object containing dep/use/earn/fee/pay roots.
    #[arg(long)]
    roots_json: Option<String>,

    /// Path to a JSON object containing dep/use/earn/fee/pay roots.
    #[arg(long, value_name = "PATH")]
    roots_file: Option<PathBuf>,

    /// JSON object containing recomputed epoch totals.
    #[arg(long)]
    totals_json: Option<String>,

    /// Path to a JSON object containing recomputed epoch totals.
    #[arg(long, value_name = "PATH")]
    totals_file: Option<PathBuf>,
}

#[derive(Debug, Parser)]
struct AdminEpochApplyArgs {
    #[command(flatten)]
    tx: AdminTxArgs,

    /// Epoch number. Defaults to recomputed_file.epoch when omitted.
    #[arg(long)]
    epoch: Option<u64>,

    #[arg(long)]
    at: u64,

    /// Output from intercom/scripts/recompute-epoch-roots.mjs.
    #[arg(long, value_name = "PATH")]
    recomputed_file: Option<PathBuf>,

    /// JSON array of {user,mu} debits.
    #[arg(long)]
    debits_json: Option<String>,

    /// Path to a JSON array of {user,mu} debits.
    #[arg(long, value_name = "PATH")]
    debits_file: Option<PathBuf>,

    /// JSON array of {provider,gross_mu} earnings.
    #[arg(long)]
    earnings_json: Option<String>,

    /// Path to a JSON array of {provider,gross_mu} earnings.
    #[arg(long, value_name = "PATH")]
    earnings_file: Option<PathBuf>,

    /// JSON object containing dep/use/earn/fee/pay roots.
    #[arg(long)]
    roots_json: Option<String>,

    /// Path to a JSON object containing dep/use/earn/fee/pay roots.
    #[arg(long, value_name = "PATH")]
    roots_file: Option<PathBuf>,

    /// JSON object containing recomputed epoch totals.
    #[arg(long)]
    totals_json: Option<String>,

    /// Path to a JSON object containing recomputed epoch totals.
    #[arg(long, value_name = "PATH")]
    totals_file: Option<PathBuf>,
}

#[derive(Debug, Parser)]
struct ProviderTxArgs {
    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Peer JSON-RPC base URL, including /v1. Defaults to config.toml or the bridge default.
    #[arg(long)]
    rpc_url: Option<String>,

    /// Intercom peer store name under <home>/stores when config.toml has no identity store.
    #[arg(long, default_value = "main")]
    peer_store_name: String,

    /// Password for the encrypted provider keypair.json. Empty by default.
    #[arg(long)]
    wallet_password: Option<String>,

    /// Build and sign provider lifecycle feature records without appending them.
    #[arg(long)]
    sim: bool,

    /// Print a machine-readable report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct ProviderJoinArgs {
    #[command(flatten)]
    tx: ProviderTxArgs,

    /// Admin-created enclave id, or a model id that resolves to one active admin enclave.
    #[arg(long)]
    enclave: String,

    /// Canonical room ids to join, comma-separated, or auto for all open admin rooms for the model.
    #[arg(long, default_value = "auto")]
    rooms: String,
}

#[derive(Debug, Parser)]
struct ProviderLeaveArgs {
    #[command(flatten)]
    tx: ProviderTxArgs,

    /// Admin-created enclave id, or a model id from an active serving row.
    #[arg(long)]
    enclave: String,

    /// Canonical room ids to leave, comma-separated, or auto for all active rooms joined by this provider for the enclave.
    #[arg(long, default_value = "auto")]
    rooms: String,
}

#[derive(Debug, Parser)]
struct ProviderStopArgs {
    #[command(flatten)]
    tx: ProviderTxArgs,
}

#[derive(Debug, Parser)]
struct ProviderRoomJoinArgs {
    #[command(flatten)]
    tx: ProviderTxArgs,

    /// Canonical admin-created room id.
    #[arg(long)]
    room: String,

    /// Admin-created enclave id already served by this provider.
    #[arg(long)]
    enclave: String,
}

#[derive(Debug, Parser)]
struct ProviderRoomLeaveArgs {
    #[command(flatten)]
    tx: ProviderTxArgs,

    /// Canonical admin-created room id.
    #[arg(long)]
    room: String,

    /// Admin-created enclave id served in the room.
    #[arg(long)]
    enclave: String,
}

#[derive(Debug, Parser)]
struct ProviderStartArgs {
    /// Mayhem home directory. Defaults to MAYHEM_HOME or ~/.mayhem.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,

    /// Admin-created enclave id, or a model id that resolves to one active admin enclave.
    /// If omitted, the first feasible admin enclave is used.
    #[arg(long)]
    enclave: Option<String>,

    /// Canonical room ids to join, comma-separated, or auto for all open admin rooms for the model.
    #[arg(long, default_value = "auto")]
    rooms: String,

    /// Peer JSON-RPC base URL, including /v1. Defaults to config.toml or the bridge default.
    #[arg(long)]
    rpc_url: Option<String>,

    /// Peer JSON-RPC base URL used for live provider session rechecks.
    /// Defaults to --rpc-url.
    #[arg(long)]
    session_rpc_url: Option<String>,

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

    /// Build and sign provider registration/join feature records without appending them.
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

#[derive(Debug, Deserialize, Serialize)]
struct MsbTransferOutput {
    ok: bool,
    network: String,
    from: String,
    to: String,
    amount: String,
    tx_hash: String,
    before_balance: String,
    validator_connections: u64,
}

#[derive(Debug, Deserialize)]
struct SignOutput {
    signature: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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
    feature: Option<Value>,
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
    tnk_treasury_address: Option<String>,
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
            CatalogCommands::CanaryMatrix(args) => catalog_canary_matrix(args),
        },
        Commands::Provider { command } => match *command {
            ProviderCommands::Start(args) => provider_start(*args).await,
            ProviderCommands::Join(args) => provider_join(args).await,
            ProviderCommands::Leave(args) => provider_leave(args).await,
            ProviderCommands::Stop(args) => provider_stop(args).await,
            ProviderCommands::Rooms { command } => match command {
                ProviderRoomsCommands::Join(args) => provider_room_join(args).await,
                ProviderRoomsCommands::Leave(args) => provider_room_leave(args).await,
            },
        },
        Commands::Admin { command } => admin(*command).await,
        Commands::Use(args) => use_gateway(args).await,
        Commands::Models(args) => models(args).await,
        Commands::Pay { command } => match command {
            PayCommands::Tnk(args) => pay_tnk(args).await,
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
            feature: None,
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
            feature: None,
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
    if args.require_match {
        ensure_calibration_matches_catalog(&report)?;
    }
    Ok(())
}

fn ensure_calibration_matches_catalog(report: &CatalogCanaryCalibrationReport) -> Result<()> {
    match report.matches_existing_catalog {
        Some(true) => Ok(()),
        Some(false) => bail!(
            "calibrated fingerprint {} for model {} artifact {} does not match catalog fingerprint {}",
            report.catalog_fingerprint,
            report.model_id,
            report.artifact,
            report
                .existing_catalog_fingerprint
                .as_deref()
                .unwrap_or("<missing>")
        ),
        None => bail!(
            "catalog has no fingerprint for model {} artifact {}",
            report.model_id,
            report.artifact
        ),
    }
}

fn catalog_canary_matrix(args: CatalogCanaryMatrixArgs) -> Result<()> {
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
    let catalog_doc = catalog::load_document(&catalog_path)?;
    let report =
        catalog_canary_matrix_report(&catalog_doc, catalog_path, canaries_dir, !args.include_dev);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Canary calibration matrix.");
        println!("Catalog: {}", report.catalog_path.display());
        println!("Canaries: {}", report.canaries_dir.display());
        println!(
            "Scope: {}",
            if report.launch_only {
                "launch models"
            } else {
                "launch + dev models"
            }
        );
        println!(
            "Coverage: {} models, {} artifacts, ok={}",
            report.model_count, report.artifact_count, report.ok
        );
        for entry in &report.entries {
            println!(
                "- {} / {} ({}) [{}]: {}",
                entry.model_id,
                entry.artifact,
                entry.engine,
                entry.canary_set,
                entry.calibration_status
            );
            for error in &entry.errors {
                println!("  error: {error}");
            }
        }
    }

    if !report.ok {
        bail!(
            "canary calibration matrix has {} error(s)",
            report.errors.len()
        );
    }
    Ok(())
}

fn catalog_canary_matrix_report(
    catalog_doc: &catalog::CatalogDocument,
    catalog_path: PathBuf,
    canaries_dir: PathBuf,
    launch_only: bool,
) -> CatalogCanaryMatrixReport {
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    for model in &catalog_doc.models {
        if launch_only && model.tier != "launch" {
            continue;
        }
        let canary_check = canary_set_matrix_check(&canaries_dir, &model.canary.set_id);
        for (artifact_name, artifact) in &model.artifacts {
            let mut entry_errors = Vec::new();
            if let Err(err) = &canary_check {
                entry_errors.push(err.clone());
            }
            let fingerprint = model.canary.fingerprints.get(artifact_name).cloned();
            match fingerprint.as_deref() {
                Some(value) if is_hex_len(value, 64) => {}
                Some(_) => entry_errors.push(format!(
                    "canary fingerprint for {artifact_name} must be 32-byte hex"
                )),
                None => entry_errors.push(format!(
                    "canary fingerprint missing artifact {artifact_name}"
                )),
            }
            let calibration_status = match artifact.engine.as_str() {
                "llama.cpp" | "mlx" => "local-calibration-supported",
                "trt-llm" => "hardware-gated-calibration",
                other => {
                    entry_errors.push(format!("unsupported calibration engine {other}"));
                    "unsupported-calibration-engine"
                }
            }
            .to_owned();
            let ok = entry_errors.is_empty();
            let entry = CatalogCanaryMatrixEntry {
                model_id: model.model_id.clone(),
                tier: model.tier.clone(),
                artifact: artifact_name.clone(),
                engine: artifact.engine.clone(),
                canary_set: model.canary.set_id.clone(),
                prompt_count: canary_check.as_ref().ok().copied(),
                fingerprint,
                calibration_status,
                ok,
                errors: entry_errors,
            };
            if !entry.ok {
                errors.extend(
                    entry
                        .errors
                        .iter()
                        .map(|error| format!("{} {}: {error}", entry.model_id, entry.artifact)),
                );
            }
            entries.push(entry);
        }
    }
    if entries.is_empty() {
        errors.push(if launch_only {
            "catalog has no launch model artifacts to audit".to_owned()
        } else {
            "catalog has no model artifacts to audit".to_owned()
        });
    }
    let model_count = entries
        .iter()
        .map(|entry| entry.model_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    CatalogCanaryMatrixReport {
        catalog_path,
        canaries_dir,
        launch_only,
        model_count,
        artifact_count: entries.len(),
        ok: errors.is_empty(),
        entries,
        errors,
    }
}

fn canary_set_matrix_check(
    canaries_dir: &Path,
    set_id: &str,
) -> std::result::Result<usize, String> {
    load_canary_prompts(Some(canaries_dir), set_id, None)
        .map_err(|err| err.to_string())
        .and_then(|prompts| {
            for prompt in &prompts {
                if prompt.temperature.unwrap_or(0.0).abs() > f64::EPSILON {
                    return Err(format!(
                        "canary prompt {} in {set_id} must use temperature 0",
                        prompt.id
                    ));
                }
            }
            Ok(prompts.len())
        })
}

fn catalog_calibration_backend(
    artifact: &catalog::CatalogArtifact,
    artifact_path: &Path,
    args: &CatalogCalibrateCanaryArgs,
) -> Result<Box<dyn EngineBackend>> {
    let mut config = match artifact.engine.as_str() {
        "llama.cpp" => LoadConfig::gguf(artifact_path),
        "mlx" => LoadConfig::mlx_safetensors(artifact_path),
        "trt-llm" => LoadConfig::trt_llm_checkpoint(artifact_path),
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
        "trt-llm" => {
            config.trt_engine_dir = Some(trt_engine_cache_dir(artifact_path, "calibration"));
            config.trt_kv_cache_dtype = trt_kv_cache_dtype_for_artifact("calibration", artifact);
            let mut backend =
                mayhem_engine::TrtLlmBackend::new().context("initializing TensorRT-LLM backend")?;
            backend
                .load(config)
                .context("loading TensorRT-LLM canary calibration artifact")?;
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

async fn admin(command: AdminCommands) -> Result<()> {
    let tx_args = admin_tx_args(&command);
    let (tx_type, value) = admin_command_payload(&command)?;
    run_admin_command(tx_args, tx_type, value).await
}

fn admin_tx_args(command: &AdminCommands) -> &AdminTxArgs {
    match command {
        AdminCommands::SetRules(args) => &args.tx,
        AdminCommands::SetParams(args) => &args.tx,
        AdminCommands::SetModelRef(args) => &args.tx,
        AdminCommands::RegisterEnclave(args) => &args.tx,
        AdminCommands::RetireEnclave(args) => &args.tx,
        AdminCommands::OpenRoom(args) => &args.tx,
        AdminCommands::CloseRoom(args) => &args.tx,
        AdminCommands::SetPrice(args) => &args.tx,
        AdminCommands::SetProviderPayout(args) => &args.tx,
        AdminCommands::BanProvider(args) => &args.tx,
        AdminCommands::AuditorRegister(args) => &args.tx,
        AdminCommands::RateOracle(args) => &args.tx,
        AdminCommands::TnkDeposit(args) => &args.tx,
        AdminCommands::FiatDeposit(args) => &args.tx,
        AdminCommands::FiatChargeback(args) => &args.tx,
        AdminCommands::PayoutConfirm(args) => &args.tx,
        AdminCommands::EpochCommit(args) => &args.tx,
        AdminCommands::EpochApply(args) => &args.tx,
    }
}

fn admin_command_payload(command: &AdminCommands) -> Result<(&'static str, Value)> {
    match command {
        AdminCommands::SetRules(args) => Ok(("setRules", admin_set_rules_payload(args))),
        AdminCommands::SetParams(args) => Ok(("setParams", admin_set_params_payload(args)?)),
        AdminCommands::SetModelRef(args) => Ok(("setModelRef", admin_set_model_ref_payload(args))),
        AdminCommands::RegisterEnclave(args) => {
            Ok(("registerEnclave", admin_register_enclave_payload(args)?))
        }
        AdminCommands::RetireEnclave(args) => Ok((
            "retireEnclave",
            json!({
                "op": "retire_enclave",
                "enclave_id": &args.enclave_id,
            }),
        )),
        AdminCommands::OpenRoom(args) => Ok(("openRoom", admin_open_room_payload(args)?)),
        AdminCommands::CloseRoom(args) => Ok((
            "closeRoom",
            json!({
                "op": "close_room",
                "room_id": &args.room_id,
            }),
        )),
        AdminCommands::SetPrice(args) => Ok(("setPrice", admin_set_price_payload(args))),
        AdminCommands::SetProviderPayout(args) => Ok((
            "setProviderPayout",
            admin_set_provider_payout_payload(args)?,
        )),
        AdminCommands::BanProvider(args) => Ok(("banProvider", admin_ban_provider_payload(args)?)),
        AdminCommands::AuditorRegister(args) => {
            Ok(("auditorRegister", admin_auditor_register_payload(args)))
        }
        AdminCommands::RateOracle(args) => Ok(("rateOracle", admin_rate_oracle_payload(args))),
        AdminCommands::TnkDeposit(args) => Ok(("tnkDeposit", admin_tnk_deposit_payload(args))),
        AdminCommands::FiatDeposit(args) => Ok(("fiatDeposit", admin_fiat_deposit_payload(args)?)),
        AdminCommands::FiatChargeback(args) => {
            Ok(("fiatChargeback", admin_fiat_chargeback_payload(args)?))
        }
        AdminCommands::PayoutConfirm(args) => {
            Ok(("payoutConfirm", admin_payout_confirm_payload(args)?))
        }
        AdminCommands::EpochCommit(args) => Ok(("epochCommit", admin_epoch_commit_payload(args)?)),
        AdminCommands::EpochApply(args) => Ok(("epochApply", admin_epoch_apply_payload(args)?)),
    }
}

fn admin_set_rules_payload(args: &AdminSetRulesArgs) -> Value {
    json!({
        "op": "set_rules",
        "ver": args.ver,
        "hash": &args.hash,
    })
}

fn admin_set_params_payload(args: &AdminSetParamsArgs) -> Result<Value> {
    let values = json_arg_or_file_object(
        args.values_json.as_deref(),
        args.values_file.as_ref(),
        None,
        "contract params",
    )?;
    Ok(json!({
        "op": "set_params",
        "submitted_at": args.submitted_at,
        "effective_at": args.effective_at,
        "values": values,
    }))
}

fn admin_set_model_ref_payload(args: &AdminSetModelRefArgs) -> Value {
    let mut payload = json!({
        "op": "set_model_ref",
        "model_id": &args.model,
        "price_ref_mu": {
            "in_per_1k": args.in_per_1k_mu,
            "out_per_1k": args.out_per_1k_mu,
        },
    });
    if let Some(source_hash) = &args.source_hash {
        payload["source_hash"] = json!(source_hash);
    }
    payload
}

fn admin_register_enclave_payload(args: &AdminRegisterEnclaveArgs) -> Result<Value> {
    let caps = json_arg_or_file_object(
        args.caps_json.as_deref(),
        args.caps_file.as_ref(),
        None,
        "enclave caps",
    )?;
    let mut payload = json!({
        "op": "register_enclave",
        "enclave_id": &args.enclave_id,
        "model_id": &args.model,
        "backend": &args.backend,
        "artifact_root": &args.artifact_root,
        "artifact_root_kind": &args.artifact_root_kind,
        "artifact_source": {
            "kind": "huggingface",
            "repo": &args.artifact_repo,
            "revision": &args.artifact_revision,
            "path": &args.artifact_path,
        },
        "manifest_hash": &args.manifest_hash,
        "att_tier": args.att_tier,
        "binary_hash": &args.binary_hash,
        "caps": caps,
    });
    if let Some(source_sha256) = &args.source_sha256 {
        payload["source_sha256"] = json!(source_sha256);
    }
    Ok(payload)
}

fn admin_open_room_payload(args: &AdminOpenRoomArgs) -> Result<Value> {
    let policy = json_arg_or_file_object(
        args.policy_json.as_deref(),
        args.policy_file.as_ref(),
        Some(json!({})),
        "room policy",
    )?;
    let mut payload = json!({
        "op": "open_room",
        "enclave_id": &args.enclave_id,
        "nonce": &args.nonce,
        "label": &args.label,
        "policy": policy,
    });
    if let Some(model) = &args.model {
        payload["model_id"] = json!(model);
    }
    Ok(payload)
}

fn admin_set_price_payload(args: &AdminSetPriceArgs) -> Value {
    json!({
        "op": "set_price",
        "enclave_id": &args.enclave_id,
        "in_per_1k_mu": args.in_per_1k_mu,
        "out_per_1k_mu": args.out_per_1k_mu,
        "per_req_mu": args.per_req_mu,
        "min_session_mu": args.min_session_mu,
        "effective_at": args.effective_at,
    })
}

fn normalize_admin_fiat_currency(value: &str) -> Result<String> {
    let currency = value.trim().to_ascii_lowercase();
    match currency.as_str() {
        "usd" | "eur" => Ok(currency),
        _ => bail!("fiat currency must be usd or eur"),
    }
}

fn admin_set_provider_payout_payload(args: &AdminSetProviderPayoutArgs) -> Result<Value> {
    let mut payload = json!({
        "op": "set_provider_payout",
        "provider": &args.provider,
        "payout_method": args.payout_method.as_str(),
        "payout_addr": &args.payout_addr,
    });
    match args.payout_method {
        AdminPayoutMethod::Tnk => {
            if args.payout_currency.is_some() {
                bail!("TNK payout targets must not include --payout-currency");
            }
        }
        AdminPayoutMethod::Stripe | AdminPayoutMethod::Coinbase => {
            let payout_currency = args
                .payout_currency
                .as_deref()
                .context("fiat payout targets require --payout-currency")?;
            payload["payout_currency"] = json!(normalize_admin_fiat_currency(payout_currency)?);
        }
    }
    Ok(payload)
}

fn admin_ban_provider_payload(args: &AdminBanProviderArgs) -> Result<Value> {
    if args.reason_hash.is_some() && args.reason.is_some() {
        bail!("pass only one of --reason-hash or --reason");
    }
    let reason_hash = args.reason_hash.clone().or_else(|| {
        args.reason
            .as_ref()
            .map(|reason| blake3::hash(reason.as_bytes()).to_hex().to_string())
    });
    let mut payload = json!({
        "op": "ban_provider",
        "provider": &args.provider,
    });
    if let Some(reason_hash) = reason_hash {
        payload["reason_hash"] = json!(reason_hash);
    }
    Ok(payload)
}

fn admin_auditor_register_payload(args: &AdminAuditorRegisterArgs) -> Value {
    json!({
        "op": "auditor_register",
        "auditor": &args.auditor,
        "registered_at_seconds": args.registered_at_seconds,
    })
}

fn admin_rate_oracle_payload(args: &AdminRateOracleArgs) -> Value {
    json!({
        "op": "rate_oracle",
        "tnk_usd_e6": args.tnk_usd_e6,
        "source": args.source.as_str(),
        "ts": args.ts,
    })
}

fn admin_tnk_deposit_payload(args: &AdminTnkDepositArgs) -> Value {
    json!({
        "op": "tnk_deposit",
        "memo_hash": &args.memo_hash,
        "tnk_e18": &args.tnk_e18,
        "msb_tx_hash": &args.msb_tx_hash,
        "epoch": args.epoch,
        "at": args.at,
    })
}

fn admin_fiat_deposit_payload(args: &AdminFiatDepositArgs) -> Result<Value> {
    if args.fiat_amount_minor == 0 {
        bail!("fiat deposits require positive --fiat-amount-minor");
    }
    Ok(json!({
        "op": "fiat_deposit",
        "rail": args.rail.as_str(),
        "who": &args.who,
        "mu": args.mu,
        "ext_ref_hash": &args.ext_ref_hash,
        "fiat_currency": normalize_admin_fiat_currency(&args.fiat_currency)?,
        "fiat_amount_minor": args.fiat_amount_minor,
        "epoch": args.epoch,
        "at": args.at,
    }))
}

fn admin_fiat_chargeback_payload(args: &AdminFiatChargebackArgs) -> Result<Value> {
    if args.fiat_amount_minor == 0 {
        bail!("fiat chargebacks require positive --fiat-amount-minor");
    }
    Ok(json!({
        "op": "fiat_chargeback",
        "rail": args.rail.as_str(),
        "who": &args.who,
        "mu": args.mu,
        "ext_ref_hash": &args.ext_ref_hash,
        "dispute_ref_hash": &args.dispute_ref_hash,
        "fiat_currency": normalize_admin_fiat_currency(&args.fiat_currency)?,
        "fiat_amount_minor": args.fiat_amount_minor,
        "epoch": args.epoch,
        "at": args.at,
    }))
}

fn admin_payout_confirm_payload(args: &AdminPayoutConfirmArgs) -> Result<Value> {
    if args.kind == AdminPayoutConfirmKind::FeeSweep && args.rail != AdminPayoutMethod::Tnk {
        bail!("fee-sweep payout confirmations must use --rail tnk");
    }
    let mut payload = json!({
        "op": "payout_confirm",
        "epoch": args.epoch,
        "who": &args.who,
        "mu": args.mu,
        "at": args.at,
    });
    if args.kind != AdminPayoutConfirmKind::Provider {
        payload["kind"] = json!(args.kind.as_str());
    }
    match args.rail {
        AdminPayoutMethod::Tnk => {
            if args.external_ref.is_some()
                || args.fiat_currency.is_some()
                || args.fiat_amount_minor.is_some()
            {
                bail!(
                    "TNK payout confirmations must not include --external-ref, --fiat-currency, or --fiat-amount-minor"
                );
            }
            let tnk_e18 = args
                .tnk_e18
                .as_deref()
                .context("TNK payout confirmations require --tnk-e18")?;
            let msb_tx_hash = args
                .msb_tx_hash
                .as_deref()
                .context("TNK payout confirmations require --msb-tx-hash")?;
            payload["tnk_e18"] = json!(tnk_e18);
            payload["msb_tx_hash"] = json!(msb_tx_hash);
        }
        AdminPayoutMethod::Stripe | AdminPayoutMethod::Coinbase => {
            if args.kind == AdminPayoutConfirmKind::FeeSweep {
                bail!("fee-sweep payout confirmations must use --rail tnk");
            }
            if args.tnk_e18.is_some() || args.msb_tx_hash.is_some() {
                bail!("fiat payout confirmations must not include --tnk-e18 or --msb-tx-hash");
            }
            let external_ref = args
                .external_ref
                .as_deref()
                .context("fiat payout confirmations require --external-ref")?;
            let fiat_currency = args
                .fiat_currency
                .as_deref()
                .context("fiat payout confirmations require --fiat-currency")?;
            let fiat_amount_minor = args
                .fiat_amount_minor
                .context("fiat payout confirmations require --fiat-amount-minor")?;
            if fiat_amount_minor == 0 {
                bail!("fiat payout confirmations require positive --fiat-amount-minor");
            }
            payload["rail"] = json!(args.rail.as_str());
            payload["external_ref"] = json!(external_ref);
            payload["fiat_currency"] = json!(normalize_admin_fiat_currency(fiat_currency)?);
            payload["fiat_amount_minor"] = json!(fiat_amount_minor);
        }
    }
    Ok(payload)
}

fn admin_epoch_commit_payload(args: &AdminEpochCommitArgs) -> Result<Value> {
    let recomputed = read_optional_json_file(args.recomputed_file.as_ref(), "recomputed epoch")?;
    let epoch = epoch_arg_or_recomputed(args.epoch, recomputed.as_ref())?;
    let roots = json_arg_or_file_object(
        args.roots_json.as_deref(),
        args.roots_file.as_ref(),
        recomputed_field(recomputed.as_ref(), "roots"),
        "epoch roots",
    )?;
    let totals = json_arg_or_file_object(
        args.totals_json.as_deref(),
        args.totals_file.as_ref(),
        recomputed_field(recomputed.as_ref(), "totals"),
        "epoch totals",
    )?;

    Ok(json!({
        "op": "epoch_commit",
        "epoch": epoch,
        "at": args.at,
        "roots": roots,
        "totals": totals,
    }))
}

fn admin_epoch_apply_payload(args: &AdminEpochApplyArgs) -> Result<Value> {
    let recomputed = read_optional_json_file(args.recomputed_file.as_ref(), "recomputed epoch")?;
    let epoch = epoch_arg_or_recomputed(args.epoch, recomputed.as_ref())?;
    let debits = json_arg_or_file_array(
        args.debits_json.as_deref(),
        args.debits_file.as_ref(),
        recomputed_field(recomputed.as_ref(), "debits").or_else(|| Some(json!([]))),
        "epoch debits",
    )?;
    let earnings = json_arg_or_file_array(
        args.earnings_json.as_deref(),
        args.earnings_file.as_ref(),
        recomputed_field(recomputed.as_ref(), "earnings").or_else(|| Some(json!([]))),
        "epoch earnings",
    )?;
    let roots = json_arg_or_file_object(
        args.roots_json.as_deref(),
        args.roots_file.as_ref(),
        recomputed_field(recomputed.as_ref(), "roots"),
        "epoch roots",
    )?;
    let totals = json_arg_or_file_object(
        args.totals_json.as_deref(),
        args.totals_file.as_ref(),
        recomputed_field(recomputed.as_ref(), "totals"),
        "epoch totals",
    )?;

    Ok(json!({
        "op": "epoch_apply",
        "epoch": epoch,
        "at": args.at,
        "debits": debits,
        "earnings": earnings,
        "roots": roots,
        "totals": totals,
    }))
}

fn read_optional_json_file(path: Option<&PathBuf>, label: &str) -> Result<Option<Value>> {
    path.map(|path| {
        read_json_file(path)
            .with_context(|| format!("reading {label} JSON from {}", path.display()))
    })
    .transpose()
}

fn recomputed_field(recomputed: Option<&Value>, field: &str) -> Option<Value> {
    recomputed.and_then(|value| value.get(field).cloned())
}

fn epoch_arg_or_recomputed(epoch: Option<u64>, recomputed: Option<&Value>) -> Result<u64> {
    let epoch = epoch
        .or_else(|| recomputed.and_then(|value| value.get("epoch").and_then(Value::as_u64)))
        .context("--epoch is required when --recomputed-file does not contain epoch")?;
    if epoch == 0 {
        bail!("--epoch must be positive");
    }
    Ok(epoch)
}

fn json_arg_or_file_object(
    inline: Option<&str>,
    file: Option<&PathBuf>,
    default: Option<Value>,
    label: &str,
) -> Result<Value> {
    let value = match (inline, file) {
        (Some(_), Some(_)) => {
            bail!("pass only one inline {label} JSON value or {label} JSON file")
        }
        (Some(inline), None) => serde_json::from_str::<Value>(inline)
            .with_context(|| format!("parsing {label} JSON"))?,
        (None, Some(path)) => read_json_file(path)
            .with_context(|| format!("reading {label} JSON from {}", path.display()))?,
        (None, None) => default.with_context(|| format!("{label} JSON is required"))?,
    };
    if !value.is_object() {
        bail!("{label} JSON must be an object");
    }
    Ok(value)
}

fn json_arg_or_file_array(
    inline: Option<&str>,
    file: Option<&PathBuf>,
    default: Option<Value>,
    label: &str,
) -> Result<Value> {
    let value = match (inline, file) {
        (Some(_), Some(_)) => {
            bail!("pass only one inline {label} JSON value or {label} JSON file")
        }
        (Some(inline), None) => serde_json::from_str::<Value>(inline)
            .with_context(|| format!("parsing {label} JSON"))?,
        (None, Some(path)) => read_json_file(path)
            .with_context(|| format!("reading {label} JSON from {}", path.display()))?,
        (None, None) => default.with_context(|| format!("{label} JSON is required"))?,
    };
    if !value.is_array() {
        bail!("{label} JSON must be an array");
    }
    Ok(value)
}

async fn run_admin_command(args: &AdminTxArgs, tx_type: &'static str, value: Value) -> Result<()> {
    let compact_command = serde_json::to_string(&value)?;
    let copy_paste = format!(
        "/tx --command {} --sim 1",
        shell_single_quote(&compact_command)
    );
    let mut report = json!({
        "ok": true,
        "submitted": false,
        "tx_type": tx_type,
        "command": value,
        "copy_paste": {
            "intercom_sim": copy_paste,
        },
    });

    if args.submit {
        let home = args.home.clone().map(Ok).unwrap_or_else(default_home)?;
        let home = absolutize(home)?;
        let config = read_mayhem_config(&home)?;
        let rpc_url = resolve_cli_rpc_url(Some(&home), args.rpc_url.as_deref())?;
        let wallet_password = args.wallet_password.as_deref().unwrap_or("");
        let wallet = resolve_cli_wallet(
            &home,
            config.as_ref(),
            &args.peer_store_name,
            wallet_password,
        )
        .await?;
        let keypair_path = PathBuf::from(&wallet.keypair_path);
        let rpc = PeerRpcClient::new(&rpc_url)?;
        let submitted = submit_contract_command(
            &rpc,
            &keypair_path,
            wallet_password,
            &wallet,
            tx_type,
            report["command"].clone(),
            args.sim,
        )
        .await?;
        report["submitted"] = json!(true);
        report["sim"] = json!(args.sim);
        report["rpc_url"] = json!(rpc_url);
        report["wallet"] = json!({
            "public_key": wallet.public_key,
            "keypair_path": wallet.keypair_path,
        });
        report["tx"] = submitted;
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_admin_report(&report)?;
    }
    Ok(())
}

fn print_admin_report(report: &Value) -> Result<()> {
    println!("Admin contract command ready.");
    println!("Tx type: {}", report["tx_type"].as_str().unwrap_or(""));
    println!("Command JSON:");
    println!("{}", serde_json::to_string_pretty(&report["command"])?);
    println!("Copy/paste Intercom sim command:");
    println!(
        "{}",
        report["copy_paste"]["intercom_sim"].as_str().unwrap_or("")
    );
    if report["submitted"].as_bool() == Some(true) {
        println!("Submitted: true");
        if let Some(tx) = report["tx"]["tx"].as_str() {
            println!("Tx: {tx}");
        }
        if let Some(command_hash) = report["tx"]["command_hash"].as_str() {
            println!("Command hash: {command_hash}");
        }
    } else {
        println!("Submitted: false (pass --submit to sign and send through peer RPC)");
    }
    Ok(())
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[derive(Debug, Clone)]
struct PayTnkRate {
    tnk_usd_e6: u64,
    source: String,
    ts: Option<u64>,
}

async fn pay_tnk(args: PayTnkArgs) -> Result<()> {
    if args.poll_interval_ms == 0 {
        bail!("--poll-interval-ms must be positive");
    }
    if args.submit_transfer && !args.submit_intent {
        bail!("--submit-transfer requires --submit-intent so the memo-bound contract intent exists before TNK moves");
    }
    if args.submit_transfer && args.sim {
        bail!("--submit-transfer cannot be combined with --sim");
    }
    if args.submit_transfer && args.msb_transfer_timeout_seconds == 0 {
        bail!("--msb-transfer-timeout-seconds must be positive");
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
    let amount_mu = parse_usd_amount_to_mu(&args.amount)?;
    let treasury_address =
        resolve_cli_tnk_treasury_address(config.as_ref(), args.treasury_address.as_deref())?;
    let needs_rpc = args.tnk_usd_e6.is_none() || args.submit_intent || args.wait;
    let rpc_url = if needs_rpc {
        Some(resolve_cli_rpc_url(Some(&home), args.rpc_url.as_deref())?)
    } else {
        args.rpc_url.clone()
    };
    let rpc = match rpc_url.as_deref() {
        Some(url) if needs_rpc => Some(PeerRpcClient::new(url)?),
        _ => None,
    };
    let rate = resolve_tnk_rate(rpc.as_ref(), args.tnk_usd_e6).await?;
    let nonce = resolve_tnk_nonce(
        &wallet.public_key,
        amount_mu,
        &treasury_address,
        &rate,
        &args,
    )?;
    let memo_hash = derive_tnk_memo_hash(&wallet.public_key, &nonce)?;
    let tnk_e18 = mu_to_tnk_e18_ceil_u128(amount_mu, rate.tnk_usd_e6)?;
    let tnk_decimal = tnk_e18_to_decimal(tnk_e18);
    let quoted_credit_mu = tnk_e18_to_mu_floor(tnk_e18, rate.tnk_usd_e6)?;
    let intent_payload = pay_tnk_deposit_intent_payload(
        &memo_hash,
        &treasury_address,
        tnk_e18,
        quoted_credit_mu,
        &rate,
    );
    let msb_transfer_command = format!("/transfer {} {}", treasury_address, tnk_decimal);
    let deposit_intent_command = pay_tnk_deposit_intent_command(
        &args.amount,
        &treasury_address,
        &nonce,
        rate.tnk_usd_e6,
        rpc_url.as_deref(),
    );
    let admin_confirm_command = format!(
        "mayhem admin tnk-deposit --memo-hash {} --tnk-e18 {} --msb-tx-hash <msb-tx-hash> --epoch <epoch> --at <unix-seconds>",
        shell_single_quote(&memo_hash),
        tnk_e18
    );

    emit_tnk_handoff(
        args.json,
        amount_mu,
        &msb_transfer_command,
        &memo_hash,
        &deposit_intent_command,
    )?;

    let before_mu = if args.wait {
        let rpc = rpc.as_ref().context("--wait requires peer RPC")?;
        Some(read_user_balance_mu(rpc, &wallet.public_key).await?)
    } else {
        None
    };

    let submitted = if args.submit_intent {
        let rpc = rpc.as_ref().context("--submit-intent requires peer RPC")?;
        let keypair_path = PathBuf::from(wallet.keypair_path.clone());
        Some(
            submit_contract_command(
                rpc,
                &keypair_path,
                args.wallet_password.as_deref().unwrap_or(""),
                &wallet,
                "depositTnk",
                intent_payload.clone(),
                args.sim,
            )
            .await?,
        )
    } else {
        None
    };

    let msb_transfer = if args.submit_transfer {
        let keypair_path = PathBuf::from(wallet.keypair_path.clone());
        let (stores_directory, store_name) = msb_store_from_keypair_path(&keypair_path)?;
        let network = resolve_tnk_msb_network(args.msb_network.as_deref(), &treasury_address)?;
        Some(
            submit_msb_transfer(
                &network,
                &stores_directory,
                &store_name,
                &treasury_address,
                &tnk_decimal,
                args.msb_transfer_timeout_seconds,
            )
            .await?,
        )
    } else {
        None
    };

    let credit = if args.wait {
        let rpc = rpc.as_ref().context("--wait requires peer RPC")?;
        let before_mu = before_mu.context("--wait balance snapshot missing")?;
        let target_mu = before_mu
            .checked_add(amount_mu)
            .context("target balance overflowed")?;
        let status = wait_for_credit(
            rpc,
            &wallet.public_key,
            before_mu,
            target_mu,
            Duration::from_secs(args.timeout_seconds),
            Duration::from_millis(args.poll_interval_ms),
        )
        .await?;
        Some(status)
    } else {
        None
    };

    let report = json!({
        "ok": (submitted.is_some() || !args.submit_intent) && (msb_transfer.is_some() || !args.submit_transfer),
        "rail": "tnk",
        "denom": "mu_usd",
        "amount_mu": amount_mu,
        "amount_usd": mu_to_usd_amount(amount_mu),
        "quoted_credit_mu": quoted_credit_mu,
        "who": wallet.public_key,
        "rpc_url": rpc_url,
        "treasury_address": treasury_address,
        "rate": {
            "denom": "tnk_usd_e6",
            "tnk_usd_e6": rate.tnk_usd_e6,
            "source": rate.source,
            "ts": rate.ts,
        },
        "deposit_intent": {
            "tx_type": "depositTnk",
            "command": intent_payload,
            "submitted": submitted.is_some(),
            "tx": submitted,
        },
        "tnk": {
            "tnk_e18": tnk_e18.to_string(),
            "amount": tnk_decimal,
        },
        "msb_transfer": msb_transfer,
        "memo_hash": memo_hash,
        "nonce": nonce,
        "copy_paste": {
            "msb_transfer_command": msb_transfer_command,
            "transfer_memo_reference": memo_hash,
            "deposit_intent_command": deposit_intent_command,
            "admin_confirm_command": admin_confirm_command,
        },
        "credit": credit.as_ref().map(|status| json!({
            "credited": status.credited,
            "before_mu": status.before_mu,
            "current_mu": status.current_mu,
            "target_mu": status.target_mu,
            "waited_ms": status.waited_ms,
        })),
    });

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        if let Some(status) = credit.as_ref().filter(|status| !status.credited) {
            bail!(
                "timed out waiting for {} mu_usd credit; current balance {} mu_usd, target {} mu_usd",
                amount_mu,
                status.current_mu,
                status.target_mu
            );
        }
    } else {
        println!("TNK amount: {tnk_decimal} ({tnk_e18} e18)");
        println!("Treasury address: {treasury_address}");
        println!("Rate: {} tnk_usd_e6 ({})", rate.tnk_usd_e6, rate.source);
        if let Some(ts) = rate.ts {
            println!("Rate timestamp: {ts}");
        }
        if submitted.is_some() {
            println!("Submitted deposit intent: true");
            if let Some(tx) = report["deposit_intent"]["tx"]["tx"].as_str() {
                println!("Tx: {tx}");
            }
            if let Some(command_hash) = report["deposit_intent"]["tx"]["command_hash"].as_str() {
                println!("Command hash: {command_hash}");
            }
        } else {
            println!(
                "Submitted deposit intent: false (pass --submit-intent to sign and send through peer RPC)"
            );
        }
        if let Some(transfer) = report["msb_transfer"].as_object() {
            if let Some(tx_hash) = transfer.get("tx_hash").and_then(Value::as_str) {
                println!("Submitted MSB transfer: true");
                println!("MSB tx: {tx_hash}");
            }
        } else if args.submit_transfer {
            println!("Submitted MSB transfer: false");
        }
        println!("Copy/paste admin/oracle confirmation command after MSB finality:");
        println!(
            "{}",
            report["copy_paste"]["admin_confirm_command"]
                .as_str()
                .unwrap_or("")
        );
        if let Some(status) = credit {
            if status.credited {
                println!(
                    "Credited: balance {} -> {} mu_usd.",
                    status.before_mu, status.current_mu
                );
            } else {
                bail!(
                    "timed out waiting for {} mu_usd credit; current balance {} mu_usd, target {} mu_usd",
                    amount_mu,
                    status.current_mu,
                    status.target_mu
                );
            }
        }
    }

    Ok(())
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
    let checkout = create_pay_checkout(PayCheckoutRequest {
        rail,
        paygate_url: &paygate_url,
        who: &wallet.public_key,
        amount_mu,
        currency: &args.currency,
        locale: &args.locale,
        idempotency_key: args.idempotency_key.as_deref(),
        success_url: args.success_url.as_deref(),
        cancel_url: args.cancel_url.as_deref(),
    })
    .await?;
    emit_checkout_handoff(args.json, rail, amount_mu, &args.currency, &checkout.url)?;
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
        "currency": args.currency.to_ascii_lowercase(),
        "who": wallet.public_key,
        "paygate_url": paygate_url,
        "rpc_url": rpc_url,
        "checkout": {
            "id": checkout.id,
            "url": checkout.url,
            "reference": checkout.reference,
        },
        "copy_paste": checkout_copy_paste_value(&checkout.url),
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
        if args.session_open_timeout_seconds == Some(0) {
            bail!("--session-open-timeout-seconds must be positive");
        }
        if args.session_frame_timeout_seconds == Some(0) {
            bail!("--session-frame-timeout-seconds must be positive");
        }
        let mut session_config =
            ScBridgeGatewaySessionConfig::new(sc_bridge_url.clone(), sc_bridge_token);
        if let Some(seconds) = args.session_open_timeout_seconds {
            session_config.open_timeout = Duration::from_secs(seconds);
        }
        if let Some(seconds) = args.session_frame_timeout_seconds {
            session_config.frame_timeout = Duration::from_secs(seconds);
        }
        let backend = ScBridgeGatewaySessionBackend::new(session_config);
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
    let direct_tool = if args.skip_direct_tool_smoke {
        json!({ "skipped": true })
    } else {
        run_gateway_tool_smoke(&client, &gateway_root, &selected_model.id).await?
    };

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
    default_model_added: bool,
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

#[derive(Debug, Clone, Serialize)]
struct CatalogCanaryMatrixReport {
    catalog_path: PathBuf,
    canaries_dir: PathBuf,
    launch_only: bool,
    model_count: usize,
    artifact_count: usize,
    ok: bool,
    entries: Vec<CatalogCanaryMatrixEntry>,
    errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CatalogCanaryMatrixEntry {
    model_id: String,
    tier: String,
    artifact: String,
    engine: String,
    canary_set: String,
    prompt_count: Option<usize>,
    fingerprint: Option<String>,
    calibration_status: String,
    ok: bool,
    errors: Vec<String>,
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
    let latest_receipt = latest_gateway_receipt(&receipts).context(
        "canary probes must leave a gateway receipt; run the probe through the normal paid gateway path",
    )?;
    let session_receipt_hash = stable_value_hash(&latest_receipt);
    let receipt_body = receipt_body(&latest_receipt);
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
    session_receipt_hash: String,
    evidence_hash: String,
}

fn canary_probe_command(input: CanaryProbeCommandInput) -> Value {
    json!({
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
        "session_receipt_hash": input.session_receipt_hash,
        "evidence_hash": input.evidence_hash,
    })
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
    let default_model = model_map
        .keys()
        .next()
        .map(|model_id| opencode_model_ref(model_id));
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

    let default_model_added = if !object.contains_key("model") {
        if let Some(default_model) = default_model {
            object.insert("model".to_owned(), Value::String(default_model));
            true
        } else {
            false
        }
    } else {
        false
    };
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
        default_model_added,
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
    let model = opencode_model_ref(model_id);
    let mut command = Command::new(opencode_bin);
    command
        .arg("run")
        .arg("--pure")
        .arg("--model")
        .arg(&model)
        .arg("--title")
        .arg("mayhem-opencode-smoke")
        .arg("--format")
        .arg("json")
        .arg("--dangerously-skip-permissions")
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

fn opencode_model_ref(model_id: &str) -> String {
    format!("{OPENCODE_PROVIDER_ID}/{model_id}")
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
        params: EpochAuditParams {
            fee_bps: args.fee_bps,
        },
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

fn resolve_cli_tnk_treasury_address(
    config: Option<&MayhemConfig>,
    treasury_address: Option<&str>,
) -> Result<String> {
    let value = treasury_address
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            env::var("MAYHEM_TNK_TREASURY_ADDRESS")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            config
                .and_then(|config| config.network.as_ref())
                .and_then(|network| network.tnk_treasury_address.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .context(
            "TNK treasury address required; pass --treasury-address, set MAYHEM_TNK_TREASURY_ADDRESS, or set network.tnk_treasury_address",
        )?;
    if value.split_whitespace().count() != 1 {
        bail!("TNK treasury address must not contain whitespace");
    }
    Ok(value)
}

fn resolve_tnk_msb_network(
    override_network: Option<&str>,
    treasury_address: &str,
) -> Result<String> {
    if let Some(network) = override_network
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return match network.to_ascii_lowercase().as_str() {
            "mainnet" => Ok("mainnet".to_owned()),
            "testnet" | "testnet1" => Ok("testnet1".to_owned()),
            _ => bail!("--msb-network must be mainnet or testnet1"),
        };
    }

    if treasury_address.starts_with("testtrac1") {
        Ok("testnet1".to_owned())
    } else if treasury_address.starts_with("trac1") {
        Ok("mainnet".to_owned())
    } else {
        bail!(
            "could not infer MSB network from treasury address; pass --msb-network mainnet|testnet1"
        )
    }
}

fn msb_store_from_keypair_path(keypair_path: &Path) -> Result<(PathBuf, String)> {
    let keypair_file = keypair_path
        .file_name()
        .and_then(|value| value.to_str())
        .context("wallet keypair path has no filename")?;
    if keypair_file != "keypair.json" {
        bail!("wallet keypair path must end in db/keypair.json for MSB transfer submission");
    }
    let db_dir = keypair_path
        .parent()
        .context("wallet keypair path has no db directory")?;
    let db_name = db_dir
        .file_name()
        .and_then(|value| value.to_str())
        .context("wallet keypair db directory has no name")?;
    if db_name != "db" {
        bail!("wallet keypair path must end in db/keypair.json for MSB transfer submission");
    }
    let store_dir = db_dir
        .parent()
        .context("wallet keypair path has no store directory")?;
    let store_name = store_dir
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .context("wallet store directory has no name")?
        .to_owned();
    let stores_directory = store_dir
        .parent()
        .context("wallet store directory has no parent")?
        .to_path_buf();
    Ok((stores_directory, store_name))
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

fn pay_tnk_deposit_intent_payload(
    memo_hash: &str,
    treasury_address: &str,
    tnk_e18: u128,
    quoted_mu: u64,
    rate: &PayTnkRate,
) -> Value {
    json!({
        "op": "deposit_tnk",
        "memo_hash": memo_hash,
        "treasury_address": treasury_address,
        "tnk_e18": tnk_e18.to_string(),
        "quoted_mu": quoted_mu,
        "rate_tnk_usd_e6": rate.tnk_usd_e6,
        "rate_source": rate.source,
    })
}

async fn resolve_tnk_rate(
    rpc: Option<&PeerRpcClient>,
    override_rate: Option<u64>,
) -> Result<PayTnkRate> {
    if let Some(tnk_usd_e6) = override_rate {
        if tnk_usd_e6 == 0 {
            bail!("--tnk-usd-e6 must be positive");
        }
        return Ok(PayTnkRate {
            tnk_usd_e6,
            source: "cli-override".to_owned(),
            ts: None,
        });
    }

    let rpc = rpc.context(
        "contract rate/latest requires peer RPC; pass --tnk-usd-e6 for offline preparation",
    )?;
    let value = read_state_value(rpc, "rate/latest").await?.context(
        "contract rate/latest not found; run mayhem admin rate-oracle or pass --tnk-usd-e6",
    )?;
    parse_tnk_rate(&value)
}

fn parse_tnk_rate(value: &Value) -> Result<PayTnkRate> {
    let tnk_usd_e6 = value
        .get("tnk_usd_e6")
        .and_then(Value::as_u64)
        .filter(|rate| *rate > 0)
        .context("rate/latest missing positive tnk_usd_e6")?;
    Ok(PayTnkRate {
        tnk_usd_e6,
        source: value
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("rate/latest")
            .to_owned(),
        ts: value.get("ts").and_then(Value::as_u64),
    })
}

fn resolve_tnk_nonce(
    wallet_pubkey: &str,
    amount_mu: u64,
    treasury_address: &str,
    rate: &PayTnkRate,
    args: &PayTnkArgs,
) -> Result<String> {
    if let Some(nonce) = args.nonce.as_deref() {
        if !is_hex_len(nonce, 64) {
            bail!("--nonce must be a 32-byte hex string");
        }
        return Ok(nonce.to_ascii_lowercase());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_nanos();
    Ok(blake3::hash(
        format!(
            "mayhem:tnk-deposit-nonce:v1:{wallet_pubkey}:{amount_mu}:{treasury_address}:{}:{now}:{}",
            rate.tnk_usd_e6,
            std::process::id()
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string())
}

fn derive_tnk_memo_hash(wallet_pubkey: &str, nonce: &str) -> Result<String> {
    let pubkey = hex_decode_array::<32>(wallet_pubkey, "wallet public key")?;
    let nonce = hex_decode_array::<32>(nonce, "TNK deposit nonce")?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(&pubkey);
    hasher.update(&nonce);
    Ok(hasher.finalize().to_hex().to_string())
}

fn mu_to_tnk_e18_ceil_u128(mu: u64, rate_tnk_usd_e6: u64) -> Result<u128> {
    if rate_tnk_usd_e6 == 0 {
        bail!("rate_tnk_usd_e6 must be positive");
    }
    let numerator = u128::from(mu)
        .checked_mul(TNK_E18)
        .context("TNK conversion overflow")?;
    Ok(numerator.div_ceil(u128::from(rate_tnk_usd_e6)))
}

fn tnk_e18_to_mu_floor(tnk_e18: u128, rate_tnk_usd_e6: u64) -> Result<u64> {
    let mu = tnk_e18
        .checked_mul(u128::from(rate_tnk_usd_e6))
        .context("TNK credit conversion overflow")?
        / TNK_E18;
    u64::try_from(mu).context("TNK credit conversion overflowed u64")
}

fn tnk_e18_to_decimal(tnk_e18: u128) -> String {
    let whole = tnk_e18 / TNK_E18;
    let fraction = tnk_e18 % TNK_E18;
    if fraction == 0 {
        return whole.to_string();
    }
    let mut fraction = format!("{fraction:018}");
    while fraction.ends_with('0') {
        fraction.pop();
    }
    format!("{whole}.{fraction}")
}

fn pay_tnk_deposit_intent_command(
    amount: &str,
    treasury_address: &str,
    nonce: &str,
    tnk_usd_e6: u64,
    rpc_url: Option<&str>,
) -> String {
    let mut command = format!(
        "mayhem pay tnk --amount {} --treasury-address {} --nonce {} --tnk-usd-e6 {} --submit-intent",
        shell_single_quote(amount),
        shell_single_quote(treasury_address),
        shell_single_quote(nonce),
        tnk_usd_e6
    );
    if let Some(rpc_url) = rpc_url.filter(|value| !value.trim().is_empty()) {
        command.push_str(" --rpc-url ");
        command.push_str(&shell_single_quote(rpc_url));
    }
    command
}

fn emit_tnk_handoff(
    json_output: bool,
    amount_mu: u64,
    msb_transfer_command: &str,
    memo_hash: &str,
    deposit_intent_command: &str,
) -> Result<()> {
    let lines = [
        format!("Mayhem tnk deposit for {}", mu_to_usd_amount(amount_mu)),
        format!("Copy/paste deposit intent command: {deposit_intent_command}"),
        format!("Copy/paste MSB transfer command: {msb_transfer_command}"),
        format!("Copy/paste transfer memo/reference: {memo_hash}"),
    ];
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

async fn submit_msb_transfer(
    network: &str,
    stores_directory: &Path,
    store_name: &str,
    to: &str,
    amount: &str,
    timeout_seconds: u64,
) -> Result<MsbTransferOutput> {
    run_msb_transfer_helper(vec![
        "transfer".to_owned(),
        "--network".to_owned(),
        network.to_owned(),
        "--stores-directory".to_owned(),
        stores_directory.display().to_string(),
        "--store-name".to_owned(),
        store_name.to_owned(),
        "--to".to_owned(),
        to.to_owned(),
        "--amount".to_owned(),
        amount.to_owned(),
        "--timeout-seconds".to_owned(),
        timeout_seconds.to_string(),
    ])
    .await
}

async fn run_msb_transfer_helper<T>(args: Vec<String>) -> Result<T>
where
    T: DeserializeOwned,
{
    let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/msb-transfer-helper.mjs");
    let output = Command::new("node")
        .arg(&helper)
        .args(args)
        .output()
        .await
        .with_context(|| format!("running MSB transfer helper {}", helper.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "MSB transfer helper failed: {}{}{}",
            stderr.trim(),
            if stderr.trim().is_empty() || stdout.trim().is_empty() {
                ""
            } else {
                "; stdout: "
            },
            stdout.trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("parsing MSB transfer helper JSON output")
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

struct PayCheckoutRequest<'a> {
    rail: PayRail,
    paygate_url: &'a str,
    who: &'a str,
    amount_mu: u64,
    currency: &'a str,
    locale: &'a str,
    idempotency_key: Option<&'a str>,
    success_url: Option<&'a str>,
    cancel_url: Option<&'a str>,
}

#[cfg(test)]
fn checkout_handoff_lines(rail: PayRail, amount_mu: u64, url: &str) -> [String; 2] {
    checkout_handoff_lines_with_currency(rail, amount_mu, "usd", url)
}

fn checkout_handoff_lines_with_currency(
    rail: PayRail,
    amount_mu: u64,
    currency: &str,
    url: &str,
) -> [String; 2] {
    [
        format!(
            "Mayhem {} checkout for {} {}",
            rail.as_str(),
            mu_to_usd_amount(amount_mu),
            currency.to_ascii_uppercase()
        ),
        format!("Copy/paste checkout URL: {url}"),
    ]
}

fn checkout_copy_paste_value(url: &str) -> Value {
    json!({
        "checkout_url": url,
    })
}

fn emit_checkout_handoff(
    json_output: bool,
    rail: PayRail,
    amount_mu: u64,
    currency: &str,
    url: &str,
) -> Result<()> {
    let lines = checkout_handoff_lines_with_currency(rail, amount_mu, currency, url);
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

async fn create_pay_checkout(request: PayCheckoutRequest<'_>) -> Result<PayCheckout> {
    let client = reqwest::Client::new();
    let endpoint = match request.rail {
        PayRail::Stripe => "v1/stripe/checkout-sessions",
        PayRail::Coinbase => "v1/coinbase/charges",
    };
    let success_url = request
        .success_url
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| default_checkout_success_url(request.paygate_url, request.rail));
    let cancel_url = request
        .cancel_url
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| default_checkout_cancel_url(request.paygate_url, request.rail));
    let mut body = json!({
        "who": request.who,
        "mu": request.amount_mu,
    });
    match request.rail {
        PayRail::Stripe => {
            body["success_url"] = Value::String(success_url);
            body["cancel_url"] = Value::String(cancel_url);
            body["currency"] = Value::String(request.currency.to_ascii_lowercase());
            body["locale"] = Value::String(request.locale.to_ascii_lowercase());
            if let Some(idempotency_key) = request.idempotency_key.filter(|value| !value.is_empty())
            {
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
            request.paygate_url.trim_end_matches('/'),
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
    checkout_from_paygate_response(request.rail, &value)
}

fn checkout_from_paygate_response(rail: PayRail, value: &Value) -> Result<PayCheckout> {
    match rail {
        PayRail::Stripe => {
            let session = value
                .get("checkout_session")
                .ok_or_else(|| anyhow::anyhow!("paygate response missing checkout_session"))?;
            Ok(PayCheckout {
                id: required_json_string(session, "id")?,
                url: {
                    let url = required_hosted_checkout_url(session, "url", "checkout.stripe.com")?;
                    validate_response_copy_paste_url(value, &url)?;
                    url
                },
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
                url: {
                    let url = required_hosted_checkout_url(
                        charge,
                        "hosted_url",
                        "commerce.coinbase.com",
                    )?;
                    validate_response_copy_paste_url(value, &url)?;
                    url
                },
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

fn required_hosted_checkout_url(value: &Value, field: &str, expected_host: &str) -> Result<String> {
    let url = required_json_string(value, field)?;
    let parsed = reqwest::Url::parse(&url)
        .with_context(|| format!("paygate response {field} is not a valid URL"))?;
    if parsed.scheme() != "https" || parsed.host_str() != Some(expected_host) {
        bail!("paygate response {field} must be an HTTPS URL on {expected_host}");
    }
    Ok(url)
}

fn validate_response_copy_paste_url(value: &Value, url: &str) -> Result<()> {
    if let Some(copy_paste_url) = value
        .get("copy_paste")
        .and_then(|copy_paste| copy_paste.get("checkout_url"))
        .and_then(Value::as_str)
    {
        if copy_paste_url != url {
            bail!("paygate response copy_paste.checkout_url does not match hosted checkout URL");
        }
    }
    Ok(())
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
    artifact_root_kind: String,
    artifact_source: LedgerArtifactSource,
    #[serde(default)]
    source_sha256: Option<String>,
    manifest_hash: String,
    att_tier: u8,
    binary_hash: String,
    #[serde(default)]
    caps: Value,
    status: String,
    created_by: String,
    #[serde(default)]
    created_by_role: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct LedgerArtifactSource {
    kind: String,
    repo: String,
    revision: String,
    path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LedgerRoom {
    room_id: String,
    sidechannel: String,
    #[serde(default)]
    enclave_id: Option<String>,
    model_id: String,
    #[serde(default)]
    label: String,
    status: String,
    #[serde(default)]
    creator_role: Option<String>,
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
struct LedgerServe {
    provider: String,
    enclave_id: String,
    model_id: String,
    status: String,
    #[serde(default)]
    rooms: Vec<String>,
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
    #[serde(default)]
    set_by_role: Option<String>,
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
    serves: Vec<LedgerServe>,
    providers: Vec<LedgerProvider>,
    prices: Vec<LedgerPriceSchedule>,
    rules: Option<RulesRef>,
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
    params: EpochAuditParams,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EpochAuditParams {
    fee_bps: u64,
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

#[derive(Clone)]
struct ProviderSessionRuntime<'a> {
    rpc: &'a PeerRpcClient,
    rooms: &'a [LedgerRoom],
    keypair_path: &'a Path,
    password: &'a str,
    attestation_identity: CatalogEnclaveIdentity,
    runtime_keypair: &'a RuntimeKeypair,
    binary_path: &'a Path,
    boot_epoch: u64,
}

#[derive(Clone, Debug)]
struct ActiveProviderSession {
    remote: String,
    user_pubkey: String,
    session_id: String,
}

#[derive(Clone, Debug)]
struct ProviderSessionTerms {
    provider: String,
    enclave_id: String,
    model_id: String,
    room_ids: Vec<String>,
    price_ver: u64,
    in_per_1k_mu: u64,
    out_per_1k_mu: u64,
    rules_ver: u64,
    ctx: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct ProviderSignedSessionReceipt {
    #[serde(flatten)]
    body: ReceiptBody,
    enclave_sig: String,
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
    validate_provider_start_security_mode(&args, cfg!(debug_assertions))?;
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
    let session_rpc_url = args
        .session_rpc_url
        .clone()
        .unwrap_or_else(|| rpc_url.clone());
    let session_rpc = if args.serve_sessions {
        Some(PeerRpcClient::new(&session_rpc_url)?)
    } else {
        None
    };
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
    let attestation_identity = CatalogEnclaveIdentity {
        admin_pubkey: selected.enclave.created_by.clone(),
        model_id: selected.enclave.model_id.clone(),
        artifact_root: selected.enclave.artifact_root.clone(),
        manifest_hash: selected.enclave.manifest_hash.clone(),
        binary_hash: binary_hash.clone(),
    };
    let draft = prepare_tier1_attestation_report(&Tier1ExternalProviderAttestationOptions {
        identity: attestation_identity.clone(),
        runtime_keypair: runtime_keypair.clone(),
        provider_pubkey: wallet.public_key.clone(),
        binary_path: binary_path.clone(),
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

    provider_log(&args, "Submitting provider opt-in feature records");
    let provider_feature =
        ensure_provider_registered(&rpc, &keypair_path, &password, &wallet, args.sim).await?;
    let serve_feature = ensure_joined_enclave(
        &rpc,
        &keypair_path,
        &password,
        &wallet,
        &selected.enclave,
        args.sim,
    )
    .await?;
    let room_features = ensure_joined_rooms(
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
        "features": {
            "provider": provider_feature,
            "serve": serve_feature,
            "rooms": room_features,
        },
        "heartbeats": heartbeats,
        "self_test": {
            "ok": true,
            "kind": "sealed-boot-attestation-heartbeat",
        },
        "session_rpc_url": if args.serve_sessions { Some(session_rpc_url.as_str()) } else { None },
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
        let live_rpc = session_rpc.as_ref().unwrap_or(&rpc);
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
            ProviderSessionRuntime {
                rpc: live_rpc,
                rooms: &rooms,
                keypair_path: &keypair_path,
                password: &password,
                attestation_identity,
                runtime_keypair: &runtime_keypair,
                binary_path: &binary_path,
                boot_epoch: attestation.report.boot_epoch,
            },
            responder,
        )
        .await?;
    }

    Ok(())
}

struct ProviderTxContext {
    home: PathBuf,
    rpc_url: String,
    rpc: PeerRpcClient,
    wallet: WalletInfo,
    keypair_path: PathBuf,
    password: String,
}

async fn provider_tx_context(args: &ProviderTxArgs) -> Result<ProviderTxContext> {
    let home = args.home.clone().map(Ok).unwrap_or_else(default_home)?;
    let home = absolutize(home)?;
    let config = read_mayhem_config(&home)?;
    let rpc_url = resolve_cli_rpc_url(Some(&home), args.rpc_url.as_deref())?;
    let password = args.wallet_password.clone().unwrap_or_default();
    let wallet = resolve_cli_wallet(&home, config.as_ref(), &args.peer_store_name, &password)
        .await
        .context("resolving provider wallet")?;
    let keypair_path = PathBuf::from(&wallet.keypair_path);
    let rpc = PeerRpcClient::new(&rpc_url)?;
    Ok(ProviderTxContext {
        home,
        rpc_url,
        rpc,
        wallet,
        keypair_path,
        password,
    })
}

async fn provider_join(args: ProviderJoinArgs) -> Result<()> {
    let ctx = provider_tx_context(&args.tx).await?;
    let contract = read_contract_catalog(&ctx.rpc).await?;
    let enclave = resolve_provider_lifecycle_enclave(&contract.enclaves, &args.enclave)?;
    let price = require_current_mu_usd_price(&contract.prices, &enclave.enclave_id)?;
    let rooms = select_provider_rooms(&contract.rooms, &enclave, &args.rooms)?;
    let provider_feature = ensure_provider_registered(
        &ctx.rpc,
        &ctx.keypair_path,
        &ctx.password,
        &ctx.wallet,
        args.tx.sim,
    )
    .await?;
    let serve_feature = ensure_joined_enclave(
        &ctx.rpc,
        &ctx.keypair_path,
        &ctx.password,
        &ctx.wallet,
        &enclave,
        args.tx.sim,
    )
    .await?;
    let room_features = ensure_joined_rooms(
        &ctx.rpc,
        &ctx.keypair_path,
        &ctx.password,
        &ctx.wallet,
        &enclave,
        &rooms,
        args.tx.sim,
    )
    .await?;
    let report = json!({
        "ok": true,
        "action": "join",
        "home": ctx.home,
        "rpc_url": ctx.rpc_url,
        "provider": ctx.wallet.public_key,
        "sim": args.tx.sim,
        "enclave": enclave,
        "price": price,
        "rooms": rooms,
        "features": {
            "provider": provider_feature,
            "serve": serve_feature,
            "rooms": room_features,
        },
    });
    print_provider_lifecycle_report(&report, args.tx.json)
}

async fn provider_leave(args: ProviderLeaveArgs) -> Result<()> {
    let ctx = provider_tx_context(&args.tx).await?;
    let contract = read_contract_catalog(&ctx.rpc).await?;
    let serves = read_active_provider_serves(&ctx.rpc, &ctx.wallet.public_key).await?;
    let enclave = resolve_provider_leave_enclave(&contract.enclaves, &serves, &args.enclave)?;
    let rooms = select_provider_rooms_to_leave(
        &contract.roomserve,
        &ctx.wallet.public_key,
        &enclave.enclave_id,
        &args.rooms,
    )?;
    let room_features =
        leave_provider_rooms(&ctx, &enclave.enclave_id, &rooms, args.tx.sim).await?;
    let serve_feature = ensure_left_enclave(&ctx, &enclave.enclave_id, args.tx.sim).await?;
    let report = json!({
        "ok": true,
        "action": "leave",
        "home": ctx.home,
        "rpc_url": ctx.rpc_url,
        "provider": ctx.wallet.public_key,
        "sim": args.tx.sim,
        "enclave": enclave,
        "rooms": rooms,
        "features": {
            "rooms": room_features,
            "serve": serve_feature,
        },
    });
    print_provider_lifecycle_report(&report, args.tx.json)
}

async fn provider_stop(args: ProviderStopArgs) -> Result<()> {
    let ctx = provider_tx_context(&args.tx).await?;
    let contract = read_contract_catalog(&ctx.rpc).await?;
    let mut active_rooms = contract
        .roomserve
        .iter()
        .filter(|room| room.provider == ctx.wallet.public_key && room.status == "active")
        .cloned()
        .collect::<Vec<_>>();
    active_rooms.sort_by(|a, b| {
        a.enclave_id
            .cmp(&b.enclave_id)
            .then_with(|| a.room_id.cmp(&b.room_id))
    });
    let mut room_features = Vec::with_capacity(active_rooms.len());
    for room in &active_rooms {
        room_features.push(
            ensure_left_room(&ctx, &room.room_id, &room.enclave_id, args.tx.sim)
                .await
                .with_context(|| {
                    format!(
                        "leaving room {} for enclave {}",
                        room.room_id, room.enclave_id
                    )
                })?,
        );
    }

    let serves = read_active_provider_serves(&ctx.rpc, &ctx.wallet.public_key).await?;
    let mut serve_features = Vec::with_capacity(serves.len());
    for serve in &serves {
        serve_features.push(
            ensure_left_enclave(&ctx, &serve.enclave_id, args.tx.sim)
                .await
                .with_context(|| format!("leaving enclave {}", serve.enclave_id))?,
        );
    }
    let report = json!({
        "ok": true,
        "action": "stop",
        "home": ctx.home,
        "rpc_url": ctx.rpc_url,
        "provider": ctx.wallet.public_key,
        "sim": args.tx.sim,
        "rooms": active_rooms,
        "enclaves": serves,
        "features": {
            "rooms": room_features,
            "serves": serve_features,
        },
    });
    print_provider_lifecycle_report(&report, args.tx.json)
}

async fn provider_room_join(args: ProviderRoomJoinArgs) -> Result<()> {
    let ctx = provider_tx_context(&args.tx).await?;
    let contract = read_contract_catalog(&ctx.rpc).await?;
    let enclave = contract
        .enclaves
        .iter()
        .find(|enclave| enclave.enclave_id == args.enclave)
        .with_context(|| format!("enclave {} is not in contract state", args.enclave))?;
    if enclave.status != "active" {
        bail!("enclave {} is not active", enclave.enclave_id);
    }
    let room = contract
        .rooms
        .iter()
        .find(|room| room.room_id == args.room)
        .with_context(|| format!("room {} is not in contract state", args.room))?;
    if room.status != "open" {
        bail!("room {} is not open", room.room_id);
    }
    require_admin_role_marker("room.creator_role", room.creator_role.as_deref())
        .with_context(|| format!("room {} is not admin-created", room.room_id))?;
    require_canonical_room_transport(room)?;
    if !room_matches_enclave(room, enclave) {
        bail!(
            "room {} is for model {} / enclave {}, not model {} / enclave {}",
            room.room_id,
            room.model_id,
            room.enclave_id
                .as_deref()
                .unwrap_or("<missing-enclave-room>"),
            enclave.model_id,
            enclave.enclave_id
        );
    }
    let price = require_current_mu_usd_price(&contract.prices, &enclave.enclave_id)?;
    let serve_key = format!("serve/{}/{}", ctx.wallet.public_key, enclave.enclave_id);
    let serving = read_state_value(&ctx.rpc, &serve_key).await?;
    if serving
        .as_ref()
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        != Some("active")
    {
        bail!(
            "provider {} is not serving enclave {}; run `mayhem provider join --enclave {}` first",
            ctx.wallet.public_key,
            enclave.enclave_id,
            enclave.enclave_id
        );
    }
    let room_features = ensure_joined_rooms(
        &ctx.rpc,
        &ctx.keypair_path,
        &ctx.password,
        &ctx.wallet,
        enclave,
        std::slice::from_ref(room),
        args.tx.sim,
    )
    .await?;
    let report = json!({
        "ok": true,
        "action": "rooms.join",
        "home": ctx.home,
        "rpc_url": ctx.rpc_url,
        "provider": ctx.wallet.public_key,
        "sim": args.tx.sim,
        "enclave": enclave,
        "price": price,
        "rooms": [room],
        "features": {
            "rooms": room_features,
        },
    });
    print_provider_lifecycle_report(&report, args.tx.json)
}

async fn provider_room_leave(args: ProviderRoomLeaveArgs) -> Result<()> {
    let ctx = provider_tx_context(&args.tx).await?;
    let room_feature = ensure_left_room(&ctx, &args.room, &args.enclave, args.tx.sim).await?;
    let report = json!({
        "ok": true,
        "action": "rooms.leave",
        "home": ctx.home,
        "rpc_url": ctx.rpc_url,
        "provider": ctx.wallet.public_key,
        "sim": args.tx.sim,
        "room_id": args.room,
        "enclave_id": args.enclave,
        "features": {
            "room": room_feature,
        },
    });
    print_provider_lifecycle_report(&report, args.tx.json)
}

fn provider_lifecycle_intent_message(intent: &Value) -> String {
    format!("mayhem-provider-lifecycle-v1{}", stable_json_value(intent))
}

fn provider_lifecycle_feature_key(intent: &Value) -> Result<String> {
    let provider = intent
        .get("provider")
        .and_then(Value::as_str)
        .context("provider lifecycle intent missing provider")?;
    let op = intent
        .get("op")
        .and_then(Value::as_str)
        .context("provider lifecycle intent missing op")?;
    let digest = blake3::hash(provider_lifecycle_intent_message(intent).as_bytes())
        .to_hex()
        .to_string();
    Ok(format!("intent/provider/{provider}/{op}/{digest}"))
}

fn provider_lifecycle_nonce(
    provider: &str,
    op: &str,
    enclave_id: Option<&str>,
    room_id: Option<&str>,
) -> Result<String> {
    Ok(stable_value_hash(&json!({
        "domain": "mayhem-provider-lifecycle-nonce-v1",
        "provider": provider,
        "op": op,
        "enclave_id": enclave_id,
        "room_id": room_id,
        "at_ms": unix_epoch_millis()?,
    })))
}

fn provider_lifecycle_intent(
    provider: &str,
    op: &str,
    enclave_id: Option<&str>,
    room_id: Option<&str>,
) -> Result<Value> {
    let nonce = provider_lifecycle_nonce(provider, op, enclave_id, room_id)?;
    let value = match (enclave_id, room_id) {
        (Some(enclave_id), Some(room_id)) => json!({
            "op": op,
            "provider": provider,
            "enclave_id": enclave_id,
            "room_id": room_id,
            "nonce": nonce,
        }),
        (Some(enclave_id), None) => json!({
            "op": op,
            "provider": provider,
            "enclave_id": enclave_id,
            "nonce": nonce,
        }),
        (None, None) => json!({
            "op": op,
            "provider": provider,
            "nonce": nonce,
        }),
        (None, Some(_)) => bail!("room lifecycle intent requires enclave_id"),
    };
    Ok(value)
}

fn resolve_provider_lifecycle_enclave(
    enclaves: &[LedgerEnclave],
    requested: &str,
) -> Result<LedgerEnclave> {
    if let Some(enclave) = enclaves
        .iter()
        .find(|enclave| enclave.enclave_id == requested && enclave.status == "active")
    {
        require_admin_role_marker(
            "enclave.created_by_role",
            enclave.created_by_role.as_deref(),
        )
        .with_context(|| format!("enclave {} is not admin-created", enclave.enclave_id))?;
        return Ok(enclave.clone());
    }
    let mut matches = enclaves
        .iter()
        .filter(|enclave| {
            enclave.model_id == requested
                && enclave.status == "active"
                && admin_role_marker_ok(enclave.created_by_role.as_deref())
        })
        .cloned()
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| a.enclave_id.cmp(&b.enclave_id));
    match matches.len() {
        0 => bail!(
            "requested {requested} is not an active admin-created enclave id or model lookup"
        ),
        1 => Ok(matches.remove(0)),
        _ => bail!(
            "model {requested} has multiple active admin-created enclaves; pass a concrete enclave id"
        ),
    }
}

fn current_mu_usd_price(schedule: &LedgerPriceSchedule) -> Option<&LedgerPriceRecord> {
    let current = schedule.current.as_ref()?;
    (schedule.denom == "mu_usd"
        && current.denom == "mu_usd"
        && admin_role_marker_ok(current.set_by_role.as_deref()))
    .then_some(current)
}

fn admin_role_marker_ok(role: Option<&str>) -> bool {
    role == Some("admin")
}

fn require_admin_role_marker(field: &str, role: Option<&str>) -> Result<()> {
    if admin_role_marker_ok(role) {
        return Ok(());
    }
    bail!("{field} must be admin; got {}", role.unwrap_or("<missing>"))
}

fn require_current_mu_usd_price<'a>(
    prices: &'a [LedgerPriceSchedule],
    enclave_id: &str,
) -> Result<&'a LedgerPriceSchedule> {
    let schedule = prices
        .iter()
        .find(|price| price.enclave_id == enclave_id)
        .with_context(|| {
            format!(
                "enclave {enclave_id} has no admin price; ask the admin to run `mayhem admin set-price` before providers join it"
            )
        })?;
    if current_mu_usd_price(schedule).is_none() {
        bail!(
            "enclave {enclave_id} has no current mu_usd admin price; ask the admin to run `mayhem admin set-price` before providers join it"
        );
    }
    Ok(schedule)
}

fn resolve_provider_leave_enclave(
    enclaves: &[LedgerEnclave],
    serves: &[LedgerServe],
    requested: &str,
) -> Result<LedgerEnclave> {
    if let Some(enclave) = enclaves
        .iter()
        .find(|enclave| enclave.enclave_id == requested)
    {
        return Ok(enclave.clone());
    }
    let mut matches = serves
        .iter()
        .filter(|serve| serve.model_id == requested && serve.status == "active")
        .cloned()
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| a.enclave_id.cmp(&b.enclave_id));
    match matches.len() {
        0 => bail!(
            "requested {requested} is not a known admin-created enclave id or active provider model lookup"
        ),
        1 => enclaves
            .iter()
            .find(|enclave| enclave.enclave_id == matches[0].enclave_id)
            .cloned()
            .with_context(|| {
                format!(
                    "provider is serving enclave {}, but its admin-created enclave record is missing",
                    matches[0].enclave_id
                )
            }),
        _ => bail!(
            "model {requested} has multiple active provider serving rows; pass a concrete enclave id"
        ),
    }
}

fn select_provider_rooms_to_leave(
    roomserve: &[LedgerRoomServe],
    provider: &str,
    enclave_id: &str,
    requested: &str,
) -> Result<Vec<String>> {
    let active = roomserve
        .iter()
        .filter(|row| {
            row.provider == provider && row.enclave_id == enclave_id && row.status == "active"
        })
        .map(|row| row.room_id.clone())
        .collect::<BTreeSet<_>>();
    if requested.trim().eq_ignore_ascii_case("auto") {
        return Ok(active.into_iter().collect());
    }
    let requested_ids = requested
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if requested_ids.is_empty() {
        bail!("--rooms must be auto or a comma-separated list of room ids");
    }
    for room_id in &requested_ids {
        if !active.contains(*room_id) {
            bail!("provider is not actively serving room {room_id} with enclave {enclave_id}");
        }
    }
    Ok(requested_ids.into_iter().map(str::to_owned).collect())
}

async fn leave_provider_rooms(
    ctx: &ProviderTxContext,
    enclave_id: &str,
    rooms: &[String],
    sim: bool,
) -> Result<Vec<Value>> {
    let mut reports = Vec::with_capacity(rooms.len());
    for room_id in rooms {
        reports.push(ensure_left_room(ctx, room_id, enclave_id, sim).await?);
    }
    Ok(reports)
}

async fn ensure_left_room(
    ctx: &ProviderTxContext,
    room_id: &str,
    enclave_id: &str,
    sim: bool,
) -> Result<Value> {
    let key = format!(
        "roomserve/{}/{}/{}",
        room_id, ctx.wallet.public_key, enclave_id
    );
    let existing = read_state_value(&ctx.rpc, &key).await?;
    if existing
        .as_ref()
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        != Some("active")
    {
        return Ok(json!({
            "room_id": room_id,
            "enclave_id": enclave_id,
            "skipped": true,
            "reason": "not_joined_room",
            "state": existing,
        }));
    }
    let submitted = submit_provider_lifecycle_feature(
        ProviderLifecycleSubmitContext {
            rpc: &ctx.rpc,
            keypair_path: &ctx.keypair_path,
            password: &ctx.password,
            wallet: &ctx.wallet,
            sim,
        },
        "leave_room",
        Some(enclave_id),
        Some(room_id),
    )
    .await?;
    if sim {
        return Ok(json!({ "room_id": room_id, "enclave_id": enclave_id, "feature": submitted }));
    }
    let state = wait_for_state(&ctx.rpc, &key, |value| {
        value.get("status").and_then(Value::as_str) == Some("inactive")
    })
    .await?;
    Ok(json!({
        "room_id": room_id,
        "enclave_id": enclave_id,
        "skipped": false,
        "feature": submitted,
        "state": state,
    }))
}

async fn ensure_left_enclave(
    ctx: &ProviderTxContext,
    enclave_id: &str,
    sim: bool,
) -> Result<Value> {
    let key = format!("serve/{}/{}", ctx.wallet.public_key, enclave_id);
    let existing = read_state_value(&ctx.rpc, &key).await?;
    if existing
        .as_ref()
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        != Some("active")
    {
        return Ok(json!({
            "enclave_id": enclave_id,
            "skipped": true,
            "reason": "not_serving_enclave",
            "state": existing,
        }));
    }
    let submitted = submit_provider_lifecycle_feature(
        ProviderLifecycleSubmitContext {
            rpc: &ctx.rpc,
            keypair_path: &ctx.keypair_path,
            password: &ctx.password,
            wallet: &ctx.wallet,
            sim,
        },
        "leave_enclave",
        Some(enclave_id),
        None,
    )
    .await?;
    if sim {
        return Ok(json!({ "enclave_id": enclave_id, "feature": submitted }));
    }
    let state = wait_for_state(&ctx.rpc, &key, |value| {
        value.get("status").and_then(Value::as_str) == Some("inactive")
    })
    .await?;
    Ok(json!({
        "enclave_id": enclave_id,
        "skipped": false,
        "feature": submitted,
        "state": state,
    }))
}

async fn read_active_provider_serves(
    rpc: &PeerRpcClient,
    provider: &str,
) -> Result<Vec<LedgerServe>> {
    let prefix = format!("serve/{provider}/");
    let mut serves = read_prefix_entries(rpc, &prefix)
        .await?
        .into_iter()
        .map(|entry| serde_json::from_value(entry.value).context("parsing provider serve record"))
        .collect::<Result<Vec<LedgerServe>>>()?
        .into_iter()
        .filter(|serve| serve.provider == provider && serve.status == "active")
        .collect::<Vec<_>>();
    serves.sort_by(|a, b| a.enclave_id.cmp(&b.enclave_id));
    Ok(serves)
}

fn print_provider_lifecycle_report(report: &Value, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    println!("Provider lifecycle command complete.");
    println!("Action: {}", report["action"].as_str().unwrap_or(""));
    println!("Provider: {}", report["provider"].as_str().unwrap_or(""));
    if let Some(enclave_id) = report["enclave"]["enclave_id"].as_str() {
        println!("Enclave: {enclave_id}");
    }
    let room_count = report["rooms"].as_array().map(Vec::len).unwrap_or(0);
    println!("Rooms touched: {room_count}");
    if report["sim"].as_bool() == Some(true) {
        println!("Simulation: true");
    }
    Ok(())
}

fn provider_log(args: &ProviderStartArgs, message: &str) {
    if !args.print_json {
        println!("-> {message}");
    }
}

fn validate_provider_start_security_mode(
    args: &ProviderStartArgs,
    debug_build: bool,
) -> Result<()> {
    if args.serve_sessions && args.dev_session_shim && !debug_build {
        bail!(
            "--dev-session-shim is debug-build only and cannot serve from an admin-trusted release binary"
        );
    }
    if args.serve_sessions && args.dev_skip_catalog_verify && !debug_build {
        bail!(
            "--serve-sessions requires a signed admin catalog in release builds; --dev-skip-catalog-verify is smoke-test only"
        );
    }
    Ok(())
}

fn provider_session_debug(message: impl AsRef<str>) {
    if std::env::var_os("MAYHEM_PROVIDER_SESSION_DEBUG").is_some() {
        eprintln!("[provider-session] {}", message.as_ref());
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
        serves: read_prefix_values(rpc, "serve/").await?,
        providers: read_prefix_values(rpc, "prov/").await?,
        prices,
        rules: read_state_value(rpc, "rules/current")
            .await?
            .map(serde_json::from_value)
            .transpose()
            .context("parsing rules/current")?,
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
        .filter(|enclave| {
            enclave.status == "active" && admin_role_marker_ok(enclave.created_by_role.as_deref())
        })
        .map(|enclave| (enclave.enclave_id.clone(), enclave))
        .collect::<BTreeMap<_, _>>();
    let open_rooms = contract
        .rooms
        .iter()
        .filter(|room| {
            room.status == "open"
                && admin_role_marker_ok(room.creator_role.as_deref())
                && canonical_room_transport_ok(room)
        })
        .collect::<Vec<_>>();
    let active_providers = contract
        .providers
        .iter()
        .filter(|provider| provider.status == "active")
        .map(|provider| provider.provider.as_str())
        .collect::<BTreeSet<_>>();
    let mut rooms_by_id = BTreeMap::new();
    let mut rooms_by_model: BTreeMap<String, u32> = BTreeMap::new();
    for room in &open_rooms {
        rooms_by_id.insert(room.room_id.clone(), *room);
        let count = rooms_by_model.entry(room.model_id.clone()).or_default();
        *count = count.saturating_add(1);
    }

    let mut prices_by_enclave = BTreeMap::new();
    for schedule in &contract.prices {
        let Some(price) = current_mu_usd_price(schedule) else {
            continue;
        };
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
        let Some(room) = rooms_by_id.get(&serving.room_id) else {
            continue;
        };
        let Some(enclave) = active_enclaves.get(&serving.enclave_id) else {
            continue;
        };
        if !room_matches_enclave(room, enclave) {
            continue;
        }
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
        let Some(room) = rooms_by_id.get(&serving.room_id) else {
            continue;
        };
        let Some(enclave) = active_enclaves.get(&serving.enclave_id) else {
            continue;
        };
        if !room_matches_enclave(room, enclave) {
            continue;
        }
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
                    admin_pubkey: enclave.created_by.clone(),
                    artifact_root: enclave.artifact_root.clone(),
                    manifest_hash: enclave.manifest_hash.clone(),
                    binary_hash: enclave.binary_hash.clone(),
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

fn room_matches_enclave(room: &LedgerRoom, enclave: &LedgerEnclave) -> bool {
    room.model_id == enclave.model_id && room.enclave_id.as_ref() == Some(&enclave.enclave_id)
}

fn canonical_room_transport_ok(room: &LedgerRoom) -> bool {
    is_hex_len(&room.room_id, 32) && room.sidechannel == format!("mx/room/{}", room.room_id)
}

fn require_canonical_room_transport(room: &LedgerRoom) -> Result<()> {
    if canonical_room_transport_ok(room) {
        return Ok(());
    }
    bail!(
        "room {} is not a canonical contract room: room_id must be 32 hex chars and sidechannel must equal mx/room/<room_id>",
        room.room_id
    )
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
    for enclave in contract.enclaves.iter().filter(|enclave| {
        enclave.status == "active" && admin_role_marker_ok(enclave.created_by_role.as_deref())
    }) {
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
        if !ledger_enclave_matches_catalog_artifact(enclave, &artifact) {
            continue;
        }
        let Some(price) = contract
            .prices
            .iter()
            .find(|price| price.enclave_id == enclave.enclave_id)
            .filter(|price| current_mu_usd_price(price).is_some())
            .cloned()
        else {
            continue;
        };
        candidates.push(ProviderCandidate {
            enclave: enclave.clone(),
            model: model.clone(),
            artifact_name,
            artifact,
            verdict: verdict.clone(),
            price: Some(price),
        });
    }
    if candidates.is_empty() {
        bail!(
            "no feasible active admin-created enclaves with a current mu_usd admin price found in contract state; providers can only join priced enclaves the admin already registered"
        );
    }
    Ok(candidates)
}

fn ledger_enclave_matches_catalog_artifact(
    enclave: &LedgerEnclave,
    artifact: &catalog::CatalogArtifact,
) -> bool {
    enclave.artifact_root == artifact.artifact_root
        && enclave.artifact_root_kind == artifact.artifact_root_kind
        && enclave.artifact_source.kind == artifact.source.kind
        && enclave.artifact_source.repo == artifact.source.repo
        && enclave.artifact_source.revision == artifact.source.revision
        && enclave.artifact_source.path == artifact.path
        && enclave.source_sha256 == artifact.source_sha256
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
            .filter(|room| {
                room.status == "open"
                    && admin_role_marker_ok(room.creator_role.as_deref())
                    && canonical_room_transport_ok(room)
                    && room_matches_enclave(room, enclave)
            })
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
        require_admin_role_marker("room.creator_role", room.creator_role.as_deref())
            .with_context(|| format!("room {room_id} is not admin-created"))?;
        require_canonical_room_transport(room)?;
        if !room_matches_enclave(room, enclave) {
            bail!(
                "room {room_id} is for model {} / enclave {}, not model {} / enclave {}",
                room.model_id,
                room.enclave_id
                    .as_deref()
                    .unwrap_or("<missing-enclave-room>"),
                enclave.model_id,
                enclave.enclave_id
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
    require_provider_merkle_artifact(selected)?;
    let artifact_file = format!(
        "{}-{}",
        safe_path_component(&selected.enclave.enclave_id),
        safe_path_component(&selected.artifact_name)
    );
    let destination = downloads_dir.join(artifact_file);
    if destination.exists() {
        let merkle = build_merkle_manifest(&destination, args.chunk_size)?;
        if merkle.root == selected.enclave.artifact_root
            && artifact_sha256_matches(&destination, &selected.artifact)?
        {
            return Ok(destination);
        }
    }

    let source = if let Some(path) = &args.artifact {
        DownloadSource::File(absolutize(path.clone())?)
    } else if selected.artifact.source.kind == "huggingface" {
        DownloadSource::Http {
            url: catalog::huggingface_resolve_url(
                &selected.artifact.source,
                &selected.artifact.path,
            )?,
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
    verify_artifact_sha256(&destination, &selected.artifact)?;
    Ok(destination)
}

fn require_provider_merkle_artifact(selected: &ProviderCandidate) -> Result<()> {
    if selected.artifact.artifact_root_kind != "blake3_merkle_v1" {
        bail!(
            "provider serving requires admin artifact_root_kind blake3_merkle_v1 for {}/{}; catalog has {}",
            selected.model.model_id,
            selected.artifact_name,
            selected.artifact.artifact_root_kind
        );
    }
    Ok(())
}

fn artifact_sha256_matches(path: &Path, artifact: &catalog::CatalogArtifact) -> Result<bool> {
    let Some(expected) = &artifact.source_sha256 else {
        return Ok(true);
    };
    if path.is_dir() {
        return Ok(true);
    }
    Ok(file_sha256_hex(path)? == expected.to_ascii_lowercase())
}

fn verify_artifact_sha256(path: &Path, artifact: &catalog::CatalogArtifact) -> Result<()> {
    if artifact_sha256_matches(path, artifact)? {
        return Ok(());
    }
    bail!(
        "artifact sha256 mismatch for {}; expected admin catalog source_sha256 {}",
        path.display(),
        artifact.source_sha256.as_deref().unwrap_or("<missing>")
    )
}

fn file_sha256_hex(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 64];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("reading {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
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

    let submitted = submit_provider_lifecycle_feature(
        ProviderLifecycleSubmitContext {
            rpc,
            keypair_path,
            password,
            wallet,
            sim,
        },
        "register_provider",
        None,
        None,
    )
    .await?;
    if sim {
        return Ok(submitted);
    }
    let state = wait_for_state(rpc, &key, |value| {
        value.get("status").and_then(Value::as_str) == Some("active")
    })
    .await?;
    Ok(json!({ "skipped": false, "feature": submitted, "state": state }))
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
    let submitted = submit_provider_lifecycle_feature(
        ProviderLifecycleSubmitContext {
            rpc,
            keypair_path,
            password,
            wallet,
            sim,
        },
        "join_enclave",
        Some(&enclave.enclave_id),
        None,
    )
    .await?;
    if sim {
        return Ok(submitted);
    }
    let state = wait_for_state(rpc, &key, |value| {
        value.get("status").and_then(Value::as_str) == Some("active")
    })
    .await?;
    Ok(json!({ "skipped": false, "feature": submitted, "state": state }))
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
        let submitted = submit_provider_lifecycle_feature(
            ProviderLifecycleSubmitContext {
                rpc,
                keypair_path,
                password,
                wallet,
                sim,
            },
            "join_room",
            Some(&enclave.enclave_id),
            Some(&room.room_id),
        )
        .await?;
        if sim {
            reports.push(json!({ "room_id": room.room_id, "feature": submitted }));
            continue;
        }
        let state = wait_for_state(rpc, &key, |value| {
            value.get("status").and_then(Value::as_str) == Some("active")
        })
        .await?;
        reports.push(
            json!({ "room_id": room.room_id, "skipped": false, "feature": submitted, "state": state }),
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
        bail!("contract {tx_type} rejected command: {result}");
    }
    Ok(json!({
        "tx": tx,
        "command_hash": command_hash,
        "result": result,
    }))
}

struct ProviderLifecycleSubmitContext<'a> {
    rpc: &'a PeerRpcClient,
    keypair_path: &'a Path,
    password: &'a str,
    wallet: &'a WalletInfo,
    sim: bool,
}

async fn submit_provider_lifecycle_feature(
    ctx: ProviderLifecycleSubmitContext<'_>,
    op: &str,
    enclave_id: Option<&str>,
    room_id: Option<&str>,
) -> Result<Value> {
    let intent = provider_lifecycle_intent(&ctx.wallet.public_key, op, enclave_id, room_id)?;
    let sig = sign_message(
        ctx.keypair_path,
        ctx.password,
        &provider_lifecycle_intent_message(&intent),
    )
    .await?;
    let key = provider_lifecycle_feature_key(&intent)?;
    let value = json!({
        "op": "provider_lifecycle",
        "intent": intent,
        "sig": sig,
    });
    if ctx.sim {
        return Ok(json!({
            "feature": "mayhem",
            "key": key,
            "value": value,
            "sim": true,
            "submitted": false,
        }));
    }
    let submitted = ctx
        .rpc
        .submit_feature(json!({
            "feature": "mayhem",
            "key": key,
            "value": value,
        }))
        .await
        .with_context(|| format!("submitting free provider lifecycle feature {op}"))?;
    if submitted.get("ok").and_then(Value::as_bool) != Some(true) {
        bail!("provider lifecycle feature {op} was not accepted: {submitted}");
    }
    Ok(json!({
        "feature": "mayhem",
        "key": key,
        "value": value,
        "result": submitted,
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
    runtime: ProviderSessionRuntime<'_>,
    mut responder: Box<dyn ProviderSessionResponder>,
) -> Result<()> {
    let terms = provider_session_terms(&ctx)?;
    let (sc_bridge_url, sc_bridge_token) = resolve_cli_sc_bridge(
        ctx.args.home.as_ref(),
        ctx.args.sc_bridge_url.as_deref(),
        ctx.args.sc_bridge_token.as_deref(),
    )?;
    provider_session_debug(format!("connecting to SC-Bridge {sc_bridge_url}"));
    let mut bridge = ScBridgeClient::connect(ScBridgeConfig::new(&sc_bridge_url, sc_bridge_token)?)
        .await
        .context("connecting to SC-Bridge for provider session serving")?;
    provider_session_debug("subscribing to all direct session frames");
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
    provider_session_debug("session frame subscription ready");

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
                let frame_type = event
                    .get("frame")
                    .and_then(|frame| frame.get("t"))
                    .and_then(Value::as_str)
                    .unwrap_or("frame");
                provider_session_debug(format!(
                    "received session event {} type {frame_type}",
                    event
                        .get("session_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                ));
                handle_provider_session_frame(
                    &mut bridge,
                    &mut sessions,
                    &terms,
                    &runtime,
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
    runtime: &ProviderSessionRuntime<'_>,
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
        Some("s.open") => {
            provider_session_debug(format!(
                "handling s.open for session {session_id} from {remote}"
            ));
            provider_session_debug(format!("opening provider side of session {session_id}"));
            bridge
                .session_open(&remote, &session_id)
                .await
                .context("opening provider side direct session")?;
            provider_session_debug(format!("provider side session {session_id} opened"));
            let static_decision = provider_session_open_decision(&frame, terms);
            let decision = match static_decision {
                ProviderSessionDecision::Accept => {
                    provider_session_debug("s.open static decision accepted; rechecking contract");
                    provider_session_current_state_decision(runtime.rpc, terms, runtime.rooms)
                        .await?
                }
                reject => reject,
            };
            match decision {
                ProviderSessionDecision::Accept => {
                    provider_session_debug(format!("sending s.accept for session {session_id}"));
                    sessions.insert(
                        session_id.clone(),
                        ActiveProviderSession {
                            remote: remote.clone(),
                            user_pubkey: frame
                                .get("user")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            session_id: session_id.clone(),
                        },
                    );
                    let ts = unix_epoch_millis()?;
                    let open_head =
                        session_frame_head(&frame).context("hashing s.open frame for s.accept")?;
                    let att_nonce = frame
                        .get("att_nonce")
                        .and_then(Value::as_str)
                        .context("accepted s.open missing validated att_nonce")?;
                    let session_attestation =
                        provider_session_attestation(runtime, terms, att_nonce)
                            .await
                            .context("building per-session s.accept attestation")?;
                    let mut accept_frame = json!({
                        "t": "s.accept",
                        "v": 1,
                        "session_id": session_id,
                        "open_head": open_head,
                        "att_nonce": att_nonce,
                        "att_report": session_attestation.report,
                        "engine": {
                            "ctx": terms.ctx,
                            "mode": "provider-session-server-v1",
                        },
                        "ts": ts,
                        "nonce": stable_value_hash(&json!({
                            "session_id": frame.get("session_id"),
                            "provider": terms.provider,
                            "kind": "accept",
                            "ts": ts,
                        })),
                    });
                    let accept_payload = session_accept_signing_bytes(&accept_frame)
                        .context("building s.accept signing payload")?;
                    let accept_sig = sign_hex(
                        runtime.keypair_path,
                        runtime.password,
                        &hex_encode(&accept_payload),
                    )
                    .await
                    .context("signing s.accept")?;
                    accept_frame["sig"] = json!(accept_sig);
                    bridge
                        .session_send(&remote, &session_id, accept_frame)
                        .await
                        .context("sending s.accept")?;
                    provider_session_debug(format!("sent s.accept for session {session_id}"));
                }
                ProviderSessionDecision::Reject { code, reason } => {
                    provider_session_debug(format!(
                        "sending s.reject {code} for session {session_id}: {reason}"
                    ));
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
                    provider_session_debug(format!("sent s.reject for session {session_id}"));
                }
            }
        }
        Some("s.req") => {
            provider_session_debug(format!("handling s.req for session {session_id}"));
            let Some(active) = sessions.get(&session_id).cloned() else {
                provider_session_debug(format!(
                    "closing unknown session {session_id} after s.req from {remote}"
                ));
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
            provider_session_debug(format!(
                "building response for session {session_id} request {request_id}"
            ));
            let output = match responder.respond(terms, &body) {
                Ok(output) => output,
                Err(err) => {
                    provider_session_debug(format!(
                        "response failed for session {session_id}: {err:#}"
                    ));
                    send_provider_session_error(
                        bridge,
                        &active.remote,
                        &active.session_id,
                        request_id,
                        "provider_response_failed",
                        &err.to_string(),
                    )
                    .await?;
                    send_provider_session_close(
                        bridge,
                        &active.remote,
                        &active.session_id,
                        "err:provider_response_failed",
                    )
                    .await?;
                    sessions.remove(&session_id);
                    return Ok(());
                }
            };
            provider_session_debug(format!(
                "sending response for session {session_id} request {request_id}"
            ));
            let receipt = match send_provider_session_output(
                bridge,
                &active,
                request_id,
                terms,
                &body,
                &output,
                runtime.runtime_keypair,
            )
            .await
            {
                Ok(receipt) => receipt,
                Err(err) => {
                    provider_session_debug(format!(
                        "sending response failed for session {session_id}: {err:#}"
                    ));
                    send_provider_session_error(
                        bridge,
                        &active.remote,
                        &active.session_id,
                        request_id,
                        "provider_send_failed",
                        &err.to_string(),
                    )
                    .await
                    .ok();
                    send_provider_session_close(
                        bridge,
                        &active.remote,
                        &active.session_id,
                        "err:provider_send_failed",
                    )
                    .await
                    .ok();
                    sessions.remove(&session_id);
                    return Ok(());
                }
            };
            provider_session_debug(format!(
                "waiting for receipt ack on session {session_id} request {request_id}"
            ));
            if wait_for_provider_receipt_ack(bridge, &active, &receipt, Duration::from_secs(5))
                .await
                .is_err()
            {
                provider_session_debug(format!(
                    "receipt ack timeout on session {session_id} request {request_id}"
                ));
                send_provider_session_close(
                    bridge,
                    &active.remote,
                    &active.session_id,
                    "err:receipt_ack",
                )
                .await?;
                sessions.remove(&session_id);
                return Ok(());
            }
            provider_session_debug(format!(
                "receipt ack received on session {session_id} request {request_id}"
            ));
            send_provider_session_close(bridge, &active.remote, &active.session_id, "done").await?;
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
    active: &ActiveProviderSession,
    request_id: &str,
    terms: &ProviderSessionTerms,
    body: &Value,
    output: &ProviderSessionOutput,
    runtime_keypair: &RuntimeKeypair,
) -> Result<ProviderSignedSessionReceipt> {
    if let Some(tool) = &output.tool {
        provider_session_debug(format!(
            "sending tool-call s.delta for session {} request {request_id}",
            active.session_id
        ));
        bridge
            .session_send(
                &active.remote,
                &active.session_id,
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
    } else {
        let mut index = 0_u64;
        for part in provider_stream_parts(&output.content) {
            provider_session_debug(format!(
                "sending content s.delta #{index} for session {} request {request_id}",
                active.session_id
            ));
            bridge
                .session_send(
                    &active.remote,
                    &active.session_id,
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
            maybe_provider_session_delta_delay(&active.session_id, request_id, index).await;
            index = index.saturating_add(1);
        }
        provider_session_debug(format!(
            "sending final s.delta #{index} for session {} request {request_id}",
            active.session_id
        ));
        bridge
            .session_send(
                &active.remote,
                &active.session_id,
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
    }

    let receipt = provider_session_receipt(terms, active, body, output, runtime_keypair)
        .context("building provider session receipt")?;
    provider_session_debug(format!(
        "sending s.receipt seq {} for session {} request {request_id}",
        receipt.body.seq, active.session_id
    ));
    bridge
        .session_send(
            &active.remote,
            &active.session_id,
            json!({
                "t": "s.receipt",
                "v": 1,
                "session_id": &active.session_id,
                "seq": receipt.body.seq,
                "receipt": receipt,
            }),
        )
        .await
        .context("sending s.receipt")?;
    Ok(receipt)
}

async fn maybe_provider_session_delta_delay(session_id: &str, request_id: &str, index: u64) {
    let Some(delay) = provider_session_delta_delay() else {
        return;
    };
    if index >= provider_session_delta_delay_count() {
        return;
    }
    provider_session_debug(format!(
        "delaying after content s.delta #{index} for session {session_id} request {request_id} by {}ms",
        delay.as_millis()
    ));
    tokio::time::sleep(delay).await;
}

fn provider_session_delta_delay() -> Option<Duration> {
    let millis = std::env::var("MAYHEM_PROVIDER_SESSION_DELTA_DELAY_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0);
    (millis > 0).then(|| Duration::from_millis(millis.min(60_000)))
}

fn provider_session_delta_delay_count() -> u64 {
    std::env::var("MAYHEM_PROVIDER_SESSION_DELTA_DELAY_COUNT")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(1)
}

async fn send_provider_session_error(
    bridge: &mut ScBridgeClient,
    remote: &str,
    session_id: &str,
    request_id: &str,
    code: &str,
    message: &str,
) -> Result<()> {
    bridge
        .session_send(
            remote,
            session_id,
            json!({
                "t": "s.error",
                "v": 1,
                "session_id": session_id,
                "rid": request_id,
                "code": code,
                "message": message,
            }),
        )
        .await
        .with_context(|| format!("sending s.error for {session_id}"))?;
    Ok(())
}

async fn wait_for_provider_receipt_ack(
    bridge: &mut ScBridgeClient,
    active: &ActiveProviderSession,
    receipt: &ProviderSignedSessionReceipt,
    wait: Duration,
) -> Result<ReceiptAck> {
    let deadline = Instant::now() + wait;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!(
                "timed out waiting for s.receipt_ack on {}",
                active.session_id
            );
        }
        match bridge.next_session_frame(remaining).await {
            Ok(event) => {
                if event.get("session_id").and_then(Value::as_str)
                    != Some(active.session_id.as_str())
                {
                    continue;
                }
                let frame = event.get("frame").cloned().unwrap_or(Value::Null);
                match frame.get("t").and_then(Value::as_str) {
                    Some("s.receipt_ack") => {
                        return provider_session_receipt_ack_from_frame(&frame, active, receipt);
                    }
                    Some("s.close") => {
                        bail!("session {} closed before s.receipt_ack", active.session_id);
                    }
                    _ => {}
                }
            }
            Err(BridgeError::Timeout) => {
                bail!(
                    "timed out waiting for s.receipt_ack on {}",
                    active.session_id
                );
            }
            Err(err) => return Err(err).context("reading s.receipt_ack"),
        }
    }
}

fn provider_session_receipt_ack_from_frame(
    frame: &Value,
    active: &ActiveProviderSession,
    receipt: &ProviderSignedSessionReceipt,
) -> Result<ReceiptAck> {
    if frame.get("t").and_then(Value::as_str) != Some("s.receipt_ack") {
        bail!("receipt ack frame must have t=s.receipt_ack");
    }
    if frame.get("session_id").and_then(Value::as_str) != Some(active.session_id.as_str()) {
        bail!("receipt ack session_id mismatch");
    }
    let ack = ReceiptAck {
        session_id: active.session_id.clone(),
        seq: frame
            .get("seq")
            .and_then(Value::as_u64)
            .context("receipt ack missing seq")?,
        user_sig: frame
            .get("user_sig")
            .and_then(Value::as_str)
            .context("receipt ack missing user_sig")?
            .to_owned(),
    };
    if ack.seq != receipt.body.seq {
        bail!("receipt ack seq mismatch");
    }
    verify_provider_session_receipt_ack(&ack, &active.user_pubkey, &receipt.body)?;
    Ok(ack)
}

fn verify_provider_session_receipt_ack(
    ack: &ReceiptAck,
    user_pubkey: &str,
    body: &ReceiptBody,
) -> Result<()> {
    let key_bytes = hex_decode_array::<32>(user_pubkey, "receipt ack user pubkey")?;
    let sig_bytes = hex_decode_array::<64>(&ack.user_sig, "receipt ack user signature")?;
    let verifying_key =
        VerifyingKey::from_bytes(&key_bytes).context("invalid receipt ack user pubkey")?;
    let signature = Signature::from_bytes(&sig_bytes);
    verifying_key
        .verify(&receipt_signing_bytes(body)?, &signature)
        .context("receipt ack user signature failed")
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
    Ok(())
}

fn provider_session_receipt(
    terms: &ProviderSessionTerms,
    active: &ActiveProviderSession,
    body: &Value,
    output: &ProviderSessionOutput,
    runtime_keypair: &RuntimeKeypair,
) -> Result<ProviderSignedSessionReceipt> {
    let usage = ReceiptUsage {
        in_tokens: output.prompt_tokens,
        out_tokens: output.completion_tokens,
    };
    let receipt_body = ReceiptBody {
        schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
        session_id: active.session_id.clone(),
        seq: 1,
        final_receipt: true,
        user: active.user_pubkey.clone(),
        provider: terms.provider.clone(),
        enclave_id: terms.enclave_id.clone(),
        model_id: terms.model_id.clone(),
        price_ver: terms.price_ver,
        rules_ver: terms.rules_ver,
        usage: usage.clone(),
        mu_owed_cum: provider_session_mu_owed(terms, &usage),
        prompt_hash: provider_session_prompt_hash(body),
        ts: unix_epoch_millis()?,
    };
    let payload = receipt_signing_bytes(&receipt_body).context("building receipt signing bytes")?;
    Ok(ProviderSignedSessionReceipt {
        body: receipt_body,
        enclave_sig: runtime_keypair.sign_hex(&payload),
    })
}

fn provider_session_mu_owed(terms: &ProviderSessionTerms, usage: &ReceiptUsage) -> u64 {
    let raw = u128::from(usage.in_tokens) * u128::from(terms.in_per_1k_mu)
        + u128::from(usage.out_tokens) * u128::from(terms.out_per_1k_mu);
    let rounded = if raw == 0 { 0 } else { raw.div_ceil(1000) };
    rounded.min(u128::from(u64::MAX)) as u64
}

fn provider_session_prompt_hash(body: &Value) -> String {
    blake3::hash(provider_session_prompt_text(body).as_bytes())
        .to_hex()
        .to_string()
}

fn provider_session_prompt_text(body: &Value) -> String {
    body.get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .map(provider_message_to_text)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

async fn provider_session_attestation(
    runtime: &ProviderSessionRuntime<'_>,
    terms: &ProviderSessionTerms,
    att_nonce: &str,
) -> Result<Tier1AttestationReport> {
    let report_ts = unix_epoch_seconds()?;
    let draft = prepare_provider_session_attestation(
        &runtime.attestation_identity,
        runtime.runtime_keypair,
        &terms.provider,
        runtime.binary_path,
        runtime.boot_epoch,
        report_ts,
        att_nonce,
    )?;
    let provider_attestation_sig = sign_hex(
        runtime.keypair_path,
        runtime.password,
        &draft.provider_signing_message_hex,
    )
    .await?;
    finalize_tier1_attestation_report(draft, provider_attestation_sig)
        .context("finalizing per-session provider attestation")
}

fn prepare_provider_session_attestation(
    identity: &CatalogEnclaveIdentity,
    runtime_keypair: &RuntimeKeypair,
    provider_pubkey: &str,
    binary_path: &Path,
    boot_epoch: u64,
    report_ts: u64,
    att_nonce: &str,
) -> Result<Tier1AttestationDraft> {
    prepare_tier1_attestation_report(&Tier1ExternalProviderAttestationOptions {
        identity: identity.clone(),
        runtime_keypair: runtime_keypair.clone(),
        provider_pubkey: provider_pubkey.to_owned(),
        binary_path: binary_path.to_path_buf(),
        boot_epoch,
        report_ts,
        nonce_u: att_nonce.to_owned(),
    })
    .context("preparing per-session provider attestation")
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
        "trt-llm" => {
            let mut backend = mayhem_engine::TrtLlmBackend::new()
                .context("initializing TensorRT-LLM provider session engine")?;
            backend
                .load(load_config)
                .context("loading TensorRT-LLM provider session engine")?;
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
        "trt-llm" => ModelArtifact::trt_llm_checkpoint(artifact_path),
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
        "trt-llm" => LoadConfig::trt_llm_checkpoint(artifact_path),
        other => bail!("unsupported local provider session engine {other}"),
    };
    config.artifact = artifact;
    config.ctx_size = ctx_size.max(1);
    config.gpu_layers = selected.verdict.n_layers_gpu;
    if selected.artifact.engine == "trt-llm" {
        config.trt_engine_dir = Some(trt_engine_cache_dir(artifact_path, &selected.artifact_name));
        config.trt_tensor_parallel = Some(1);
        config.trt_kv_cache_dtype =
            trt_kv_cache_dtype_for_artifact(&selected.artifact_name, &selected.artifact);
        config.trt_require_engine_dir = true;
    }
    Ok(config)
}

fn trt_engine_cache_dir(artifact_path: &Path, artifact_name: &str) -> PathBuf {
    let base = if artifact_path.is_dir() {
        artifact_path.to_path_buf()
    } else {
        artifact_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    };
    base.join(".trtllm-engines")
        .join(safe_path_component(artifact_name))
}

fn trt_kv_cache_dtype_for_artifact(
    _artifact_name: &str,
    artifact: &catalog::CatalogArtifact,
) -> Option<String> {
    let mut haystack = String::new();
    if let Some(notes) = &artifact.notes {
        haystack.push_str(&notes.to_ascii_lowercase());
    }
    if haystack.contains("kv_cache_dtype=nvfp4")
        || haystack.contains("kv-cache-dtype=nvfp4")
        || haystack.contains("kv_cache_dtype: nvfp4")
        || haystack.contains("kv-cache-dtype: nvfp4")
    {
        Some("nvfp4".to_owned())
    } else if haystack.contains("kv_cache_dtype=fp8")
        || haystack.contains("kv-cache-dtype=fp8")
        || haystack.contains("kv_cache_dtype: fp8")
        || haystack.contains("kv-cache-dtype: fp8")
    {
        Some("fp8".to_owned())
    } else {
        None
    }
}

fn provider_session_terms(ctx: &ProviderSessionContext<'_>) -> Result<ProviderSessionTerms> {
    let price = ctx
        .selected
        .price
        .as_ref()
        .and_then(current_mu_usd_price)
        .context("selected provider enclave has no current admin price")?;
    Ok(ProviderSessionTerms {
        provider: ctx.wallet.public_key.clone(),
        enclave_id: ctx.selected.enclave.enclave_id.clone(),
        model_id: ctx.selected.enclave.model_id.clone(),
        room_ids: ctx.rooms.iter().map(|room| room.room_id.clone()).collect(),
        price_ver: price.ver,
        in_per_1k_mu: price.in_per_1k_mu,
        out_per_1k_mu: price.out_per_1k_mu,
        rules_ver: ctx.rules.ver,
        ctx: ctx.selected.model.caps.ctx_max,
    })
}

async fn provider_session_current_state_decision(
    rpc: &PeerRpcClient,
    terms: &ProviderSessionTerms,
    startup_rooms: &[LedgerRoom],
) -> Result<ProviderSessionDecision> {
    let contract = read_contract_catalog(rpc).await?;
    Ok(provider_session_contract_decision(
        &contract,
        terms,
        startup_rooms,
    ))
}

fn provider_session_contract_decision(
    contract: &ContractCatalog,
    terms: &ProviderSessionTerms,
    startup_rooms: &[LedgerRoom],
) -> ProviderSessionDecision {
    let reject = |code, reason: String| ProviderSessionDecision::Reject { code, reason };
    if contract.rules.as_ref().map(|rules| rules.ver) != Some(terms.rules_ver) {
        return reject(
            "CONSENT",
            "current contract rules version no longer matches provider startup terms".to_owned(),
        );
    }
    if contract
        .providers
        .iter()
        .find(|provider| provider.provider == terms.provider)
        .map(|provider| provider.status.as_str())
        != Some("active")
    {
        return reject(
            "BANNED",
            "provider is no longer active in contract state".to_owned(),
        );
    }
    let Some(enclave) = contract
        .enclaves
        .iter()
        .find(|enclave| enclave.enclave_id == terms.enclave_id)
    else {
        return reject(
            "ENCLAVE",
            "admin enclave is no longer present in contract state".to_owned(),
        );
    };
    if enclave.status != "active" || enclave.model_id != terms.model_id {
        return reject(
            "ENCLAVE",
            "admin enclave is no longer active for this model".to_owned(),
        );
    }
    if !admin_role_marker_ok(enclave.created_by_role.as_deref()) {
        return reject(
            "ENCLAVE",
            "enclave record is explicitly not admin-created".to_owned(),
        );
    }
    if !contract.serves.iter().any(|serve| {
        serve.provider == terms.provider
            && serve.enclave_id == terms.enclave_id
            && serve.status == "active"
    }) {
        return reject(
            "SERVE",
            "provider is no longer actively serving this admin enclave".to_owned(),
        );
    }
    let Some(schedule) = contract
        .prices
        .iter()
        .find(|price| price.enclave_id == terms.enclave_id)
    else {
        return reject(
            "PRICE_VER",
            "admin price schedule is no longer present for this enclave".to_owned(),
        );
    };
    let Some(current_price) = current_mu_usd_price(schedule) else {
        return reject(
            "PRICE_VER",
            "admin price schedule is no longer current mu_usd".to_owned(),
        );
    };
    if current_price.ver != terms.price_ver {
        return reject(
            "PRICE_VER",
            "current admin price version changed after provider startup".to_owned(),
        );
    }

    let startup_room_ids = terms.room_ids.iter().collect::<BTreeSet<_>>();
    let startup_rooms_by_id = startup_rooms
        .iter()
        .map(|room| (room.room_id.as_str(), room))
        .collect::<BTreeMap<_, _>>();
    let live_rooms_by_id = contract
        .rooms
        .iter()
        .map(|room| (room.room_id.as_str(), room))
        .collect::<BTreeMap<_, _>>();
    let has_active_roomserve = contract.roomserve.iter().any(|serving| {
        if serving.provider != terms.provider
            || serving.enclave_id != terms.enclave_id
            || serving.status != "active"
            || !startup_room_ids.contains(&serving.room_id)
        {
            return false;
        }
        let Some(startup_room) = startup_rooms_by_id.get(serving.room_id.as_str()) else {
            return false;
        };
        let Some(live_room) = live_rooms_by_id.get(serving.room_id.as_str()) else {
            return false;
        };
        live_room.status == "open"
            && admin_role_marker_ok(live_room.creator_role.as_deref())
            && admin_role_marker_ok(startup_room.creator_role.as_deref())
            && canonical_room_transport_ok(live_room)
            && canonical_room_transport_ok(startup_room)
            && room_matches_enclave(live_room, enclave)
            && room_matches_enclave(startup_room, enclave)
    });
    if !has_active_roomserve {
        return reject(
            "ROOM",
            "provider is no longer active in any startup canonical room".to_owned(),
        );
    }
    ProviderSessionDecision::Accept
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
    let user = frame.get("user").and_then(Value::as_str).unwrap_or("");
    if !is_hex_len(user, 64) {
        return reject("USER", "session user must be 32 bytes of hex".to_owned());
    }
    let att_nonce = frame.get("att_nonce").and_then(Value::as_str).unwrap_or("");
    if !is_hex_len(att_nonce, 64) {
        return reject(
            "ATTESTATION",
            "att_nonce must be 32 bytes of hex".to_owned(),
        );
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
    let prompt_lower = prompt.to_lowercase();
    if prompt.contains(OPENCODE_TEST_MARKER) || prompt_lower.contains("bash tool") {
        return ProviderSessionOutput {
            content: String::new(),
            tool: Some(json!({
                "id": format!("call-{}", stable_value_hash(&json!({ "tool": "bash", "prompt": prompt }))),
                "name": "bash",
                "arguments": provider_tool_arguments("bash"),
            })),
            finish_reason: "tool_calls".to_owned(),
            prompt_tokens,
            completion_tokens: 1,
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

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode_array<const N: usize>(value: &str, label: &str) -> Result<[u8; N]> {
    if value.len() != N * 2 {
        bail!("{label} must be {N} bytes of hex");
    }
    let mut out = [0_u8; N];
    let bytes = value.as_bytes();
    for index in 0..N {
        let high = hex_nibble(bytes[index * 2]).with_context(|| format!("{label} is not hex"))?;
        let low =
            hex_nibble(bytes[index * 2 + 1]).with_context(|| format!("{label} is not hex"))?;
        out[index] = (high << 4) | low;
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
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
    let helper = env::var_os("MAYHEM_WALLET_HELPER")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/wallet-helper.mjs"));
    let node = env::var_os("MAYHEM_NODE_BIN").unwrap_or_else(|| "node".into());
    let output = Command::new(&node)
        .arg(&helper)
        .args(args)
        .output()
        .await
        .with_context(|| {
            format!(
                "running wallet helper {} with {}",
                helper.display(),
                PathBuf::from(&node).display()
            )
        })?;

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
    let key = format!("consent/{}/{}/{}", wallet.public_key, rules.ver, rules.hash);
    let value = json!({
        "op": "consent",
        "sender": wallet.public_key,
        "ver": rules.ver,
        "hash": rules.hash,
        "sig": consent_sig,
    });
    let feature = json!({
        "feature": "mayhem",
        "key": key,
        "value": value,
    });
    let result = if sim {
        None
    } else {
        Some(
            rpc.submit_feature(&feature)
                .await
                .context("submitting free consent feature")?,
        )
    };

    let state = if sim {
        None
    } else {
        Some(wait_for_consent_state(rpc, &wallet.public_key, &rules).await?)
    };

    Ok(ConsentReport {
        skipped: false,
        simulated: sim,
        rules: Some(rules),
        tx: None,
        command_hash: None,
        feature: Some(feature),
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
    if let Some(notice) = setup_admin_payout_notice(report["role"].as_str().unwrap_or_default()) {
        println!("{notice}");
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

fn setup_admin_payout_notice(role: &str) -> Option<&'static str> {
    matches!(role, "provider" | "both").then_some(
        "Provider payout target: admin-set later; setup does not set provider payout terms.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
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
    fn admin_set_price_cli_parses_hyphenated_mu_usd_flags() {
        let cli = Cli::try_parse_from([
            "mayhem",
            "admin",
            "set-price",
            "--enclave-id",
            "enclave-a",
            "--in-per-1k-mu",
            "20",
            "--out-per-1k-mu",
            "60",
            "--per-req-mu",
            "1",
            "--min-session-mu",
            "100",
            "--effective-at",
            "21600",
            "--json",
        ])
        .unwrap();

        let Commands::Admin { command } = cli.command else {
            panic!("expected admin command");
        };
        let AdminCommands::SetPrice(args) = *command else {
            panic!("expected set-price command");
        };
        assert_eq!(args.enclave_id, "enclave-a");
        assert_eq!(args.in_per_1k_mu, 20);
        assert_eq!(args.out_per_1k_mu, 60);
        assert_eq!(args.per_req_mu, 1);
        assert_eq!(args.min_session_mu, 100);
        assert_eq!(args.effective_at, 21600);
        assert!(args.tx.json);
    }

    #[test]
    fn admin_set_price_payload_uses_canonical_mu_usd_terms() {
        let args = AdminSetPriceArgs {
            tx: test_admin_tx_args(),
            enclave_id: "enclave-a".to_owned(),
            in_per_1k_mu: 20,
            out_per_1k_mu: 60,
            per_req_mu: 0,
            min_session_mu: 100,
            effective_at: 21_600,
        };

        assert_eq!(
            admin_set_price_payload(&args),
            json!({
                "op": "set_price",
                "enclave_id": "enclave-a",
                "in_per_1k_mu": 20,
                "out_per_1k_mu": 60,
                "per_req_mu": 0,
                "min_session_mu": 100,
                "effective_at": 21_600,
            })
        );
    }

    #[test]
    fn admin_rules_and_params_payloads_cover_admin_control_plane() {
        let rules = AdminSetRulesArgs {
            tx: test_admin_tx_args(),
            ver: 2,
            hash: "aa".repeat(32),
        };
        assert_eq!(
            admin_set_rules_payload(&rules),
            json!({
                "op": "set_rules",
                "ver": 2,
                "hash": "aa".repeat(32),
            })
        );

        let params = AdminSetParamsArgs {
            tx: test_admin_tx_args(),
            submitted_at: 0,
            effective_at: 86_400,
            values_json: Some(r#"{ "fee_bps": 1500, "holdback_epochs": 168 }"#.to_owned()),
            values_file: None,
        };
        assert_eq!(
            admin_set_params_payload(&params).unwrap(),
            json!({
                "op": "set_params",
                "submitted_at": 0,
                "effective_at": 86_400,
                "values": {
                    "fee_bps": 1500,
                    "holdback_epochs": 168,
                },
            })
        );

        let bad = AdminSetParamsArgs {
            values_json: Some("[]".to_owned()),
            ..params
        };
        let err = admin_set_params_payload(&bad).unwrap_err();
        assert!(err
            .to_string()
            .contains("contract params JSON must be an object"));
    }

    #[test]
    fn admin_open_room_payload_defaults_empty_policy_and_requires_object_policy() {
        let args = AdminOpenRoomArgs {
            tx: test_admin_tx_args(),
            enclave_id: "enclave-a".to_owned(),
            model: Some("catalog/model".to_owned()),
            nonce: "stable-room-nonce".to_owned(),
            label: "eu-central".to_owned(),
            policy_json: None,
            policy_file: None,
        };

        assert_eq!(
            admin_open_room_payload(&args).unwrap(),
            json!({
                "op": "open_room",
                "enclave_id": "enclave-a",
                "model_id": "catalog/model",
                "nonce": "stable-room-nonce",
                "label": "eu-central",
                "policy": {},
            })
        );

        let bad = AdminOpenRoomArgs {
            policy_json: Some("[]".to_owned()),
            ..args
        };
        let err = admin_open_room_payload(&bad).unwrap_err();
        assert!(err
            .to_string()
            .contains("room policy JSON must be an object"));
    }

    #[test]
    fn admin_ban_provider_payload_hashes_plaintext_reason_and_rejects_double_reason() {
        let args = AdminBanProviderArgs {
            tx: test_admin_tx_args(),
            provider: "provider-a".to_owned(),
            reason_hash: None,
            reason: Some("served wrong artifact".to_owned()),
        };
        let expected_hash = blake3::hash(b"served wrong artifact").to_hex().to_string();

        assert_eq!(
            admin_ban_provider_payload(&args).unwrap(),
            json!({
                "op": "ban_provider",
                "provider": "provider-a",
                "reason_hash": expected_hash,
            })
        );

        let err = admin_ban_provider_payload(&AdminBanProviderArgs {
            reason_hash: Some("aa".repeat(32)),
            ..args
        })
        .unwrap_err();
        assert!(err
            .to_string()
            .contains("pass only one of --reason-hash or --reason"));
    }

    #[test]
    fn admin_auditor_register_payload_accredits_auditor_key() {
        let (tx_type, payload) =
            admin_command_payload(&AdminCommands::AuditorRegister(AdminAuditorRegisterArgs {
                tx: test_admin_tx_args(),
                auditor: "aa".repeat(32),
                registered_at_seconds: 123,
            }))
            .unwrap();

        assert_eq!(tx_type, "auditorRegister");
        assert_eq!(
            payload,
            json!({
                "op": "auditor_register",
                "auditor": "aa".repeat(32),
                "registered_at_seconds": 123,
            })
        );
    }

    #[test]
    fn admin_payout_payload_is_admin_set_not_provider_supplied() {
        let (tx_type, payload) = admin_command_payload(&AdminCommands::SetProviderPayout(
            AdminSetProviderPayoutArgs {
                tx: test_admin_tx_args(),
                provider: "provider-a".to_owned(),
                payout_method: AdminPayoutMethod::Stripe,
                payout_addr: "acct_adminapproved".to_owned(),
                payout_currency: Some("EUR".to_owned()),
            },
        ))
        .unwrap();

        assert_eq!(tx_type, "setProviderPayout");
        assert_eq!(
            payload,
            json!({
                "op": "set_provider_payout",
                "provider": "provider-a",
                "payout_method": "stripe",
                "payout_addr": "acct_adminapproved",
                "payout_currency": "eur",
            })
        );
    }

    #[test]
    fn admin_oracle_payment_payloads_match_contract_schemas() {
        assert_eq!(
            admin_rate_oracle_payload(&AdminRateOracleArgs {
                tx: test_admin_tx_args(),
                tnk_usd_e6: 50_000,
                source: AdminRateSource::GateSpot,
                ts: 3_600,
            }),
            json!({
                "op": "rate_oracle",
                "tnk_usd_e6": 50_000,
                "source": "gate-spot",
                "ts": 3_600,
            })
        );

        assert_eq!(
            admin_tnk_deposit_payload(&AdminTnkDepositArgs {
                tx: test_admin_tx_args(),
                memo_hash: "memo".to_owned(),
                tnk_e18: "1000000000000000000".to_owned(),
                msb_tx_hash: "msb".to_owned(),
                epoch: 1,
                at: 3_600,
            }),
            json!({
                "op": "tnk_deposit",
                "memo_hash": "memo",
                "tnk_e18": "1000000000000000000",
                "msb_tx_hash": "msb",
                "epoch": 1,
                "at": 3_600,
            })
        );

        assert_eq!(
            admin_fiat_deposit_payload(&AdminFiatDepositArgs {
                tx: test_admin_tx_args(),
                rail: AdminFiatRail::Stripe,
                who: "user-a".to_owned(),
                mu: 10_000_000,
                ext_ref_hash: "stripe-ref-hash".to_owned(),
                fiat_currency: "usd".to_owned(),
                fiat_amount_minor: 1_000,
                epoch: 7,
                at: 25_200,
            })
            .unwrap(),
            json!({
                "op": "fiat_deposit",
                "rail": "stripe",
                "who": "user-a",
                "mu": 10_000_000,
                "ext_ref_hash": "stripe-ref-hash",
                "fiat_currency": "usd",
                "fiat_amount_minor": 1_000,
                "epoch": 7,
                "at": 25_200,
            })
        );

        assert_eq!(
            admin_fiat_chargeback_payload(&AdminFiatChargebackArgs {
                tx: test_admin_tx_args(),
                rail: AdminFiatRail::Coinbase,
                who: "user-a".to_owned(),
                mu: 5_000_000,
                ext_ref_hash: "coinbase-ref-hash".to_owned(),
                dispute_ref_hash: "coinbase-dispute-hash".to_owned(),
                fiat_currency: "eur".to_owned(),
                fiat_amount_minor: 500,
                epoch: 8,
                at: 28_800,
            })
            .unwrap(),
            json!({
                "op": "fiat_chargeback",
                "rail": "coinbase",
                "who": "user-a",
                "mu": 5_000_000,
                "ext_ref_hash": "coinbase-ref-hash",
                "dispute_ref_hash": "coinbase-dispute-hash",
                "fiat_currency": "eur",
                "fiat_amount_minor": 500,
                "epoch": 8,
                "at": 28_800,
            })
        );
    }

    #[test]
    fn admin_payout_confirm_payloads_enforce_rail_specific_evidence() {
        let tnk = admin_payout_confirm_payload(&AdminPayoutConfirmArgs {
            tx: test_admin_tx_args(),
            kind: AdminPayoutConfirmKind::Provider,
            rail: AdminPayoutMethod::Tnk,
            epoch: 7,
            who: "provider-a".to_owned(),
            mu: 1_000_000,
            tnk_e18: Some("500000000000000000".to_owned()),
            msb_tx_hash: Some("msb-tx".to_owned()),
            external_ref: None,
            fiat_currency: None,
            fiat_amount_minor: None,
            at: 25_200,
        })
        .unwrap();
        assert_eq!(
            tnk,
            json!({
                "op": "payout_confirm",
                "epoch": 7,
                "who": "provider-a",
                "mu": 1_000_000,
                "tnk_e18": "500000000000000000",
                "msb_tx_hash": "msb-tx",
                "at": 25_200,
            })
        );

        let stripe = admin_payout_confirm_payload(&AdminPayoutConfirmArgs {
            tx: test_admin_tx_args(),
            kind: AdminPayoutConfirmKind::Provider,
            rail: AdminPayoutMethod::Stripe,
            epoch: 7,
            who: "provider-a".to_owned(),
            mu: 1_000_000,
            tnk_e18: None,
            msb_tx_hash: None,
            external_ref: Some("tr_123".to_owned()),
            fiat_currency: Some("usd".to_owned()),
            fiat_amount_minor: Some(100),
            at: 25_200,
        })
        .unwrap();
        assert_eq!(
            stripe,
            json!({
                "op": "payout_confirm",
                "rail": "stripe",
                "epoch": 7,
                "who": "provider-a",
                "mu": 1_000_000,
                "external_ref": "tr_123",
                "fiat_currency": "usd",
                "fiat_amount_minor": 100,
                "at": 25_200,
            })
        );

        let fee_sweep = admin_payout_confirm_payload(&AdminPayoutConfirmArgs {
            tx: test_admin_tx_args(),
            kind: AdminPayoutConfirmKind::FeeSweep,
            rail: AdminPayoutMethod::Tnk,
            epoch: 7,
            who: "treasury".to_owned(),
            mu: 1_000_000,
            tnk_e18: Some("500000000000000000".to_owned()),
            msb_tx_hash: Some("treasury-msb-tx".to_owned()),
            external_ref: None,
            fiat_currency: None,
            fiat_amount_minor: None,
            at: 25_200,
        })
        .unwrap();
        assert_eq!(
            fee_sweep,
            json!({
                "op": "payout_confirm",
                "kind": "fee_sweep",
                "epoch": 7,
                "who": "treasury",
                "mu": 1_000_000,
                "tnk_e18": "500000000000000000",
                "msb_tx_hash": "treasury-msb-tx",
                "at": 25_200,
            })
        );

        let err = admin_payout_confirm_payload(&AdminPayoutConfirmArgs {
            rail: AdminPayoutMethod::Stripe,
            tnk_e18: Some("1".to_owned()),
            external_ref: Some("tr_123".to_owned()),
            fiat_currency: Some("usd".to_owned()),
            fiat_amount_minor: Some(100),
            ..test_payout_confirm_args()
        })
        .unwrap_err();
        assert!(err
            .to_string()
            .contains("fiat payout confirmations must not include"));

        let err = admin_payout_confirm_payload(&AdminPayoutConfirmArgs {
            kind: AdminPayoutConfirmKind::FeeSweep,
            rail: AdminPayoutMethod::Coinbase,
            external_ref: Some("transfer_123".to_owned()),
            fiat_currency: Some("usd".to_owned()),
            fiat_amount_minor: Some(100),
            ..test_payout_confirm_args()
        })
        .unwrap_err();
        assert!(err.to_string().contains("fee-sweep"));
    }

    #[test]
    fn admin_epoch_payloads_accept_recomputed_outputs() {
        let roots = json!({
            "dep": "d".repeat(64),
            "use": "u".repeat(64),
            "earn": "e".repeat(64),
            "fee": "f".repeat(64),
            "pay": "p".repeat(64),
        });
        let totals = json!({
            "dep_count": 0,
            "dep_mu": 0,
            "use_count": 1,
            "use_mu": 2_000,
            "provider_count": 1,
            "earn_mu": 1_700,
            "fee_mu": 300,
            "fee_cum_mu": 300,
            "pay_count": 0,
            "pay_mu": 0,
        });

        let commit = admin_epoch_commit_payload(&AdminEpochCommitArgs {
            tx: test_admin_tx_args(),
            epoch: Some(7),
            at: 25_200,
            recomputed_file: None,
            roots_json: Some(roots.to_string()),
            roots_file: None,
            totals_json: Some(totals.to_string()),
            totals_file: None,
        })
        .unwrap();
        assert_eq!(
            commit,
            json!({
                "op": "epoch_commit",
                "epoch": 7,
                "at": 25_200,
                "roots": roots,
                "totals": totals,
            })
        );

        let apply = admin_epoch_apply_payload(&AdminEpochApplyArgs {
            tx: test_admin_tx_args(),
            epoch: Some(7),
            at: 25_200,
            recomputed_file: None,
            debits_json: Some(r#"[{"user":"user-a","mu":2000}]"#.to_owned()),
            debits_file: None,
            earnings_json: Some(r#"[{"provider":"provider-a","gross_mu":2000}]"#.to_owned()),
            earnings_file: None,
            roots_json: Some(commit["roots"].to_string()),
            roots_file: None,
            totals_json: Some(commit["totals"].to_string()),
            totals_file: None,
        })
        .unwrap();
        assert_eq!(
            apply,
            json!({
                "op": "epoch_apply",
                "epoch": 7,
                "at": 25_200,
                "debits": [{"user": "user-a", "mu": 2000}],
                "earnings": [{"provider": "provider-a", "gross_mu": 2000}],
                "roots": commit["roots"],
                "totals": commit["totals"],
            })
        );
    }

    #[test]
    fn admin_epoch_payloads_reject_wrong_json_shapes() {
        let roots = json!({
            "dep": "d".repeat(64),
            "use": "u".repeat(64),
            "earn": "e".repeat(64),
            "fee": "f".repeat(64),
            "pay": "p".repeat(64),
        });
        let totals = json!({ "use_mu": 0 });

        let bad_roots = admin_epoch_commit_payload(&AdminEpochCommitArgs {
            tx: test_admin_tx_args(),
            epoch: Some(1),
            at: 1,
            recomputed_file: None,
            roots_json: Some("[]".to_owned()),
            roots_file: None,
            totals_json: Some(totals.to_string()),
            totals_file: None,
        })
        .unwrap_err();
        assert!(bad_roots
            .to_string()
            .contains("epoch roots JSON must be an object"));

        let bad_debits = admin_epoch_apply_payload(&AdminEpochApplyArgs {
            tx: test_admin_tx_args(),
            epoch: Some(1),
            at: 1,
            recomputed_file: None,
            debits_json: Some("{}".to_owned()),
            debits_file: None,
            earnings_json: Some("[]".to_owned()),
            earnings_file: None,
            roots_json: Some(roots.to_string()),
            roots_file: None,
            totals_json: Some(totals.to_string()),
            totals_file: None,
        })
        .unwrap_err();
        assert!(bad_debits
            .to_string()
            .contains("epoch debits JSON must be an array"));
    }

    #[test]
    fn shell_single_quote_handles_embedded_quotes_for_copy_paste_commands() {
        assert_eq!(shell_single_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn provider_lifecycle_intents_are_limited_to_opt_in_and_out_ops() {
        let provider = "11".repeat(32);
        let intent = json!({
            "op": "join_room",
            "provider": provider,
            "enclave_id": "enclave-a",
            "room_id": "room-a",
            "nonce": "22".repeat(32),
        });
        assert_eq!(
            provider_lifecycle_intent_message(&intent),
            format!("mayhem-provider-lifecycle-v1{}", stable_json_value(&intent))
        );
        assert_eq!(
            provider_lifecycle_feature_key(&intent).unwrap(),
            format!(
                "intent/provider/{}/join_room/{}",
                provider,
                blake3::hash(provider_lifecycle_intent_message(&intent).as_bytes()).to_hex()
            )
        );

        let register = provider_lifecycle_intent(&provider, "register_provider", None, None)
            .expect("register intent");
        assert_eq!(register["op"], "register_provider");
        assert_eq!(register["provider"], provider);
        assert!(is_hex_len(register["nonce"].as_str().unwrap(), 64));

        let enclave =
            provider_lifecycle_intent(&provider, "leave_enclave", Some("enclave-a"), None)
                .expect("enclave intent");
        assert_eq!(
            enclave,
            json!({
                "op": "leave_enclave",
                "provider": provider,
                "enclave_id": "enclave-a",
                "nonce": enclave["nonce"].as_str().unwrap(),
            })
        );
    }

    #[test]
    fn provider_lifecycle_resolves_admin_enclave_and_rejects_ambiguous_model() {
        let root = "aa".repeat(32);
        let mut contract = test_contract(&root);
        let resolved =
            resolve_provider_lifecycle_enclave(&contract.enclaves, "test/model@4bit").unwrap();
        assert_eq!(resolved.enclave_id, "11".repeat(32));

        let unknown =
            resolve_provider_lifecycle_enclave(&contract.enclaves, "provider/custom@4bit")
                .unwrap_err();
        assert!(unknown
            .to_string()
            .contains("not an active admin-created enclave id or model lookup"));

        contract.enclaves.push(LedgerEnclave {
            enclave_id: "22".repeat(32),
            model_id: "test/model@4bit".to_owned(),
            backend: "llama.cpp".to_owned(),
            artifact_root: root.clone(),
            artifact_root_kind: "blake3_merkle_v1".to_owned(),
            artifact_source: LedgerArtifactSource {
                kind: "huggingface".to_owned(),
                repo: "test/model".to_owned(),
                revision: "1".repeat(40),
                path: "model.gguf".to_owned(),
            },
            source_sha256: None,
            manifest_hash: "cc".repeat(32),
            att_tier: 1,
            binary_hash: "dd".repeat(32),
            caps: json!({}),
            status: "active".to_owned(),
            created_by: "admin".to_owned(),
            created_by_role: Some("admin".to_owned()),
        });
        let err =
            resolve_provider_lifecycle_enclave(&contract.enclaves, "test/model@4bit").unwrap_err();
        assert!(err.to_string().contains("multiple active"));

        contract.enclaves[0].status = "retired".to_owned();
        let resolved =
            resolve_provider_lifecycle_enclave(&contract.enclaves, &"22".repeat(32)).unwrap();
        assert_eq!(resolved.enclave_id, "22".repeat(32));

        let mut missing_role = test_contract(&"aa".repeat(32));
        missing_role.enclaves[0].created_by_role = None;
        let missing_role_err =
            resolve_provider_lifecycle_enclave(&missing_role.enclaves, "test/model@4bit")
                .unwrap_err();
        assert!(missing_role_err
            .to_string()
            .contains("not an active admin-created enclave"));

        let mut provider_created = test_contract(&"aa".repeat(32));
        provider_created.enclaves[0].created_by_role = Some("provider".to_owned());
        let by_id = resolve_provider_lifecycle_enclave(
            &provider_created.enclaves,
            &provider_created.enclaves[0].enclave_id,
        )
        .unwrap_err();
        assert!(format!("{by_id:#}").contains("created_by_role"));
        let by_model =
            resolve_provider_lifecycle_enclave(&provider_created.enclaves, "test/model@4bit")
                .unwrap_err();
        assert!(by_model
            .to_string()
            .contains("not an active admin-created enclave"));
    }

    #[test]
    fn provider_leave_resolves_retired_enclave_from_provider_serves() {
        let root = "aa".repeat(32);
        let mut contract = test_contract(&root);
        let provider = "55".repeat(32);
        contract.enclaves[0].status = "retired".to_owned();
        let serves = vec![LedgerServe {
            provider: provider.clone(),
            enclave_id: contract.enclaves[0].enclave_id.clone(),
            model_id: contract.enclaves[0].model_id.clone(),
            status: "active".to_owned(),
            rooms: vec!["room-a".to_owned()],
        }];

        let resolved =
            resolve_provider_leave_enclave(&contract.enclaves, &serves, "test/model@4bit").unwrap();
        assert_eq!(resolved.enclave_id, "11".repeat(32));
        assert_eq!(resolved.status, "retired");

        let resolved =
            resolve_provider_leave_enclave(&contract.enclaves, &serves, &"11".repeat(32)).unwrap();
        assert_eq!(resolved.enclave_id, "11".repeat(32));

        contract.enclaves.push(LedgerEnclave {
            enclave_id: "22".repeat(32),
            model_id: "test/model@4bit".to_owned(),
            backend: "llama.cpp".to_owned(),
            artifact_root: root.clone(),
            artifact_root_kind: "blake3_merkle_v1".to_owned(),
            artifact_source: LedgerArtifactSource {
                kind: "huggingface".to_owned(),
                repo: "test/model".to_owned(),
                revision: "1".repeat(40),
                path: "model.gguf".to_owned(),
            },
            source_sha256: None,
            manifest_hash: "cc".repeat(32),
            att_tier: 1,
            binary_hash: "dd".repeat(32),
            caps: json!({}),
            status: "retired".to_owned(),
            created_by: "admin".to_owned(),
            created_by_role: Some("admin".to_owned()),
        });
        let mut ambiguous_serves = serves;
        ambiguous_serves.push(LedgerServe {
            provider,
            enclave_id: "22".repeat(32),
            model_id: "test/model@4bit".to_owned(),
            status: "active".to_owned(),
            rooms: vec![],
        });
        let err = resolve_provider_leave_enclave(
            &contract.enclaves,
            &ambiguous_serves,
            "test/model@4bit",
        )
        .unwrap_err();
        assert!(err.to_string().contains("multiple active provider"));
    }

    #[test]
    fn provider_lifecycle_requires_current_admin_price() {
        let root = "aa".repeat(32);
        let mut contract = test_contract(&root);
        assert!(require_current_mu_usd_price(&contract.prices, &"11".repeat(32)).is_ok());

        contract.prices.clear();
        let err = require_current_mu_usd_price(&contract.prices, &"11".repeat(32)).unwrap_err();
        assert!(err.to_string().contains("has no admin price"));

        let mut contract = test_contract(&root);
        contract.prices[0].current = None;
        let err = require_current_mu_usd_price(&contract.prices, &"11".repeat(32)).unwrap_err();
        assert!(err
            .to_string()
            .contains("has no current mu_usd admin price"));

        let mut contract = test_contract(&root);
        contract.prices[0].current.as_mut().unwrap().denom = "provider_points".to_owned();
        let err = require_current_mu_usd_price(&contract.prices, &"11".repeat(32)).unwrap_err();
        assert!(err
            .to_string()
            .contains("has no current mu_usd admin price"));
    }

    #[test]
    fn provider_rooms_to_leave_are_provider_and_enclave_scoped() {
        let rows = vec![
            LedgerRoomServe {
                room_id: "room-b".to_owned(),
                provider: "provider-a".to_owned(),
                enclave_id: "enclave-a".to_owned(),
                model_id: "model".to_owned(),
                status: "active".to_owned(),
            },
            LedgerRoomServe {
                room_id: "room-a".to_owned(),
                provider: "provider-a".to_owned(),
                enclave_id: "enclave-a".to_owned(),
                model_id: "model".to_owned(),
                status: "active".to_owned(),
            },
            LedgerRoomServe {
                room_id: "room-other".to_owned(),
                provider: "provider-b".to_owned(),
                enclave_id: "enclave-a".to_owned(),
                model_id: "model".to_owned(),
                status: "active".to_owned(),
            },
        ];

        assert_eq!(
            select_provider_rooms_to_leave(&rows, "provider-a", "enclave-a", "auto").unwrap(),
            vec!["room-a".to_owned(), "room-b".to_owned()]
        );
        assert_eq!(
            select_provider_rooms_to_leave(&rows, "provider-a", "enclave-a", "room-b").unwrap(),
            vec!["room-b".to_owned()]
        );
        assert!(
            select_provider_rooms_to_leave(&rows, "provider-a", "enclave-a", "room-other").is_err()
        );
    }

    #[test]
    fn provider_lifecycle_cli_parses_join_leave_and_room_commands() {
        let join = Cli::try_parse_from([
            "mayhem",
            "provider",
            "join",
            "--enclave",
            "enclave-a",
            "--rooms",
            "room-a,room-b",
            "--sim",
            "--json",
        ])
        .unwrap();
        let Commands::Provider { command } = join.command else {
            panic!("expected provider command");
        };
        let ProviderCommands::Join(args) = *command else {
            panic!("expected provider join command");
        };
        assert_eq!(args.enclave, "enclave-a");
        assert_eq!(args.rooms, "room-a,room-b");
        assert!(args.tx.sim);
        assert!(args.tx.json);

        let rooms = Cli::try_parse_from([
            "mayhem",
            "provider",
            "rooms",
            "leave",
            "--room",
            "room-a",
            "--enclave",
            "enclave-a",
        ])
        .unwrap();
        let Commands::Provider { command } = rooms.command else {
            panic!("expected provider command");
        };
        let ProviderCommands::Rooms { command } = *command else {
            panic!("expected provider rooms command");
        };
        let ProviderRoomsCommands::Leave(args) = command else {
            panic!("expected provider rooms leave command");
        };
        assert_eq!(args.room, "room-a");
        assert_eq!(args.enclave, "enclave-a");
    }

    #[test]
    fn receipts_export_cli_requires_explicit_admin_fee_bps() {
        let missing_fee = Cli::try_parse_from([
            "mayhem",
            "receipts",
            "export",
            "--epoch",
            "1",
            "--output",
            "/tmp/mayhem-epoch.json",
        ]);
        assert!(missing_fee.is_err());

        let export = Cli::try_parse_from([
            "mayhem",
            "receipts",
            "export",
            "--epoch",
            "1",
            "--fee-bps",
            "1500",
            "--output",
            "/tmp/mayhem-epoch.json",
            "--json",
        ])
        .unwrap();
        let Commands::Receipts { command } = export.command else {
            panic!("expected receipts command");
        };
        let ReceiptsCommands::Export(args) = command else {
            panic!("expected receipts export command");
        };
        assert_eq!(args.fee_bps, 1_500);
        assert!(args.json);
    }

    #[test]
    fn setup_notice_keeps_provider_payout_targets_admin_set() {
        assert_eq!(
            setup_admin_payout_notice("provider"),
            Some(
                "Provider payout target: admin-set later; setup does not set provider payout terms."
            )
        );
        assert_eq!(
            setup_admin_payout_notice("both"),
            Some(
                "Provider payout target: admin-set later; setup does not set provider payout terms."
            )
        );
        assert_eq!(setup_admin_payout_notice("user"), None);
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

        let mut unpriced = contract.clone();
        unpriced.prices.clear();
        let err = build_provider_candidates(&unpriced, &catalog, &hardware, &args).unwrap_err();
        assert!(err.to_string().contains("current mu_usd admin price"));

        let mut wrong_denom = contract.clone();
        wrong_denom.prices[0].denom = "provider_points".to_owned();
        let err = build_provider_candidates(&wrong_denom, &catalog, &hardware, &args).unwrap_err();
        assert!(err.to_string().contains("current mu_usd admin price"));

        let mut provider_created = contract.clone();
        provider_created.enclaves[0].created_by_role = Some("provider".to_owned());
        let err =
            build_provider_candidates(&provider_created, &catalog, &hardware, &args).unwrap_err();
        assert!(err.to_string().contains("current mu_usd admin price"));

        let mut provider_priced = contract.clone();
        provider_priced.prices[0]
            .current
            .as_mut()
            .unwrap()
            .set_by_role = Some("provider".to_owned());
        let err =
            build_provider_candidates(&provider_priced, &catalog, &hardware, &args).unwrap_err();
        assert!(err.to_string().contains("current mu_usd admin price"));

        let mut missing_role = contract.clone();
        missing_role.enclaves[0].created_by_role = None;
        missing_role.rooms[0].creator_role = None;
        missing_role.prices[0].current.as_mut().unwrap().set_by_role = None;
        assert!(build_provider_candidates(&missing_role, &catalog, &hardware, &args).is_err());

        let mut mismatched = contract;
        mismatched.enclaves[0].artifact_root = "bb".repeat(32);
        assert!(build_provider_candidates(&mismatched, &catalog, &hardware, &args).is_err());
    }

    #[test]
    fn provider_candidates_reject_ledger_catalog_source_mismatch() {
        let root = "aa".repeat(32);
        let catalog = test_catalog(&root);
        let hardware = test_hardware(FixtureProfile::CpuOnly);
        let args = test_provider_start_args();

        let mut wrong_repo = test_contract(&root);
        wrong_repo.enclaves[0].artifact_source.repo = "provider/fake-model".to_owned();
        assert!(build_provider_candidates(&wrong_repo, &catalog, &hardware, &args).is_err());

        let mut wrong_revision = test_contract(&root);
        wrong_revision.enclaves[0].artifact_source.revision = "2".repeat(40);
        assert!(build_provider_candidates(&wrong_revision, &catalog, &hardware, &args).is_err());

        let mut wrong_path = test_contract(&root);
        wrong_path.enclaves[0].artifact_source.path = "fake.gguf".to_owned();
        assert!(build_provider_candidates(&wrong_path, &catalog, &hardware, &args).is_err());
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
        assert_eq!(
            models[0].mayhem.route_candidates[0].room_id,
            "aa".repeat(16)
        );
        assert_eq!(models[0].mayhem.route_candidates[0].price_ver, 1);
        assert_eq!(
            models[0].mayhem.route_candidates[0].admin_pubkey,
            "44".repeat(32)
        );
        assert_eq!(models[0].mayhem.route_candidates[0].artifact_root, root);
        assert_eq!(
            models[0].mayhem.route_candidates[0].manifest_hash,
            "22".repeat(32)
        );
        assert_eq!(
            models[0].mayhem.route_candidates[0].binary_hash,
            "33".repeat(32)
        );

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

        let mut missing_role = test_contract(&root);
        missing_role.enclaves[0].created_by_role = None;
        missing_role.rooms[0].creator_role = None;
        missing_role.prices[0].current.as_mut().unwrap().set_by_role = None;
        let err = gateway_models_from_contract(&missing_role)
            .expect_err("missing admin role markers should hide model");
        assert!(format!("{err:#}").contains("no canonical contract-backed models"));

        let mut provider_created = test_contract(&root);
        provider_created.enclaves[0].created_by_role = Some("provider".to_owned());
        let err = gateway_models_from_contract(&provider_created)
            .expect_err("provider-created enclave marker should hide model");
        assert!(format!("{err:#}").contains("no canonical contract-backed models"));

        let mut provider_room = test_contract(&root);
        provider_room.rooms[0].creator_role = Some("provider".to_owned());
        let err = gateway_models_from_contract(&provider_room)
            .expect_err("provider-created room marker should hide model");
        assert!(format!("{err:#}").contains("no canonical contract-backed models"));

        let mut provider_priced = test_contract(&root);
        provider_priced.prices[0]
            .current
            .as_mut()
            .unwrap()
            .set_by_role = Some("provider".to_owned());
        let err = gateway_models_from_contract(&provider_priced)
            .expect_err("provider-priced record marker should hide model");
        assert!(format!("{err:#}").contains("no canonical contract-backed models"));

        let mut noncanonical_room_id = test_contract(&root);
        noncanonical_room_id.rooms[0].room_id = "provider-local-only".to_owned();
        noncanonical_room_id.rooms[0].sidechannel = "mx/room/provider-local-only".to_owned();
        noncanonical_room_id.roomserve[0].room_id = "provider-local-only".to_owned();
        noncanonical_room_id.serves[0].rooms = vec!["provider-local-only".to_owned()];
        let err = gateway_models_from_contract(&noncanonical_room_id)
            .expect_err("raw Intercom room ids should hide model");
        assert!(format!("{err:#}").contains("no canonical contract-backed models"));

        let mut wrong_sidechannel = test_contract(&root);
        wrong_sidechannel.rooms[0].sidechannel = "mx/room/provider-made".to_owned();
        let err = gateway_models_from_contract(&wrong_sidechannel)
            .expect_err("wrong room sidechannel should hide model");
        assert!(format!("{err:#}").contains("no canonical contract-backed models"));

        let mut wrong_same_model_enclave = test_contract(&root);
        let other_enclave_id = "66".repeat(32);
        let mut other_enclave = wrong_same_model_enclave.enclaves[0].clone();
        other_enclave.enclave_id = other_enclave_id.clone();
        other_enclave.artifact_root = "77".repeat(32);
        wrong_same_model_enclave.enclaves.push(other_enclave);
        let mut other_price = wrong_same_model_enclave.prices[0].clone();
        other_price.enclave_id = other_enclave_id.clone();
        wrong_same_model_enclave.prices.push(other_price);
        wrong_same_model_enclave.roomserve[0].enclave_id = other_enclave_id;
        let err = gateway_models_from_contract(&wrong_same_model_enclave)
            .expect_err("same-model wrong-enclave roomserve should hide model");
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

        let mut bad_att_nonce = frame.clone();
        bad_att_nonce["att_nonce"] = json!("not-hex");
        assert!(matches!(
            provider_session_open_decision(&bad_att_nonce, &terms),
            ProviderSessionDecision::Reject {
                code: "ATTESTATION",
                ..
            }
        ));

        let mut bad_user = frame.clone();
        bad_user["user"] = json!("not-hex");
        assert!(matches!(
            provider_session_open_decision(&bad_user, &terms),
            ProviderSessionDecision::Reject { code: "USER", .. }
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
    fn provider_session_open_rechecks_current_admin_contract_state() {
        let terms = test_provider_session_terms();
        let contract = test_contract(&"aa".repeat(32));
        let startup_rooms = contract.rooms[..1].to_vec();

        assert_eq!(
            provider_session_contract_decision(&contract, &terms, &startup_rooms),
            ProviderSessionDecision::Accept
        );

        let mut missing_role = contract.clone();
        missing_role.enclaves[0].created_by_role = None;
        missing_role.rooms[0].creator_role = None;
        missing_role.prices[0].current.as_mut().unwrap().set_by_role = None;
        let mut missing_role_startup_rooms = startup_rooms.clone();
        missing_role_startup_rooms[0].creator_role = None;
        assert!(matches!(
            provider_session_contract_decision(&missing_role, &terms, &missing_role_startup_rooms),
            ProviderSessionDecision::Reject { .. }
        ));

        let mut banned = contract.clone();
        banned.providers[0].status = "banned".to_owned();
        assert!(matches!(
            provider_session_contract_decision(&banned, &terms, &startup_rooms),
            ProviderSessionDecision::Reject { code: "BANNED", .. }
        ));

        let mut retired = contract.clone();
        retired.enclaves[0].status = "retired".to_owned();
        assert!(matches!(
            provider_session_contract_decision(&retired, &terms, &startup_rooms),
            ProviderSessionDecision::Reject {
                code: "ENCLAVE",
                ..
            }
        ));

        let mut provider_created_enclave = contract.clone();
        provider_created_enclave.enclaves[0].created_by_role = Some("provider".to_owned());
        assert!(matches!(
            provider_session_contract_decision(&provider_created_enclave, &terms, &startup_rooms),
            ProviderSessionDecision::Reject {
                code: "ENCLAVE",
                ..
            }
        ));

        let mut stale_rules = contract.clone();
        stale_rules.rules.as_mut().unwrap().ver = terms.rules_ver + 1;
        assert!(matches!(
            provider_session_contract_decision(&stale_rules, &terms, &startup_rooms),
            ProviderSessionDecision::Reject {
                code: "CONSENT",
                ..
            }
        ));

        let mut stale_price = contract.clone();
        stale_price.prices[0].current.as_mut().unwrap().ver = terms.price_ver + 1;
        assert!(matches!(
            provider_session_contract_decision(&stale_price, &terms, &startup_rooms),
            ProviderSessionDecision::Reject {
                code: "PRICE_VER",
                ..
            }
        ));

        let mut provider_priced = contract.clone();
        provider_priced.prices[0]
            .current
            .as_mut()
            .unwrap()
            .set_by_role = Some("provider".to_owned());
        assert!(matches!(
            provider_session_contract_decision(&provider_priced, &terms, &startup_rooms),
            ProviderSessionDecision::Reject {
                code: "PRICE_VER",
                ..
            }
        ));

        let mut provider_created_room = contract.clone();
        provider_created_room.rooms[0].creator_role = Some("provider".to_owned());
        assert!(matches!(
            provider_session_contract_decision(&provider_created_room, &terms, &startup_rooms),
            ProviderSessionDecision::Reject { code: "ROOM", .. }
        ));

        let mut provider_startup_room = startup_rooms.clone();
        provider_startup_room[0].creator_role = Some("provider".to_owned());
        assert!(matches!(
            provider_session_contract_decision(&contract, &terms, &provider_startup_room),
            ProviderSessionDecision::Reject { code: "ROOM", .. }
        ));

        let mut wrong_live_sidechannel = contract.clone();
        wrong_live_sidechannel.rooms[0].sidechannel = "mx/room/provider-made".to_owned();
        assert!(matches!(
            provider_session_contract_decision(&wrong_live_sidechannel, &terms, &startup_rooms),
            ProviderSessionDecision::Reject { code: "ROOM", .. }
        ));

        let mut wrong_startup_sidechannel = startup_rooms.clone();
        wrong_startup_sidechannel[0].sidechannel = "mx/room/provider-made".to_owned();
        assert!(matches!(
            provider_session_contract_decision(&contract, &terms, &wrong_startup_sidechannel),
            ProviderSessionDecision::Reject { code: "ROOM", .. }
        ));

        let mut tombstoned_roomserve = contract.clone();
        tombstoned_roomserve.roomserve[0].status = "inactive".to_owned();
        assert!(matches!(
            provider_session_contract_decision(&tombstoned_roomserve, &terms, &startup_rooms),
            ProviderSessionDecision::Reject { code: "ROOM", .. }
        ));

        let mut tombstoned_serve = contract.clone();
        tombstoned_serve.serves[0].status = "inactive".to_owned();
        assert!(matches!(
            provider_session_contract_decision(&tombstoned_serve, &terms, &startup_rooms),
            ProviderSessionDecision::Reject { code: "SERVE", .. }
        ));
    }

    #[test]
    fn provider_session_accept_signing_payload_is_bound_and_sig_excluded() {
        let mut frame = json!({
            "t": "s.accept",
            "v": 1,
            "session_id": "aa".repeat(32),
            "open_head": "bb".repeat(32),
            "att_nonce": "88".repeat(32),
            "att_report": {
                "enclave_id": "11".repeat(32),
                "provider_pubkey": "55".repeat(32),
                "sig_provider": "66".repeat(64),
            },
            "engine": { "ctx": 8192 },
            "ts": 123,
            "nonce": "77".repeat(32),
        });
        let payload = session_accept_signing_bytes(&frame).unwrap();

        frame["sig"] = json!("88".repeat(64));
        assert_eq!(session_accept_signing_bytes(&frame).unwrap(), payload);

        frame["session_id"] = json!("bb".repeat(32));
        assert_ne!(session_accept_signing_bytes(&frame).unwrap(), payload);
    }

    #[test]
    fn provider_session_attestation_draft_binds_open_nonce() {
        let terms = test_provider_session_terms();
        let frame = test_session_open_frame(&terms);
        let att_nonce = frame.get("att_nonce").and_then(Value::as_str).unwrap();
        let binary_path = std::env::current_exe().unwrap();
        let identity = CatalogEnclaveIdentity {
            admin_pubkey: "44".repeat(32),
            model_id: terms.model_id.clone(),
            artifact_root: "aa".repeat(32),
            manifest_hash: "bb".repeat(32),
            binary_hash: measure_binary(&binary_path).unwrap(),
        };
        let runtime_keypair = RuntimeKeypair::from_seed([9; 32]);
        let draft = prepare_provider_session_attestation(
            &identity,
            &runtime_keypair,
            &terms.provider,
            &binary_path,
            100,
            101,
            att_nonce,
        )
        .unwrap();

        assert_eq!(draft.body.nonce_u, att_nonce);
        assert_eq!(draft.body.provider_pubkey, terms.provider.as_str());

        let other_nonce = "99".repeat(32);
        let other_draft = prepare_provider_session_attestation(
            &identity,
            &runtime_keypair,
            &terms.provider,
            &binary_path,
            100,
            101,
            &other_nonce,
        )
        .unwrap();
        assert_eq!(other_draft.body.nonce_u, other_nonce);
        assert_ne!(draft.sig_enclave, other_draft.sig_enclave);
        assert_ne!(
            draft.provider_signing_message_hex,
            other_draft.provider_signing_message_hex
        );
    }

    #[test]
    fn provider_session_receipt_binds_admin_terms_usage_and_runtime_key() {
        let terms = test_provider_session_terms();
        let active = ActiveProviderSession {
            remote: "peer-a".to_owned(),
            user_pubkey: "66".repeat(32),
            session_id: "aa".repeat(32),
        };
        let body = json!({
            "messages": [
                { "role": "system", "content": "be precise" },
                { "role": "user", "content": "hello mayhem" }
            ],
            "stream": true
        });
        let output = ProviderSessionOutput {
            content: "receipt ok".to_owned(),
            tool: None,
            finish_reason: "stop".to_owned(),
            prompt_tokens: 3,
            completion_tokens: 4,
        };
        let runtime_keypair = RuntimeKeypair::from_seed([9; 32]);
        let receipt =
            provider_session_receipt(&terms, &active, &body, &output, &runtime_keypair).unwrap();

        assert_eq!(receipt.body.session_id, active.session_id);
        assert_eq!(receipt.body.user, active.user_pubkey);
        assert_eq!(receipt.body.provider, terms.provider);
        assert_eq!(receipt.body.enclave_id, terms.enclave_id);
        assert_eq!(receipt.body.model_id, terms.model_id);
        assert_eq!(receipt.body.price_ver, terms.price_ver);
        assert_eq!(receipt.body.rules_ver, terms.rules_ver);
        assert_eq!(receipt.body.usage.in_tokens, 3);
        assert_eq!(receipt.body.usage.out_tokens, 4);
        assert_eq!(receipt.body.mu_owed_cum, 1);
        assert_eq!(
            receipt.body.prompt_hash,
            provider_session_prompt_hash(&body)
        );

        let key_bytes: [u8; 32] = test_hex_decode(&runtime_keypair.public_key_hex())
            .try_into()
            .unwrap();
        let sig_bytes: [u8; 64] = test_hex_decode(&receipt.enclave_sig).try_into().unwrap();
        let verifying_key = VerifyingKey::from_bytes(&key_bytes).unwrap();
        let signature = Signature::from_bytes(&sig_bytes);
        verifying_key
            .verify(&receipt_signing_bytes(&receipt.body).unwrap(), &signature)
            .unwrap();
    }

    #[test]
    fn provider_session_receipt_ack_verifies_user_signature() {
        let terms = test_provider_session_terms();
        let user_seed = [41_u8; 32];
        let user_key = SigningKey::from_bytes(&user_seed);
        let active = ActiveProviderSession {
            remote: "peer-a".to_owned(),
            user_pubkey: hex_encode(&user_key.verifying_key().to_bytes()),
            session_id: "aa".repeat(32),
        };
        let body = json!({
            "messages": [{ "role": "user", "content": "hello mayhem" }],
            "stream": true
        });
        let output = ProviderSessionOutput {
            content: "receipt ok".to_owned(),
            tool: None,
            finish_reason: "stop".to_owned(),
            prompt_tokens: 2,
            completion_tokens: 3,
        };
        let receipt = provider_session_receipt(
            &terms,
            &active,
            &body,
            &output,
            &RuntimeKeypair::from_seed([9; 32]),
        )
        .unwrap();
        let payload = receipt_signing_bytes(&receipt.body).unwrap();
        let user_sig = hex_encode(&user_key.sign(&payload).to_bytes());
        let frame = json!({
            "t": "s.receipt_ack",
            "v": 1,
            "session_id": &active.session_id,
            "seq": receipt.body.seq,
            "user_sig": user_sig,
        });

        let ack = provider_session_receipt_ack_from_frame(&frame, &active, &receipt).unwrap();
        assert_eq!(ack.session_id, active.session_id);
        assert_eq!(ack.seq, receipt.body.seq);

        let mut wrong_seq = frame.clone();
        wrong_seq["seq"] = json!(receipt.body.seq + 1);
        assert!(provider_session_receipt_ack_from_frame(&wrong_seq, &active, &receipt).is_err());

        let mut wrong_sig = frame;
        wrong_sig["user_sig"] = json!("11".repeat(64));
        assert!(provider_session_receipt_ack_from_frame(&wrong_sig, &active, &receipt).is_err());
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
    fn provider_engine_load_config_wires_trt_admin_artifact() {
        let root = "aa".repeat(32);
        let mut catalog = test_catalog(&root);
        let mut trt_artifact = catalog.models[0].artifacts["gguf-q4_k_m"].clone();
        trt_artifact.engine = "trt-llm".to_owned();
        trt_artifact.path = "checkpoint.nvfp4.safetensors".to_owned();
        trt_artifact.min_compute_cap = Some("10.0".to_owned());
        trt_artifact.notes = Some("NVFP4 Blackwell checkpoint".to_owned());
        catalog.models[0]
            .artifacts
            .insert("nvfp4".to_owned(), trt_artifact);
        catalog.models[0].requirements.backends = vec!["trt-llm".to_owned()];

        let mut contract = test_contract(&root);
        contract.enclaves[0].backend = "trt-llm".to_owned();
        contract.enclaves[0].artifact_source.path = "checkpoint.nvfp4.safetensors".to_owned();
        let hardware = test_hardware(FixtureProfile::LinuxNvidia);
        let args = test_provider_start_args();
        let selected = build_provider_candidates(&contract, &catalog, &hardware, &args)
            .unwrap()
            .remove(0);
        let config = provider_engine_load_config(
            &selected,
            Path::new("/tmp/admin-approved-checkpoint.safetensors"),
        )
        .unwrap();

        assert_eq!(selected.artifact_name, "nvfp4");
        assert_eq!(
            config.artifact.format,
            mayhem_engine::ArtifactFormat::TensorRtLlmCheckpoint
        );
        assert_eq!(config.trt_tensor_parallel, Some(1));
        assert_eq!(config.trt_kv_cache_dtype, None);
        assert!(config.trt_require_engine_dir);
        assert_eq!(
            config.trt_engine_dir,
            Some(PathBuf::from("/tmp/.trtllm-engines/nvfp4"))
        );
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

    #[tokio::test]
    async fn provider_local_artifact_must_match_admin_sha256_when_present() {
        let temp = env::temp_dir().join(format!(
            "mayhem-cli-local-artifact-sha-{}-{}",
            std::process::id(),
            unix_epoch_millis().unwrap()
        ));
        fs::create_dir_all(&temp).unwrap();
        let source = temp.join("artifact.gguf");
        fs::write(&source, b"admin-approved artifact bytes").unwrap();
        let root = build_merkle_manifest(&source, 8).unwrap().root;
        let mut catalog = test_catalog(&root);
        catalog
            .models
            .first_mut()
            .unwrap()
            .artifacts
            .get_mut("gguf-q4_k_m")
            .unwrap()
            .source_sha256 = Some("00".repeat(32));
        let hardware = test_hardware(FixtureProfile::CpuOnly);
        let mut contract = test_contract(&root);
        contract.enclaves[0].source_sha256 = Some("00".repeat(32));
        let mut args = test_provider_start_args();
        args.artifact = Some(source.clone());
        args.chunk_size = 8;
        let selected =
            build_provider_candidates(&contract, &catalog, &hardware, &args).unwrap()[0].clone();

        let err = download_provider_artifact(&args, &temp.join("downloads-bad"), &selected)
            .await
            .expect_err("wrong catalog source_sha256 must be rejected");
        assert!(
            format!("{err:#}").contains("artifact sha256 mismatch"),
            "{err:#}"
        );

        let _ = fs::remove_dir_all(temp);
    }

    #[tokio::test]
    async fn provider_serving_requires_full_admin_merkle_artifact_root() {
        let temp = env::temp_dir().join(format!(
            "mayhem-cli-local-artifact-kind-{}-{}",
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
        args.artifact = Some(source);
        args.chunk_size = 8;
        let mut selected =
            build_provider_candidates(&contract, &catalog, &hardware, &args).unwrap()[0].clone();
        selected.artifact.artifact_root_kind = "blake3_descriptor_until_p2_4".to_owned();

        let err = download_provider_artifact(&args, &temp.join("downloads-bad"), &selected)
            .await
            .expect_err("descriptor roots are not provider-serving artifacts");
        assert!(
            format!("{err:#}").contains("requires admin artifact_root_kind blake3_merkle_v1"),
            "{err:#}"
        );

        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn provider_rooms_auto_selects_only_open_matching_admin_rooms() {
        let root = "aa".repeat(32);
        let contract = test_contract(&root);
        let enclave = &contract.enclaves[0];
        let mut canonical_rooms = contract.rooms.clone();
        let other_model_room_id = "cc".repeat(16);
        let other_enclave_room_id = "dd".repeat(16);
        let provider_created_room_id = "ee".repeat(16);
        canonical_rooms.push(LedgerRoom {
            room_id: other_enclave_room_id.clone(),
            sidechannel: format!("mx/room/{other_enclave_room_id}"),
            enclave_id: Some("66".repeat(32)),
            model_id: enclave.model_id.clone(),
            label: "other-enclave".to_owned(),
            status: "open".to_owned(),
            creator_role: Some("admin".to_owned()),
        });
        canonical_rooms.push(LedgerRoom {
            room_id: provider_created_room_id.clone(),
            sidechannel: format!("mx/room/{provider_created_room_id}"),
            enclave_id: Some(enclave.enclave_id.clone()),
            model_id: enclave.model_id.clone(),
            label: "provider-created".to_owned(),
            status: "open".to_owned(),
            creator_role: Some("provider".to_owned()),
        });
        canonical_rooms.push(LedgerRoom {
            room_id: "provider-local-only".to_owned(),
            sidechannel: "mx/room/provider-local-only".to_owned(),
            enclave_id: Some(enclave.enclave_id.clone()),
            model_id: enclave.model_id.clone(),
            label: "raw-intercom".to_owned(),
            status: "open".to_owned(),
            creator_role: Some("admin".to_owned()),
        });
        let rooms = select_provider_rooms(&canonical_rooms, enclave, "auto").unwrap();

        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].room_id, "aa".repeat(16));
        assert!(select_provider_rooms(&canonical_rooms, enclave, &other_model_room_id).is_err());
        assert!(select_provider_rooms(&canonical_rooms, enclave, &other_enclave_room_id).is_err());
        let err = select_provider_rooms(&canonical_rooms, enclave, &provider_created_room_id)
            .unwrap_err();
        assert!(format!("{err:#}").contains("creator_role"));
        let err =
            select_provider_rooms(&canonical_rooms, enclave, "provider-local-only").unwrap_err();
        assert!(format!("{err:#}").contains("not a canonical contract room"));
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
                "copy_paste": {
                    "checkout_url": "https://checkout.stripe.com/c/pay/cs_test"
                },
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
                "copy_paste": {
                    "checkout_url": "https://commerce.coinbase.com/charges/CBTEST"
                },
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
        assert!(checkout_from_paygate_response(
            PayRail::Stripe,
            &json!({
                "checkout_session": {
                    "id": "cs_test",
                    "url": "javascript:alert(1)"
                }
            }),
        )
        .is_err());
        assert!(checkout_from_paygate_response(
            PayRail::Coinbase,
            &json!({
                "charge": {
                    "id": "charge_test",
                    "hosted_url": "file:///tmp/mayhem-checkout.html"
                }
            }),
        )
        .is_err());
        assert!(checkout_from_paygate_response(
            PayRail::Stripe,
            &json!({
                "checkout_session": {
                    "id": "cs_test",
                    "url": "https://checkout.stripe.com.evil.example/c/pay/cs_test"
                }
            }),
        )
        .is_err());
        assert!(checkout_from_paygate_response(
            PayRail::Coinbase,
            &json!({
                "copy_paste": {
                    "checkout_url": "https://commerce.coinbase.com/charges/OTHER"
                },
                "charge": {
                    "id": "charge_test",
                    "hosted_url": "https://commerce.coinbase.com/charges/CBTEST"
                }
            }),
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

        assert_eq!(lines[0], "Mayhem stripe checkout for 10.00 USD");
        let eur_lines = checkout_handoff_lines_with_currency(
            PayRail::Stripe,
            10_000_000,
            "eur",
            "https://checkout.stripe.com/c/pay/cs_test",
        );
        assert_eq!(eur_lines[0], "Mayhem stripe checkout for 10.00 EUR");
        assert_eq!(
            lines[1],
            "Copy/paste checkout URL: https://checkout.stripe.com/c/pay/cs_test"
        );
        assert_eq!(
            checkout_copy_paste_value("https://checkout.stripe.com/c/pay/cs_test"),
            json!({
                "checkout_url": "https://checkout.stripe.com/c/pay/cs_test",
            })
        );
    }

    #[test]
    fn pay_tnk_helpers_prepare_memo_bound_transfer() {
        let tnk_e18 = mu_to_tnk_e18_ceil_u128(10_000_000, 50_000).unwrap();
        assert_eq!(tnk_e18, 200 * TNK_E18);
        assert_eq!(tnk_e18_to_decimal(tnk_e18), "200");
        assert_eq!(tnk_e18_to_decimal(1_234_567_890_000_000_000), "1.23456789");
        assert_eq!(tnk_e18_to_mu_floor(tnk_e18, 50_000).unwrap(), 10_000_000);

        let pubkey = "00".repeat(32);
        let nonce = "11".repeat(32);
        let memo_hash = derive_tnk_memo_hash(&pubkey, &nonce).unwrap();
        let mut expected = blake3::Hasher::new();
        expected.update(&[0_u8; 32]);
        expected.update(&[0x11_u8; 32]);
        assert_eq!(memo_hash, expected.finalize().to_hex().to_string());

        assert_eq!(
            pay_tnk_deposit_intent_payload(
                &memo_hash,
                "testtrac1treasury",
                tnk_e18,
                10_000_000,
                &PayTnkRate {
                    tnk_usd_e6: 50_000,
                    source: "gate-spot".to_owned(),
                    ts: Some(3_600),
                },
            ),
            json!({
                "op": "deposit_tnk",
                "memo_hash": memo_hash,
                "treasury_address": "testtrac1treasury",
                "tnk_e18": "200000000000000000000",
                "quoted_mu": 10_000_000,
                "rate_tnk_usd_e6": 50_000,
                "rate_source": "gate-spot",
            })
        );
    }

    #[test]
    fn pay_tnk_copy_paste_command_is_replayable() {
        let command = pay_tnk_deposit_intent_command(
            "10.25",
            "testtrac1treasury",
            &"ab".repeat(32),
            50_000,
            Some("http://127.0.0.1:49223/v1"),
        );

        assert!(command.starts_with("mayhem pay tnk "));
        assert!(command.contains("--amount '10.25'"));
        assert!(command.contains("--treasury-address 'testtrac1treasury'"));
        assert!(command.contains("--tnk-usd-e6 50000"));
        assert!(command.contains(" --submit-intent"));
        assert!(command.ends_with(" --rpc-url 'http://127.0.0.1:49223/v1'"));
    }

    #[test]
    fn pay_tnk_treasury_address_resolves_from_config() {
        let config = MayhemConfig {
            identity: None,
            network: Some(ConfigNetwork {
                rpc_url: None,
                sc_bridge_url: None,
                sc_bridge_token: None,
                gateway_url: None,
                paygate_url: None,
                tnk_treasury_address: Some("testtrac1treasury".to_owned()),
            }),
            provider: None,
            role: None,
        };

        assert_eq!(
            resolve_cli_tnk_treasury_address(Some(&config), None).unwrap(),
            "testtrac1treasury"
        );
        assert!(resolve_cli_tnk_treasury_address(Some(&config), Some("bad address")).is_err());
    }

    #[test]
    fn pay_tnk_msb_network_resolves_from_treasury_address_or_override() {
        assert_eq!(
            resolve_tnk_msb_network(None, "testtrac1treasury").unwrap(),
            "testnet1"
        );
        assert_eq!(
            resolve_tnk_msb_network(None, "trac1treasury").unwrap(),
            "mainnet"
        );
        assert_eq!(
            resolve_tnk_msb_network(Some("testnet"), "trac1treasury").unwrap(),
            "testnet1"
        );
        assert!(resolve_tnk_msb_network(None, "not-an-address").is_err());
    }

    #[test]
    fn pay_tnk_msb_store_resolves_from_wallet_keypair_path() {
        let path = Path::new("/tmp/mayhem/stores/testnet-epoch-wallet/db/keypair.json");
        let (stores_directory, store_name) = msb_store_from_keypair_path(path).unwrap();

        assert_eq!(stores_directory, PathBuf::from("/tmp/mayhem/stores"));
        assert_eq!(store_name, "testnet-epoch-wallet");
        assert!(msb_store_from_keypair_path(Path::new("/tmp/keypair.json")).is_err());
        assert!(msb_store_from_keypair_path(Path::new("/tmp/store/keypair.json")).is_err());
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
                tnk_treasury_address: None,
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
    fn calibration_match_guard_accepts_matching_catalog_fingerprint() {
        let report = test_calibration_report("aa".repeat(32), Some("aa".repeat(32)));

        ensure_calibration_matches_catalog(&report).unwrap();
    }

    #[test]
    fn calibration_match_guard_rejects_catalog_mismatch() {
        let report = test_calibration_report("aa".repeat(32), Some("bb".repeat(32)));
        let err = ensure_calibration_matches_catalog(&report).unwrap_err();

        assert!(err
            .to_string()
            .contains("does not match catalog fingerprint"));
    }

    #[test]
    fn calibration_match_guard_rejects_missing_catalog_fingerprint() {
        let report = test_calibration_report("aa".repeat(32), None);
        let err = ensure_calibration_matches_catalog(&report).unwrap_err();

        assert!(err.to_string().contains("catalog has no fingerprint"));
    }

    #[test]
    fn canary_matrix_covers_launch_artifacts_and_marks_hardware_gated_backends() {
        let mut catalog = test_catalog(&"aa".repeat(32));
        catalog.models[0].tier = "launch".to_owned();
        catalog.models[0].canary.set_id = "test-canary-zero".to_owned();
        let mut trt_artifact = catalog.models[0].artifacts["gguf-q4_k_m"].clone();
        trt_artifact.engine = "trt-llm".to_owned();
        catalog.models[0]
            .artifacts
            .insert("nvfp4".to_owned(), trt_artifact);
        catalog.models[0]
            .canary
            .fingerprints
            .insert("gguf-q4_k_m".to_owned(), "aa".repeat(32));
        catalog.models[0]
            .canary
            .fingerprints
            .insert("nvfp4".to_owned(), "bb".repeat(32));
        let canaries_dir = test_canary_dir("test-canary-zero", 0.0);

        let report = catalog_canary_matrix_report(
            &catalog,
            PathBuf::from("catalog.json"),
            canaries_dir,
            true,
        );

        assert!(report.ok, "{:?}", report.errors);
        assert_eq!(report.model_count, 1);
        assert_eq!(report.artifact_count, 2);
        assert!(report.entries.iter().any(|entry| {
            entry.artifact == "nvfp4" && entry.calibration_status == "hardware-gated-calibration"
        }));
    }

    #[test]
    fn canary_matrix_detects_missing_backend_fingerprint_and_nonzero_prompt_temperature() {
        let mut catalog = test_catalog(&"aa".repeat(32));
        catalog.models[0].tier = "launch".to_owned();
        catalog.models[0].canary.set_id = "test-canary-hot".to_owned();
        catalog.models[0]
            .canary
            .fingerprints
            .insert("gguf-q4_k_m".to_owned(), "aa".repeat(32));
        let mut mlx_artifact = catalog.models[0].artifacts["gguf-q4_k_m"].clone();
        mlx_artifact.engine = "mlx".to_owned();
        catalog.models[0]
            .artifacts
            .insert("mlx-4bit".to_owned(), mlx_artifact);
        let canaries_dir = test_canary_dir("test-canary-hot", 0.7);

        let report = catalog_canary_matrix_report(
            &catalog,
            PathBuf::from("catalog.json"),
            canaries_dir,
            true,
        );

        assert!(!report.ok);
        assert!(report
            .errors
            .iter()
            .any(|error| error.contains("missing artifact mlx-4bit")));
        assert!(report
            .errors
            .iter()
            .any(|error| error.contains("must use temperature 0")));
    }

    #[test]
    fn stable_value_hash_is_object_key_order_independent() {
        let a = json!({ "b": 2, "a": { "d": 4, "c": 3 } });
        let b = json!({ "a": { "c": 3, "d": 4 }, "b": 2 });

        assert_eq!(stable_value_hash(&a), stable_value_hash(&b));
    }

    #[test]
    fn canary_probe_command_requires_paid_session_receipt_hash() {
        let command = canary_probe_command(test_canary_probe_command_input("rr".repeat(32)));

        assert_eq!(command["op"], "probe_result");
        assert_eq!(command["enclave_id"], "enclave");
        assert_eq!(command["session_receipt_hash"], "rr".repeat(32));
        assert_eq!(command["evidence_hash"], "ee".repeat(32));
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
        assert!(!report.default_model_added);
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
    fn opencode_merge_sets_default_model_only_when_absent() {
        let path = env::temp_dir().join(format!(
            "mayhem-opencode-new-{}-{}.json",
            std::process::id(),
            now_millis_for_path()
        ));
        let models = vec![json!({
            "id": "mayhem/default-model",
            "mayhem": {
                "caps": { "tools": true, "json": true, "ctx": 8192 }
            }
        })];

        let report =
            merge_mayhem_opencode_config(&path, "http://127.0.0.1:11435", Some(&models), true)
                .unwrap();
        let merged: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

        assert!(report.created);
        assert!(report.default_model_added);
        assert_eq!(merged["model"], "mayhem/mayhem/default-model");
        assert_eq!(
            merged["provider"]["mayhem"]["models"]["mayhem/default-model"]["limit"]["context"],
            8192
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

    fn test_admin_tx_args() -> AdminTxArgs {
        AdminTxArgs {
            home: None,
            rpc_url: None,
            peer_store_name: "main".to_owned(),
            wallet_password: None,
            submit: false,
            sim: false,
            json: true,
        }
    }

    fn test_payout_confirm_args() -> AdminPayoutConfirmArgs {
        AdminPayoutConfirmArgs {
            tx: test_admin_tx_args(),
            kind: AdminPayoutConfirmKind::Provider,
            rail: AdminPayoutMethod::Tnk,
            epoch: 7,
            who: "provider-a".to_owned(),
            mu: 1_000_000,
            tnk_e18: Some("500000000000000000".to_owned()),
            msb_tx_hash: Some("msb-tx".to_owned()),
            external_ref: None,
            fiat_currency: None,
            fiat_amount_minor: None,
            at: 25_200,
        }
    }

    fn test_provider_start_args() -> ProviderStartArgs {
        ProviderStartArgs {
            home: None,
            enclave: None,
            rooms: "auto".to_owned(),
            rpc_url: None,
            session_rpc_url: None,
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

    #[test]
    fn release_provider_serving_requires_signed_catalog_and_real_responder() {
        let mut args = test_provider_start_args();
        args.serve_sessions = true;
        args.dev_skip_catalog_verify = true;
        args.dev_session_shim = false;
        let err = validate_provider_start_security_mode(&args, false)
            .expect_err("release serving must reject unsigned catalogs");
        assert!(
            format!("{err:#}").contains("requires a signed admin catalog"),
            "{err:#}"
        );

        args.dev_session_shim = true;
        let err = validate_provider_start_security_mode(&args, false)
            .expect_err("release serving must reject fake responders");
        assert!(format!("{err:#}").contains("debug-build only"), "{err:#}");

        assert!(
            validate_provider_start_security_mode(&args, true).is_ok(),
            "debug smokes may still use the unsigned deterministic shim"
        );

        args.dev_session_shim = false;
        args.dev_skip_catalog_verify = false;
        assert!(
            validate_provider_start_security_mode(&args, false).is_ok(),
            "release serving with a signed catalog is allowed"
        );
    }

    fn test_provider_session_terms() -> ProviderSessionTerms {
        ProviderSessionTerms {
            provider: "55".repeat(32),
            enclave_id: "11".repeat(32),
            model_id: "test/model@4bit".to_owned(),
            room_ids: vec!["aa".repeat(16)],
            price_ver: 1,
            in_per_1k_mu: 1,
            out_per_1k_mu: 2,
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

    fn test_hex_decode(value: &str) -> Vec<u8> {
        assert_eq!(value.len() % 2, 0);
        value
            .as_bytes()
            .chunks(2)
            .map(|chunk| {
                let high = (chunk[0] as char).to_digit(16).unwrap();
                let low = (chunk[1] as char).to_digit(16).unwrap();
                ((high << 4) | low) as u8
            })
            .collect()
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

    fn test_calibration_report(
        fingerprint: String,
        existing: Option<String>,
    ) -> CatalogCanaryCalibrationReport {
        CatalogCanaryCalibrationReport {
            model_id: "test/model".to_owned(),
            artifact: "gguf-q4_k_m".to_owned(),
            engine: "llama.cpp".to_owned(),
            artifact_path: PathBuf::from("model.gguf"),
            canary_set: "test-canary".to_owned(),
            prompt_count: 1,
            matches_existing_catalog: existing.as_ref().map(|existing| existing == &fingerprint),
            existing_catalog_fingerprint: existing,
            catalog_fingerprint: fingerprint,
            prompts: vec![test_calibration_prompt("p1", "aa".repeat(32))],
        }
    }

    fn test_canary_dir(set_id: &str, temperature: f64) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let safe_set_id = set_id.replace(|c: char| !c.is_ascii_alphanumeric(), "-");
        let dir = std::env::temp_dir().join(format!(
            "mayhem-canary-matrix-{}-{}-{}",
            std::process::id(),
            safe_set_id,
            nanos
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{set_id}.json")),
            json!({
                "set_id": set_id,
                "prompts": [{
                    "id": "p1",
                    "messages": [{ "role": "user", "content": "calibrate" }],
                    "temperature": temperature,
                    "max_tokens": 8
                }]
            })
            .to_string(),
        )
        .unwrap();
        dir
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
        let room_id = "aa".repeat(16);
        let closed_room_id = "bb".repeat(16);
        let other_room_id = "cc".repeat(16);
        let enclave = LedgerEnclave {
            enclave_id: "11".repeat(32),
            model_id: "test/model@4bit".to_owned(),
            backend: "llama.cpp".to_owned(),
            artifact_root: root.to_owned(),
            artifact_root_kind: "blake3_merkle_v1".to_owned(),
            artifact_source: LedgerArtifactSource {
                kind: "huggingface".to_owned(),
                repo: "test/model".to_owned(),
                revision: "1".repeat(40),
                path: "model.gguf".to_owned(),
            },
            source_sha256: None,
            manifest_hash: "22".repeat(32),
            att_tier: 1,
            binary_hash: "33".repeat(32),
            caps: json!({ "tools": true, "json": true, "ctx": 8192 }),
            status: "active".to_owned(),
            created_by: "44".repeat(32),
            created_by_role: Some("admin".to_owned()),
        };
        ContractCatalog {
            enclaves: vec![enclave],
            rooms: vec![
                LedgerRoom {
                    room_id: room_id.clone(),
                    sidechannel: format!("mx/room/{room_id}"),
                    enclave_id: Some("11".repeat(32)),
                    model_id: "test/model@4bit".to_owned(),
                    label: "test".to_owned(),
                    status: "open".to_owned(),
                    creator_role: Some("admin".to_owned()),
                },
                LedgerRoom {
                    room_id: closed_room_id.clone(),
                    sidechannel: format!("mx/room/{closed_room_id}"),
                    enclave_id: Some("11".repeat(32)),
                    model_id: "test/model@4bit".to_owned(),
                    label: "test".to_owned(),
                    status: "closed".to_owned(),
                    creator_role: Some("admin".to_owned()),
                },
                LedgerRoom {
                    room_id: other_room_id.clone(),
                    sidechannel: format!("mx/room/{other_room_id}"),
                    enclave_id: None,
                    model_id: "other/model".to_owned(),
                    label: "test".to_owned(),
                    status: "open".to_owned(),
                    creator_role: Some("admin".to_owned()),
                },
            ],
            roomserve: vec![LedgerRoomServe {
                room_id: room_id.clone(),
                provider: "55".repeat(32),
                enclave_id: "11".repeat(32),
                model_id: "test/model@4bit".to_owned(),
                status: "active".to_owned(),
            }],
            serves: vec![LedgerServe {
                provider: "55".repeat(32),
                enclave_id: "11".repeat(32),
                model_id: "test/model@4bit".to_owned(),
                status: "active".to_owned(),
                rooms: vec![room_id],
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
                    set_by_role: Some("admin".to_owned()),
                }),
                pending: None,
            }],
            rules: Some(RulesRef {
                ver: 3,
                hash: "99".repeat(32),
            }),
        }
    }

    fn test_canary_probe_command_input(session_receipt_hash: String) -> CanaryProbeCommandInput {
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
