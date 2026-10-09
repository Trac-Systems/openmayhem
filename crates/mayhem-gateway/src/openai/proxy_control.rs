//! Explicit proxy-only catalog/presence lifecycle. Construction creates no tasks;
//! the gateway owner runs and joins the returned lifecycle. Native discovery,
//! provider routing, payment policy and request dispatch remain independently owned.

use mayhem_bridge::ScBridgeConfig;
use mayhem_proxy::{
    attempts::Digest,
    catalog::Catalog,
    connector::config::private_file,
    discovery::{DiscoveryClient, Identity},
    presence::{self, gateway::Gateway, Table},
    registry::publication::{Limits as RegistryLimits, Reader as RegistryReader, TrustedOrigin},
    supervisor::{self, RefreshPolicy},
};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::watch;

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid proxy gateway configuration")]
    Configuration,
    #[error("proxy gateway network differs from the gateway owner")]
    Identity,
    #[error("proxy gateway requires protected local configuration and state files")]
    Protection,
    #[error("proxy gateway storage could not be opened or read")]
    Storage,
    #[error("proxy gateway background control stopped unexpectedly")]
    Supervisor,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub network: Identity,
    pub peer_rpc_url: String,
    pub state_dir: PathBuf,
    pub bridge: Bridge,
    pub max_markets: usize,
    pub max_presence_routes: u64,
    pub selected_markets: Vec<Digest>,
    #[serde(default = "refresh_policy")]
    pub refresh: RefreshPolicy,
    #[serde(default = "rpc_timeout_ms")]
    pub rpc_timeout_ms: u64,
    /// Administrative semantics only; never provider evidence or a request URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<RegistryConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conformance: Option<mayhem_proxy::conformance::Config>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryConfig {
    pub origin: String,
    /// Explicit local acceptance opt-in; HTTPS is otherwise mandatory.
    pub allow_loopback_http: bool,
}
impl RegistryConfig {
    fn reader(&self) -> Result<Arc<RegistryReader>> {
        let origin = if self.allow_loopback_http {
            TrustedOrigin::local_loopback_http(&self.origin)
        } else {
            TrustedOrigin::https(&self.origin)
        }
        .map_err(|_| Error::Configuration)?;
        RegistryReader::new(origin, RegistryLimits::default())
            .map(Arc::new)
            .map_err(|_| Error::Configuration)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bridge {
    pub url: String,
    pub token_file: PathBuf,
    pub operation_timeout_ms: u64,
    pub frame_bytes: usize,
    pub queue_events: usize,
    pub queue_bytes: usize,
}

fn rpc_timeout_ms() -> u64 {
    5_000
}

fn refresh_policy() -> RefreshPolicy {
    RefreshPolicy {
        interval_ms: 5_000,
        page_pause_ms: 10,
        retry_initial_ms: 500,
        retry_max_ms: 5_000,
        jitter_percent: 20,
    }
}

/// Protected, identity-checked startup material; not constructible from a public
/// request. Credentials never appear in Debug, public health or returned errors.
pub struct Prepared {
    config: Config,
    bridge: ScBridgeConfig,
    client: DiscoveryClient,
    seed: u64,
    registry: Option<Arc<RegistryReader>>,
}

impl Prepared {
    /// The expected identity comes from the gateway owner, not from this file.
    /// Loading validates everything before stores, sockets or tasks are created.
    pub fn load(path: &Path, expected_network: &Identity) -> Result<Self> {
        let bytes = private_file(path, 1024 * 1024).map_err(|_| Error::Protection)?;
        let mut config: Config =
            serde_json::from_slice(&bytes).map_err(|_| Error::Configuration)?;
        expected_network.validate().map_err(|_| Error::Identity)?;
        if &config.network != expected_network {
            return Err(Error::Identity);
        }
        if config.schema_version != 1
            || config.network.contract_version != mayhem_proto::CONTRACT_VERSION
            || config.max_markets == 0
            || config.max_presence_routes == 0
            || config.selected_markets.len() > config.max_markets
            || config.rpc_timeout_ms == 0
        {
            return Err(Error::Configuration);
        }
        config
            .refresh
            .validate()
            .map_err(|_| Error::Configuration)?;
        let latest_refresh = config.refresh.interval_ms.saturating_add(
            config.refresh.interval_ms * u64::from(config.refresh.jitter_percent) / 100,
        );
        if latest_refresh.saturating_add(config.rpc_timeout_ms) >= presence::CATALOG_AGE_MS {
            return Err(Error::Configuration);
        }
        let parent = std::fs::canonicalize(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )
        .map_err(|_| Error::Protection)?;
        for path in [&mut config.state_dir, &mut config.bridge.token_file] {
            if path.is_relative() {
                *path = parent.join(&*path);
            }
        }
        let rpc = url::Url::parse(&config.peer_rpc_url).map_err(|_| Error::Configuration)?;
        validate_local_url(&rpc, &["http", "https"])?;
        let url = url::Url::parse(&config.bridge.url).map_err(|_| Error::Configuration)?;
        validate_local_url(&url, &["ws", "wss"])?;
        let b = &config.bridge;
        if !(1..=60_000).contains(&b.operation_timeout_ms)
            || !(64 * 1024..=256 * 1024 * 1024).contains(&b.frame_bytes)
            || !(1..=4096).contains(&b.queue_events)
            || b.queue_bytes < b.frame_bytes
            || b.queue_bytes > 256 * 1024 * 1024
        {
            return Err(Error::Configuration);
        }
        let token = private_file(&b.token_file, 8192).map_err(|_| Error::Protection)?;
        let token = std::str::from_utf8(&token)
            .map_err(|_| Error::Configuration)?
            .trim();
        if token.is_empty() || !token.bytes().all(|v| v.is_ascii_graphic()) {
            return Err(Error::Configuration);
        }
        let bridge = ScBridgeConfig {
            url,
            token: token.to_owned(),
            max_message_bytes: b.frame_bytes,
            max_queued_events: b.queue_events,
            max_queued_bytes: b.queue_bytes,
            operation_deadline: Some(Duration::from_millis(b.operation_timeout_ms)),
        };
        let client = DiscoveryClient::with_timeout(
            &config.peer_rpc_url,
            config.network.clone(),
            Duration::from_millis(config.rpc_timeout_ms),
        )
        .map_err(|_| Error::Configuration)?;
        let mut seed = [0; 8];
        getrandom::fill(&mut seed).map_err(|_| Error::Configuration)?;
        let registry = config
            .registry
            .as_ref()
            .map(RegistryConfig::reader)
            .transpose()?;
        if let Some(evidence) = &mut config.conformance {
            evidence.validate().map_err(|_| Error::Configuration)?;
            if let Some(tokenizer) = &mut evidence.tokenizer {
                if tokenizer.file.is_relative() {
                    tokenizer.file = parent.join(&tokenizer.file);
                }
            }
        }
        Ok(Self {
            config,
            bridge,
            client,
            seed: u64::from_le_bytes(seed),
            registry,
        })
    }

    /// Opens distinct durable proxy stores only. Run off the async request
    /// executor. Existing invalid, foreign or unprotected stores are never reset.
    pub fn open(self) -> Result<(Arc<ProxyControl>, ProxyLifecycle)> {
        private_dir(&self.config.state_dir)?;
        let catalog_path = self.config.state_dir.join("proxy-catalog.redb");
        let presence_path = self.config.state_dir.join("proxy-presence.redb");
        // Losing only the replay store must not silently recreate its fences.
        // A partial first initialization also fails closed on the next startup.
        if store_exists(&catalog_path)? != store_exists(&presence_path)? {
            return Err(Error::Protection);
        }
        let _catalog_file = private_store(&catalog_path)?;
        let _presence_file = private_store(&presence_path)?;
        let catalog = Arc::new(
            Catalog::open(&catalog_path, self.config.network.clone())
                .map_err(|_| Error::Storage)?,
        );
        let table = Arc::new(
            Table::open(
                &presence_path,
                self.config.network.clone(),
                self.config.max_presence_routes,
            )
            .map_err(|_| Error::Storage)?,
        );
        let (presence, presence_runner) =
            Gateway::new(self.bridge, catalog.clone(), table, self.config.max_markets)
                .map_err(|_| Error::Configuration)?;
        presence
            .select(self.config.selected_markets)
            .map_err(|_| Error::Configuration)?;
        let (catalog_updates, catalog_health) = watch::channel(supervisor::Health::default());
        let conformance = self
            .config
            .conformance
            .map(|c| {
                mayhem_proxy::conformance::Store::open(
                    &self.config.state_dir.join("proxy-conformance.redb"),
                    self.config.network.clone(),
                    c,
                )
                .map(Arc::new)
                .map_err(|_| Error::Configuration)
            })
            .transpose()?;
        let (failure_updates, failure) = watch::channel(None);
        let operator = Arc::new(
            mayhem_proxy::operator::Reader::new(
                &self.config.peer_rpc_url,
                self.config.network.clone(),
                Duration::from_millis(self.config.rpc_timeout_ms),
            )
            .map_err(|_| Error::Configuration)?,
        );
        let control = Arc::new(ProxyControl {
            catalog,
            presence,
            catalog_health,
            failure,
            running: Arc::new(AtomicBool::new(false)),
            registry: self.registry,
            conformance,
            operator,
        });
        let lifecycle = ProxyLifecycle {
            control: control.clone(),
            presence_runner,
            client: self.client,
            policy: self.config.refresh,
            seed: self.seed,
            catalog_updates,
            failure_updates,
        };
        Ok((control, lifecycle))
    }
}

fn validate_local_url(url: &url::Url, schemes: &[&str]) -> Result<()> {
    let local = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    if !local
        || !schemes.contains(&url.scheme())
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Configuration);
    }
    Ok(())
}

fn store_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(Error::Protection),
    }
}

/// Shared proxy discovery state attached explicitly to GatewayState. No paid
/// authorization follows from this handle, catalog freshness or presence health.
pub struct ProxyControl {
    catalog: Arc<Catalog>,
    presence: Gateway,
    catalog_health: watch::Receiver<supervisor::Health>,
    failure: watch::Receiver<Option<&'static str>>,
    running: Arc<AtomicBool>,
    registry: Option<Arc<RegistryReader>>,
    conformance: Option<Arc<mayhem_proxy::conformance::Store>>,
    operator: Arc<mayhem_proxy::operator::Reader>,
}

impl fmt::Debug for ProxyControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyControl")
            .field("running", &self.running.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Health {
    pub running: bool,
    pub catalog_fresh: bool,
    pub catalog: supervisor::Health,
    pub presence: presence::gateway::Health,
    pub failure: Option<&'static str>,
}

impl ProxyControl {
    pub(crate) fn operator(&self) -> &Arc<mayhem_proxy::operator::Reader> {
        &self.operator
    }
    pub fn conformance(&self) -> Option<&Arc<mayhem_proxy::conformance::Store>> {
        self.conformance.as_ref()
    }
    pub(crate) fn registry(&self) -> Option<&Arc<RegistryReader>> {
        self.registry.as_ref()
    }
    pub fn catalog(&self) -> &Arc<Catalog> {
        &self.catalog
    }

    pub fn presence(&self) -> &Gateway {
        &self.presence
    }

    pub fn select_markets(&self, markets: Vec<Digest>) -> Result<()> {
        self.presence
            .select(markets)
            .map_err(|_| Error::Configuration)
    }

    /// Bounded synchronous metadata read. Freshness is recomputed at observation
    /// time; a previously Ready supervisor phase does not keep old data fresh.
    pub fn health(&self) -> Result<Health> {
        let status = self.catalog.read().map_err(|_| Error::Storage)?.status();
        let running = self.running.load(Ordering::Acquire);
        Ok(Health {
            running,
            catalog_fresh: running
                && status.discovery_is_fresh(supervisor::unix_ms(), presence::CATALOG_AGE_MS),
            catalog: self.catalog_health.borrow().clone(),
            presence: self.presence.health().borrow().clone(),
            failure: *self.failure.borrow(),
        })
    }
}

pub struct ProxyLifecycle {
    control: Arc<ProxyControl>,
    presence_runner: presence::gateway::Supervisor,
    client: DiscoveryClient,
    policy: RefreshPolicy,
    seed: u64,
    catalog_updates: watch::Sender<supervisor::Health>,
    failure_updates: watch::Sender<Option<&'static str>>,
}

impl ProxyLifecycle {
    /// Owns two bounded control loops in this caller-owned future. Signal stop
    /// (or drop its senders) and await completion, including any started catalog
    /// disk commit. No requests, ledger writes or all-market subscriptions occur.
    pub async fn run(self, mut stop: watch::Receiver<bool>) -> Result<()> {
        let _running = Running(self.control.running.clone());
        self.control.running.store(true, Ordering::Release);
        let (halt, halted) = watch::channel(false);
        let catalog = supervisor::run(
            self.control.catalog.clone(),
            self.client,
            self.policy,
            self.seed,
            halted.clone(),
            self.catalog_updates,
        );
        let presence = self.presence_runner.run(halted);
        tokio::pin!(catalog, presence);
        enum Finished {
            Stop,
            Catalog(mayhem_proxy::Result<()>),
            Presence(mayhem_proxy::Result<()>),
        }
        let finished = tokio::select! {
            biased;
            _ = stopped(&mut stop) => Finished::Stop,
            result = &mut catalog => Finished::Catalog(result),
            result = &mut presence => Finished::Presence(result),
        };
        halt.send_replace(true);
        let failure = match finished {
            Finished::Stop => {
                let (catalog, presence) = tokio::join!(catalog, presence);
                catalog
                    .err()
                    .map(|e| supervisor::error_code(&e))
                    .or_else(|| presence.err().map(|_| "presence_supervisor_failed"))
            }
            Finished::Catalog(result) => {
                let _ = presence.await;
                Some(
                    result
                        .err()
                        .map(|e| supervisor::error_code(&e))
                        .unwrap_or("catalog_supervisor_stopped"),
                )
            }
            Finished::Presence(result) => {
                let _ = catalog.await;
                Some(if result.is_err() {
                    "presence_supervisor_failed"
                } else {
                    "presence_supervisor_stopped"
                })
            }
        };
        self.control.catalog.wait_for_refresh_idle().await;
        self.failure_updates.send_replace(failure);
        if failure.is_some() {
            Err(Error::Supervisor)
        } else {
            Ok(())
        }
    }
}

async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow() || stop.changed().await.is_err() {
            return;
        }
    }
}

struct Running(Arc<AtomicBool>);
impl Drop for Running {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[cfg(unix)]
fn private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err(Error::Protection),
    }
    let meta = std::fs::symlink_metadata(path).map_err(|_| Error::Protection)?;
    if !meta.is_dir()
        || meta.mode() & 0o077 != 0
        || meta.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(Error::Protection);
    }
    Ok(())
}

#[cfg(unix)]
fn private_store(path: &Path) -> Result<std::fs::File> {
    use rustix::fs::{open, Mode, OFlags};
    use std::os::unix::fs::MetadataExt;
    let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let (fd, created) = match open(
        path,
        flags | OFlags::CREATE | OFlags::EXCL,
        Mode::RUSR | Mode::WUSR,
    ) {
        Ok(fd) => (fd, true),
        Err(rustix::io::Errno::EXIST) => (
            open(path, flags, Mode::empty()).map_err(|_| Error::Protection)?,
            false,
        ),
        Err(_) => return Err(Error::Protection),
    };
    let file = std::fs::File::from(fd);
    let meta = file.metadata().map_err(|_| Error::Protection)?;
    if !meta.is_file()
        || meta.mode() & 0o077 != 0
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.nlink() != 1
        || (!created && meta.len() == 0)
    {
        return Err(Error::Protection);
    }
    Ok(file)
}

#[cfg(not(unix))]
fn private_dir(_: &Path) -> Result<()> {
    Err(Error::Protection)
}
#[cfg(not(unix))]
fn private_store(_: &Path) -> Result<std::fs::File> {
    Err(Error::Protection)
}

#[cfg(all(test, unix))]
mod tests;
