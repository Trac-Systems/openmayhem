//! Explicit gateway buyer provisioning and startup. Public requests cannot choose
//! wallet, transport, decoder executable, retained stores or settlement policy.
use anyhow::{anyhow, ensure, Context, Result};
use clap::Args;
use ed25519_dalek::SigningKey;
use mayhem_bridge::ScBridgeConfig;
use mayhem_gateway::openai::{
    proxy_buyer::Runtime, proxy_request, GatewayAccessControl, GatewayKeyBudgetLimits,
};
use mayhem_proto::proxy::finance::ProxySettlementPolicy;
use mayhem_proxy::{
    attempts::{Digest, Identity},
    buyer_controller::{Controller, Limits as ControllerLimits},
    connector::config::private_file,
    discovery, endpoint,
    financial::{
        self,
        negotiation::{BuyerNegotiation, Limits as NegotiationLimits, Store as NegotiationStore},
        quote::Lifetimes,
        recovery::{BuyerRecovery, Limits as RecoveryLimits, Store as RecoveryStore},
    },
    signing::Authority,
    worker::host::{Pool, PoolLimits},
};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

const NEGOTIATION: &str = "buyer-negotiation.redb";
const RECOVERY: &str = "buyer-recovery.redb";
const BUDGET: &str = "gateway-key-budget.redb";
const ACTIVATION: &str = "gateway-key-budget.json";

/// Held throughout serving and provisioning so importing legacy counters cannot
/// race another current-version gateway's admissions or final receipt writes.
pub fn budget_owner_lock(home: &Path, migration: bool) -> Result<std::fs::File> {
    #[cfg(unix)]
    {
        use rustix::fs::{fstat, open, FileType, Mode, OFlags};
        use std::os::unix::fs::DirBuilderExt;
        if !home.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(home)
                .map_err(|_| anyhow!("gateway home unavailable"))?;
        }
        let fd = open(
            home.join("gateway-key-budget.lock"),
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| anyhow!("gateway budget ownership lock unavailable"))?;
        let m = fstat(&fd).map_err(|_| anyhow!("gateway budget ownership lock unavailable"))?;
        ensure!(
            FileType::from_raw_mode(m.st_mode) == FileType::RegularFile
                && m.st_uid == rustix::process::geteuid().as_raw()
                && m.st_mode & 0o077 == 0
                && m.st_nlink == 1,
            "gateway budget ownership lock protection failed"
        );
        let file: std::fs::File = fd.into();
        if migration {
            fs2::FileExt::try_lock_exclusive(&file)
        } else {
            fs2::FileExt::try_lock_shared(&file)
        }
        .map_err(|_| anyhow!("gateway is serving or buyer provisioning is active for this home"))?;
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        // Native serving retains its existing platform support. Paid buyer
        // provisioning still rejects platforms without the file ACL checks.
        std::fs::create_dir_all(home)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(home.join("gateway-key-budget.lock"))?;
        if migration {
            fs2::FileExt::try_lock_exclusive(&file)
        } else {
            fs2::FileExt::try_lock_shared(&file)
        }
        .map_err(|_| anyhow!("gateway budget migration already active"))?;
        Ok(file)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetActivation {
    schema_version: u32,
    pub path: PathBuf,
    max_tokens: u64,
    max_reservations: u64,
}
impl BudgetActivation {
    pub fn limits(&self) -> GatewayKeyBudgetLimits {
        GatewayKeyBudgetLimits {
            max_tokens: self.max_tokens,
            max_reservations: self.max_reservations,
        }
    }
    pub fn matches(&self, prepared: &Prepared) -> bool {
        self.path == prepared.budget_path
            && self.max_tokens == prepared.budget_limits.max_tokens
            && self.max_reservations == prepared.budget_limits.max_reservations
    }
}

/// Startup-only downgrade fence. Even a native-only restart must reopen the
/// authority once provisioned. Missing either half of migration fails closed.
pub fn budget_activation(
    home: &Path,
    tokens: &mayhem_gateway::openai::GatewayTokenStore,
) -> Result<Option<BudgetActivation>> {
    let path = home.join(ACTIVATION);
    if matches!(std::fs::symlink_metadata(&path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
    {
        ensure!(
            !tokens.requires_durable_key_budget(),
            "durable gateway key budget activation is missing"
        );
        return Ok(None);
    }
    ensure!(
        tokens.version == 2,
        "gateway key budget migration is incomplete or unsupported"
    );
    let bytes = private_file(&path, 16 * 1024)
        .map_err(|_| anyhow!("gateway key budget activation protection failed"))?;
    let activation: BudgetActivation = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow!("invalid gateway key budget activation"))?;
    ensure!(
        activation.schema_version == 1
            && activation.path.is_absolute()
            && (1..=1_000_000).contains(&activation.max_tokens)
            && (1..=1_000_000).contains(&activation.max_reservations),
        "invalid gateway key budget activation"
    );
    Ok(Some(activation))
}

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
    /// Import the existing gateway token spending counters exactly once.
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    schema_version: u32,
    network: discovery::Identity,
    buyer_pubkey: Digest,
    peer_rpc_url: String,
    /// Optional wallet-owning peer for authenticated proxy finance. The main
    /// gateway peer remains the canonical discovery/native read source.
    #[serde(default)]
    financial_rpc_url: Option<String>,
    state_dir: PathBuf,
    worker_program: PathBuf,
    bridge: mayhem_gateway::openai::proxy_control::Bridge,
    worker: PoolLimits,
    protocol: endpoint::Limits,
    sessions: usize,
    buffer_bytes: usize,
    message_bytes: usize,
    control_wait_ms: u64,
    financial_reads: usize,
    storage_workers: usize,
    max_records: u64,
    max_payload_bytes: u64,
    max_record_bytes: usize,
    closed_retention_ms: u64,
    max_budget_tokens: u64,
    max_budget_reservations: u64,
    policy_revision: Digest,
    settlement_policy: ProxySettlementPolicy,
    acceptance_epochs: u64,
    reservation_epochs: u64,
    receipt_grace_epochs: u64,
    #[serde(default)]
    retail_authorization: Option<mayhem_gateway::openai::proxy_buyer::RetailAuthorizationConfig>,
    #[serde(default)]
    profile_resolution: Option<mayhem_gateway::openai::proxy_buyer::ProfileResolutionLimits>,
}

impl Config {
    fn load(path: &Path) -> Result<Self> {
        let bytes = private_file(path, 64 * 1024)
            .map_err(|_| anyhow!("proxy buyer configuration protection failed"))?;
        let mut config: Self = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow!("invalid proxy buyer configuration"))?;
        ensure!(
            config.schema_version == 1,
            "unsupported proxy buyer configuration"
        );
        config
            .network
            .validate()
            .map_err(|_| anyhow!("invalid proxy buyer network"))?;
        ensure!(
            config.network.contract_version == mayhem_proto::CONTRACT_VERSION,
            "unsupported proxy buyer contract"
        );
        ensure!(
            (1..=64).contains(&config.sessions)
                && (1..=64).contains(&config.storage_workers)
                && (1..=64).contains(&config.financial_reads)
                && (1..=60_000).contains(&config.control_wait_ms)
                && (1..=1_000_000).contains(&config.max_records)
                && (1..=300 * 1024 * 1024).contains(&config.max_record_bytes)
                && config.max_payload_bytes >= config.max_record_bytes as u64
                && config.closed_retention_ms > 0
                && config.closed_retention_ms <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER
                && (1..=1_000_000).contains(&config.max_budget_tokens)
                && (1..=1_000_000).contains(&config.max_budget_reservations),
            "invalid proxy buyer resource limits"
        );
        config.policy()?;
        if let Some(resolution) = &config.profile_resolution {
            resolution.validate().map_err(anyhow::Error::msg)?;
        }
        if let Some(retail) = &config.retail_authorization {
            retail.validate().map_err(anyhow::Error::msg)?;
        }
        let parent = std::fs::canonicalize(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )
        .map_err(|_| anyhow!("proxy buyer configuration protection failed"))?;
        for path in [
            &mut config.state_dir,
            &mut config.worker_program,
            &mut config.bridge.token_file,
        ] {
            if path.is_relative() {
                *path = parent.join(&*path);
            }
        }
        Ok(config)
    }
    fn identity(&self) -> Result<Identity> {
        Ok(Identity {
            network_id: self.network.network_id.clone(),
            msb_bootstrap: Digest::new(&self.network.msb_bootstrap)?,
            subnet_bootstrap: Digest::new(&self.network.subnet_bootstrap)?,
            controller_pubkey: self.buyer_pubkey.clone(),
        })
    }
    fn financial_client(&self) -> Result<financial::Client> {
        financial::Client::new(
            self.financial_rpc_url
                .as_deref()
                .unwrap_or(&self.peer_rpc_url),
            self.network.clone(),
            self.buyer_pubkey.as_str().to_owned(),
            self.financial_reads,
        )
        .map_err(|_| anyhow!("invalid proxy buyer financial client"))
    }
    fn policy(&self) -> Result<proxy_request::Policy> {
        let hash = self
            .settlement_policy
            .digest()
            .map_err(|_| anyhow!("invalid proxy settlement policy"))?;
        proxy_request::Policy::new(
            self.policy_revision.clone(),
            Digest::new(hash)?,
            Lifetimes {
                acceptance_epochs: self.acceptance_epochs,
                reservation_epochs: self.reservation_epochs,
                receipt_grace_epochs: self.receipt_grace_epochs,
            },
            self.protocol.request_bytes,
        )
        .map_err(|_| anyhow!("invalid proxy buyer request policy"))
    }
    fn negotiation_limits(&self) -> NegotiationLimits {
        NegotiationLimits {
            max_records: self.max_records,
            max_payload_bytes: self.max_payload_bytes,
            max_record_bytes: self.max_record_bytes,
            closed_retention_ms: self.closed_retention_ms,
        }
    }
    fn recovery_limits(&self) -> RecoveryLimits {
        RecoveryLimits {
            max_records: self.max_records,
            closed_retention_ms: self.closed_retention_ms,
        }
    }
    fn budget_limits(&self) -> GatewayKeyBudgetLimits {
        GatewayKeyBudgetLimits {
            max_tokens: self.max_budget_tokens,
            max_reservations: self.max_budget_reservations,
        }
    }
    fn bridge(&self) -> Result<ScBridgeConfig> {
        let b = &self.bridge;
        ensure!(
            (1..=60_000).contains(&b.operation_timeout_ms)
                && (64 * 1024..=256 * 1024 * 1024).contains(&b.frame_bytes)
                && (1..=4096).contains(&b.queue_events)
                && b.queue_bytes >= b.frame_bytes
                && b.queue_bytes <= 256 * 1024 * 1024,
            "invalid proxy buyer bridge limits"
        );
        let token = private_file(&b.token_file, 8192)
            .map_err(|_| anyhow!("proxy buyer bridge credential protection failed"))?;
        let token = std::str::from_utf8(&token)
            .map_err(|_| anyhow!("invalid proxy buyer bridge credential"))?
            .trim();
        ensure!(
            !token.is_empty() && token.bytes().all(|v| v.is_ascii_graphic()),
            "invalid proxy buyer bridge credential"
        );
        let mut bridge = ScBridgeConfig::new(&b.url, token.to_owned())
            .map_err(|_| anyhow!("invalid proxy buyer bridge configuration"))?;
        ensure!(
            bridge
                .url
                .host_str()
                .and_then(|s| s.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok())
                .is_some_and(|ip| ip.is_loopback())
                && matches!(bridge.url.scheme(), "ws" | "wss")
                && bridge.url.username().is_empty()
                && bridge.url.password().is_none()
                && bridge.url.query().is_none()
                && bridge.url.fragment().is_none(),
            "proxy buyer bridge must use a literal loopback address"
        );
        bridge.operation_deadline = Some(Duration::from_millis(b.operation_timeout_ms));
        bridge.max_message_bytes = b.frame_bytes;
        bridge.max_queued_events = b.queue_events;
        bridge.max_queued_bytes = b.queue_bytes;
        Ok(bridge)
    }
}

pub struct Prepared {
    pub runtime: Arc<Runtime>,
    pub budget_path: PathBuf,
    pub budget_limits: GatewayKeyBudgetLimits,
}

pub async fn prepare(
    path: PathBuf,
    expected: discovery::Identity,
    rpc_url: String,
    key: SigningKey,
) -> Result<Prepared> {
    tokio::task::spawn_blocking(move || {
        let config = Config::load(&path)?;
        ensure!(
            config.network == expected
                && config.peer_rpc_url.trim_end_matches('/') == rpc_url.trim_end_matches('/'),
            "proxy buyer differs from the canonical gateway network or peer"
        );
        let signer = Arc::new(
            Authority::from_unlocked_wallet(key, config.identity()?)
                .map_err(|_| anyhow!("proxy buyer wallet differs from gateway wallet"))?,
        );
        // Serving never treats loss of a durable journal as first initialization.
        private_dir(&config.state_dir, false)?;
        for name in [NEGOTIATION, RECOVERY, BUDGET] {
            existing_store(&config.state_dir.join(name))?;
        }
        let workdir = config.state_dir.join("decoder-work");
        private_dir(&workdir, false)?;
        let verifier = Arc::new(
            Pool::new(&config.worker_program, &workdir, config.worker)
                .map_err(|_| anyhow!("proxy buyer decoder protection or limits failed"))?,
        );
        let bridge = config.bridge()?;
        // Every financial observation still binds the configured canonical
        // network and buyer wallet. A separate peer is not a separate ledger,
        // payment identity, budget, or permission to trust upstream responses.
        let client = Arc::new(config.financial_client()?);
        let negotiation = Arc::new(BuyerNegotiation::new(
            Arc::new(NegotiationStore::open_existing(
                config.state_dir.join(NEGOTIATION),
                config.identity()?,
                config.negotiation_limits(),
            )?),
            client.clone(),
            config.storage_workers,
        )?);
        let recovery = Arc::new(BuyerRecovery::new(
            Arc::new(RecoveryStore::open_existing(
                config.state_dir.join(RECOVERY),
                config.identity()?,
                config.recovery_limits(),
            )?),
            client.clone(),
            config.storage_workers,
        )?);
        let controller = Arc::new(
            Controller::new(
                negotiation,
                recovery,
                client,
                signer,
                verifier,
                bridge,
                ControllerLimits {
                    sessions: config.sessions,
                    buffer_bytes: config.buffer_bytes,
                    protocol: config.protocol,
                    message_bytes: config.message_bytes,
                    control_wait: Duration::from_millis(config.control_wait_ms),
                },
            )
            .map_err(|_| anyhow!("proxy buyer controller limits or identity failed"))?,
        );
        let mut runtime = Runtime::new(
            controller,
            config.policy()?,
            config.settlement_policy.clone(),
            config.sessions,
        )
        .map_err(anyhow::Error::msg)?
        .with_profile_resolution_limits(config.profile_resolution.clone().unwrap_or_default())
        .map_err(anyhow::Error::msg)?;
        let budget_limits = config.budget_limits();
        if let Some(retail) = config.retail_authorization {
            runtime = runtime
                .with_retail_authorization(retail)
                .map_err(anyhow::Error::msg)?;
        }
        Ok(Prepared {
            runtime: Arc::new(runtime),
            budget_path: config.state_dir.join(BUDGET),
            budget_limits,
        })
    })
    .await
    .context("preparing explicit proxy buyer")?
}

/// An explicit operator provisioning action, never called by serving/recovery.
/// Partial initialization remains on disk and requires operator reconciliation.
pub async fn initialize(args: InitArgs) -> Result<()> {
    let home = super::absolutize(args.home.map(Ok).unwrap_or_else(super::default_home)?)?;
    tokio::task::spawn_blocking(move || {
        let _owner = budget_owner_lock(&home, true)?;
        let config = Config::load(&args.config)?;
        private_dir(&home, false)?;
        let token_path = super::gateway_token_store_path(&home);
        let mut tokens = super::read_gateway_token_store(&token_path)?;
        ensure!(!tokens.requires_durable_key_budget()
            && matches!(std::fs::symlink_metadata(home.join(ACTIVATION)), Err(e) if e.kind() == std::io::ErrorKind::NotFound),
            "gateway key budget authority is already activated or requires reconciliation");
        private_dir(&config.state_dir, true)?;
        private_dir(&config.state_dir.join("decoder-work"), true)?;
        for name in [NEGOTIATION, RECOVERY, BUDGET] {
            ensure!(matches!(std::fs::symlink_metadata(config.state_dir.join(name)), Err(e) if e.kind() == std::io::ErrorKind::NotFound),
                "proxy buyer stores already exist or cannot be inspected");
        }
        let _budget = GatewayAccessControl::new(true, tokens.clone(), Some(token_path.clone()))
            .initialize_durable_key_budget(config.state_dir.join(BUDGET), config.budget_limits()).map_err(anyhow::Error::msg)?;
        create_empty_store(&config.state_dir.join(NEGOTIATION))?;
        let _negotiation = NegotiationStore::open(config.state_dir.join(NEGOTIATION), config.identity()?, config.negotiation_limits())?;
        create_empty_store(&config.state_dir.join(RECOVERY))?;
        let _recovery = RecoveryStore::open(config.state_dir.join(RECOVERY), config.identity()?, config.recovery_limits())?;
        std::fs::File::open(&config.state_dir)?.sync_all()?;
        let activation = BudgetActivation { schema_version: 1, path: config.state_dir.join(BUDGET),
            max_tokens: config.max_budget_tokens, max_reservations: config.max_budget_reservations };
        create_private_bytes(&home.join(ACTIVATION), &serde_json::to_vec(&activation)?)?;
        std::fs::File::open(&home)?.sync_all()?;
        tokens.version = 2;
        replace_private_bytes(&token_path, &serde_json::to_vec_pretty(&tokens)?)?;
        Ok::<_, anyhow::Error>(())
    }).await.context("initializing explicit proxy buyer stores")??;
    println!("Proxy buyer stores initialized. No gateway, signing or paid request was started.");
    Ok(())
}

fn private_dir(path: &Path, create: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        if create {
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(path) {
                Ok(()) => {
                    std::fs::File::open(
                        path.parent().context("proxy buyer state parent missing")?,
                    )?
                    .sync_all()?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(_) => return Err(anyhow!("proxy buyer private directory unavailable")),
            }
        }
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|_| anyhow!("proxy buyer private directory unavailable"))?;
        ensure!(
            metadata.is_dir()
                && metadata.uid() == rustix::process::geteuid().as_raw()
                && metadata.mode() & 0o077 == 0,
            "proxy buyer directory protection failed"
        );
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, create);
        Err(anyhow!("proxy buyer file protection is unsupported"))
    }
}
fn existing_store(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use rustix::fs::{fstat, open, FileType, Mode, OFlags};
        let fd = open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| anyhow!("proxy buyer retained store is missing or protected"))?;
        let m =
            fstat(&fd).map_err(|_| anyhow!("proxy buyer retained store cannot be inspected"))?;
        ensure!(
            FileType::from_raw_mode(m.st_mode) == FileType::RegularFile
                && m.st_uid == rustix::process::geteuid().as_raw()
                && m.st_mode & 0o077 == 0
                && m.st_nlink == 1
                && m.st_size > 0,
            "proxy buyer retained store protection or contents failed"
        );
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(anyhow!("proxy buyer file protection is unsupported"))
    }
}
fn create_empty_store(path: &Path) -> Result<()> {
    create_private_bytes(path, &[])
}
fn create_private_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|_| anyhow!("proxy buyer private file already exists or cannot be created"))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, bytes);
        Err(anyhow!("proxy buyer file protection is unsupported"))
    }
}

fn replace_private_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut seed = [0u8; 8];
    getrandom::fill(&mut seed).map_err(|_| anyhow!("gateway key budget migration unavailable"))?;
    let parent = path
        .parent()
        .context("gateway token-store parent missing")?;
    let temporary = parent.join(format!(
        ".gateway-budget-migration-{}",
        u64::from_le_bytes(seed)
    ));
    create_private_bytes(&temporary, bytes)?;
    std::fs::rename(&temporary, path)
        .map_err(|_| anyhow!("gateway key budget migration commit failed"))?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests;
