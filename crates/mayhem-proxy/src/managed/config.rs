use super::*;
use crate::{
    connector::config::{private_file, ConnectionConfig},
    discovery,
};
use mayhem_proto::proxy::{finance::ProxySettlementPolicy, ProxyEndpoint, ProxyOffer};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub network: discovery::Identity,
    pub provider_pubkey: Digest,
    pub peer_rpc_url: String,
    pub bridge: Bridge,
    pub state_dir: PathBuf,
    pub worker_program: PathBuf,
    pub health: health::Policy,
    pub limits: Limits,
    pub connections: Vec<ConnectionSpec>,
    pub allocations: Vec<Group>,
    pub routes: Vec<Route>,
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
    pub logical_message_bytes: usize,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub max_groups: u64,
    pub max_routes: u64,
    pub max_leases: u64,
    pub tokenizer_bytes: usize,
    pub tokenizer_workers: usize,
    pub financial_reads: usize,
    pub sessions: usize,
    pub registrations: usize,
    pub observation_ms: u64,
    /// Explicit operator monitoring policy. Only unhealthy/stale routes are
    /// probed; persistent attempt/cost allowances are always enforced.
    pub recovery_interval_ms: u64,
    pub serving: serving::Limits,
    pub worker: crate::worker::host::PoolLimits,
    pub recovery_worker: crate::worker::host::PoolLimits,
    pub settlement_worker: crate::worker::host::PoolLimits,
    pub journal: attempts::Limits,
    pub maintenance: serving::maintenance::Policy,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionSpec {
    pub group: Digest,
    pub ceiling: u32,
    pub config_file: PathBuf,
    pub probe_budget: capacity::probes::Budget,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub id: Digest,
    pub ceiling: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub id: Digest,
    pub connection: Digest,
    pub ceiling: u32,
    pub constraints: Vec<Digest>,
    pub adapter: crate::endpoint::AdapterSnapshot,
    pub offers: Vec<ProxyOffer>,
    pub settlement_policy: ProxySettlementPolicy,
    pub tokenizer: Option<Tokenizer>,
    pub recovery: Option<Recovery>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tokenizer {
    pub file: PathBuf,
    pub digest: Digest,
    pub limits: health::native::Limits,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recovery {
    pub request: serde_json::Value,
    pub streaming: bool,
    pub max_output_tokens: u64,
    pub timeout_ms: u64,
}
/// Owns checked connections and tokenizer data. This is not deserializable;
/// untrusted requests cannot manufacture startup authority from JSON.
pub struct Prepared {
    pub(super) config: Config,
    pub(super) identity: attempts::Identity,
    pub(super) bridge: mayhem_bridge::ScBridgeConfig,
    pub(super) connections: BTreeMap<Digest, super::Connection>,
    pub(super) routes: Vec<super::Route>,
    pub(super) financial: Arc<financial::Client>,
    pub(super) seed: u64,
}
impl Prepared {
    pub fn identity(&self) -> &attempts::Identity {
        &self.identity
    }
    /// All relative references resolve against their owning protected file.
    /// Missing credentials, mismatched pins or inconsistent groups fail before
    /// a state database, model request, signature or bridge connection exists.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = private_file(path, 4 * 1024 * 1024).map_err(|_| Error::Protection)?;
        let mut config: Config =
            serde_json::from_slice(&bytes).map_err(|_| Error::Configuration)?;
        let parent = std::fs::canonicalize(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )
        .map_err(|_| Error::Protection)?;
        require(
            config.schema_version == 1
                && config.network.contract_version == mayhem_proto::CONTRACT_VERSION,
        )?;
        config
            .network
            .validate()
            .map_err(|_| Error::Configuration)?;
        let identity = attempts::Identity {
            network_id: config.network.network_id.clone(),
            msb_bootstrap: Digest::new(&config.network.msb_bootstrap)
                .map_err(|_| Error::Configuration)?,
            subnet_bootstrap: Digest::new(&config.network.subnet_bootstrap)
                .map_err(|_| Error::Configuration)?,
            controller_pubkey: config.provider_pubkey.clone(),
        };
        identity.validate().map_err(|_| Error::Configuration)?;
        config.limits.validate()?;
        require(
            !config.connections.is_empty()
                && !config.routes.is_empty()
                && config.connections.len() + config.allocations.len()
                    <= config.limits.max_groups as usize
                && config.routes.len() <= config.limits.max_routes as usize
                && config.health.max_routes >= config.routes.len(),
        )?;
        for path in [
            &mut config.state_dir,
            &mut config.worker_program,
            &mut config.bridge.token_file,
        ] {
            relative(path, &parent);
        }
        let token = private_file(&config.bridge.token_file, 8192).map_err(|_| Error::Protection)?;
        let token = std::str::from_utf8(&token)
            .map_err(|_| Error::Configuration)?
            .trim();
        require(!token.is_empty() && token.bytes().all(|v| v.is_ascii_graphic()))?;
        let bridge = mayhem_bridge::ScBridgeConfig {
            url: url::Url::parse(&config.bridge.url).map_err(|_| Error::Configuration)?,
            token: token.to_owned(),
            max_message_bytes: config.bridge.frame_bytes,
            max_queued_events: config.bridge.queue_events,
            max_queued_bytes: config.bridge.queue_bytes,
            operation_deadline: Some(Duration::from_millis(config.bridge.operation_timeout_ms)),
        };
        require(
            (1..=4096).contains(&bridge.max_queued_events)
                && (64 * 1024..=256 * 1024 * 1024).contains(&bridge.max_message_bytes)
                && bridge.max_queued_bytes >= bridge.max_message_bytes
                && bridge.max_queued_bytes <= 256 * 1024 * 1024
                && (1..=86_400_000).contains(&config.bridge.operation_timeout_ms),
        )?;
        crate::exchange::channel::validate_config(
            &bridge,
            exchange::Limits {
                max_message_bytes: config.bridge.logical_message_bytes,
            },
        )
        .map_err(|_| Error::Configuration)?;
        let financial = Arc::new(
            financial::Client::new(
                &config.peer_rpc_url,
                config.network.clone(),
                config.provider_pubkey.as_str().into(),
                config.limits.financial_reads,
            )
            .map_err(|_| Error::Configuration)?,
        );
        let mut seed = [0u8; 8];
        getrandom::fill(&mut seed).map_err(|_| Error::Setup)?;
        let seed = u64::from_le_bytes(seed);
        let mut ids = BTreeSet::new();
        let mut physical = BTreeSet::new();
        for group in &config.allocations {
            require(group.ceiling > 0 && ids.insert(group.id.clone()))?;
            physical.insert(group.id.clone());
        }
        let mut connections = BTreeMap::new();
        let mut operations = BTreeMap::new();
        let mut connection_pins = BTreeSet::new();
        let mut connection_names = BTreeSet::new();
        for c in &mut config.connections {
            require(c.ceiling > 0 && ids.insert(c.group.clone()))?;
            relative(&mut c.config_file, &parent);
            let private = ConnectionConfig::load(&c.config_file).map_err(|_| Error::Protection)?;
            require(
                c.ceiling as usize <= private.limits.max_in_flight
                    && connection_names.insert(private.id.clone()),
            )?;
            operations.insert(
                c.group.clone(),
                private.paths.keys().copied().collect::<BTreeSet<_>>(),
            );
            let http = Arc::new(HttpConnection::new(private).map_err(|_| Error::Configuration)?);
            require(connection_pins.insert(http.fingerprint().clone()))?;
            let monitor = Monitor::new(
                config.health.clone(),
                c.ceiling,
                seed.wrapping_add(connections.len() as u64),
            )
            .map_err(|_| Error::Configuration)?;
            connections.insert(c.group.clone(), super::Connection { http, monitor });
        }
        let mut route_ids = BTreeSet::new();
        let mut offer_keys = BTreeSet::new();
        let mut routes = Vec::new();
        let mut tokenizer_bytes = 0usize;
        let mut tokenizer_workers = 0usize;
        for mut spec in std::mem::take(&mut config.routes) {
            require(
                spec.ceiling > 0
                    && route_ids.insert(spec.id.clone())
                    && !spec.offers.is_empty()
                    && spec.constraints.len() <= 16,
            )?;
            let connection = connections
                .get(&spec.connection)
                .ok_or(Error::Configuration)?;
            let mut constraints = BTreeSet::new();
            require(
                spec.constraints
                    .iter()
                    .all(|g| physical.contains(g) && constraints.insert(g.clone())),
            )?;
            let adapter =
                Arc::new(Adapter::restore(spec.adapter.clone()).map_err(|_| Error::Configuration)?);
            require(
                operations
                    .get(&spec.connection)
                    .is_some_and(|ops| ops.contains(&adapter.operation())),
            )?;
            require(
                config.limits.serving.proposals.request_bytes <= adapter.limits().request_bytes
                    && adapter.limits().request_bytes <= connection.http.limits().max_request_bytes
                    && adapter.limits().response_bytes
                        <= connection.http.limits().max_response_bytes,
            )?;
            spec.settlement_policy
                .validate()
                .map_err(|_| Error::Configuration)?;
            for offer in &spec.offers {
                offer.validate().map_err(|_| Error::Configuration)?;
                require(
                    offer.provider_pubkey == config.provider_pubkey.as_str()
                        && offer.endpoint == adapter.endpoint()
                        && offer_keys.insert((
                            offer.market_id.clone(),
                            offer.endpoint,
                            offer.ctx_bracket.clone(),
                            offer.outcome_class.clone(),
                            offer.metering_policy_hash.clone(),
                        )),
                )?;
            }
            let tokenizer = match (&mut spec.tokenizer, adapter.endpoint()) {
                (None, ProxyEndpoint::Decisions) => {
                    connection
                        .monitor
                        .register(spec.id.clone(), spec.ceiling, false)
                        .map_err(|_| Error::Configuration)?;
                    None
                }
                (Some(pin), endpoint) if endpoint != ProxyEndpoint::Decisions => {
                    require((1..=64 * 1024 * 1024).contains(&pin.limits.artifact_bytes))?;
                    tokenizer_bytes = tokenizer_bytes
                        .checked_add(pin.limits.artifact_bytes)
                        .ok_or(Error::Configuration)?;
                    tokenizer_workers = tokenizer_workers
                        .checked_add(pin.limits.workers)
                        .ok_or(Error::Configuration)?;
                    require(
                        tokenizer_bytes <= config.limits.tokenizer_bytes
                            && tokenizer_workers <= config.limits.tokenizer_workers,
                    )?;
                    relative(&mut pin.file, &parent);
                    let data = private_file(&pin.file, pin.limits.artifact_bytes)
                        .map_err(|_| Error::Protection)?;
                    let source = Source::from_bytes(
                        &data,
                        pin.digest.clone(),
                        connection.http.fingerprint().clone(),
                        adapter.recipe_hash().clone(),
                        pin.limits,
                    )
                    .map_err(|_| Error::Configuration)?;
                    connection
                        .monitor
                        .register_measured(spec.id.clone(), spec.ceiling, pin.digest.clone())
                        .map_err(|_| Error::Configuration)?;
                    Some(Arc::new(source))
                }
                _ => return Err(Error::Configuration),
            };
            if let Some(probe) = &spec.recovery {
                let body = serde_json::to_vec(&probe.request).map_err(|_| Error::Configuration)?;
                if probe.streaming {
                    adapter.prepare_stream(&body)
                } else {
                    adapter.prepare_json(&body)
                }
                .map_err(|_| Error::Configuration)?;
                require(
                    probe.max_output_tokens > 0 && (1..=86_400_000).contains(&probe.timeout_ms),
                )?;
                if adapter.endpoint() != ProxyEndpoint::Decisions {
                    // JSON-only replies do not establish generation speed.
                    require(probe.streaming)?;
                    let mut found = false;
                    for key in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
                        if let Some(v) = probe.request.get(key) {
                            require(
                                v.as_u64()
                                    .is_some_and(|n| n > 0 && n <= probe.max_output_tokens),
                            )?;
                            found = true;
                        }
                    }
                    require(found)?;
                }
            }
            routes.push(super::Route {
                spec,
                adapter,
                tokenizer,
            });
        }
        require(offer_keys.len() <= config.limits.registrations)?;
        Ok(Self {
            config,
            identity,
            bridge,
            connections,
            routes,
            financial,
            seed,
        })
    }
}
impl Limits {
    fn validate(&self) -> Result<()> {
        require(
            (1..=4096).contains(&self.max_groups)
                && (1..=4096).contains(&self.max_routes)
                && self.max_leases > 0
                && (1..=1024 * 1024 * 1024).contains(&self.tokenizer_bytes)
                && (1..=64).contains(&self.tokenizer_workers)
                && (1..=64).contains(&self.financial_reads)
                && (1..=4096).contains(&self.sessions)
                && (1..=4096).contains(&self.registrations)
                && (10..=60_000).contains(&self.observation_ms)
                && (10..=86_400_000).contains(&self.recovery_interval_ms)
                && (1..=64).contains(&self.maintenance.page_size),
        )?;
        self.maintenance
            .schedule
            .validate()
            .map_err(|_| Error::Configuration)?;
        for p in [self.worker, self.recovery_worker, self.settlement_worker] {
            require(
                (1..=128).contains(&p.max_children)
                    && (64 * 1024..=1024 * 1024 * 1024).contains(&p.max_buffer_bytes)
                    && !p.startup_timeout.is_zero()
                    && !p.processing_timeout.is_zero(),
            )?;
        }
        require(
            self.journal.max_records > 0
                && self.journal.max_unfinished > 0
                && self.journal.max_unfinished <= self.journal.max_records
                && self.journal.max_payload_bytes > 0
                && self.journal.closed_retention_ms > 0,
        )?;
        let s = self.serving;
        let p = s.proposals;
        require(
            (1..=4096).contains(&s.sessions)
                && s.sessions <= self.sessions
                && s.per_buyer > 0
                && s.per_buyer <= s.sessions
                && (1..=4096).contains(&s.outbound_messages)
                && (1..=256 * 1024 * 1024).contains(&s.outbound_bytes)
                && !s.control_wait.is_zero()
                && s.control_wait <= Duration::from_secs(86_400)
                && (1..=4096).contains(&p.pending)
                && p.per_buyer > 0
                && p.per_buyer <= p.pending
                && (1..=64).contains(&p.storage_operations)
                && (1..=256 * 1024 * 1024).contains(&p.request_bytes)
                && p.total_request_bytes >= p.request_bytes
                && !p.unsigned_lifetime.is_zero(),
        )
    }
}
fn relative(path: &mut PathBuf, parent: &Path) {
    if path.is_relative() {
        *path = parent.join(&*path);
    }
}
