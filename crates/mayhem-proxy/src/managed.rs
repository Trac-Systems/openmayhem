//! Explicit opt-in provider startup. Only protected operator files select
//! connections, runtime groups, adapters, probe spending or tokenizer data.
//! This does not register offers or pay admission. Public presence requires a
//! current canonical registration observation, independently of local health.
mod config;
mod declarations;
pub use declarations::Status as DeclarationStatus;
mod presence;
mod recovery;
#[cfg(windows)]
mod windows;
use crate::{
    attempts::{self, Digest, Journal},
    capacity,
    connector::http::HttpConnection,
    endpoint::Adapter,
    exchange,
    execution::probes,
    financial::{self, provider::Runtime},
    health::{self, native::Source, Monitor},
    negotiation::opening::Listener,
    serving::{
        self,
        dispatch::{Dispatcher, Registration},
    },
    signing::Authority,
    worker::host::Pool,
};
pub use config::{
    Bridge, Config, ConnectionSpec, Group, Limits, Prepared, Recovery, Route as RouteConfig,
    Tokenizer,
};
use serde::Serialize;
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};
use tokio::sync::watch;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("supervised proxy credentials must use protected file references, not shell environment variables")]
    RestartCredential,
    #[error("proxy provider configuration is invalid or exceeds its resource limits")]
    Configuration,
    #[error("proxy provider requires protected local configuration and state files")]
    Protection,
    #[error("proxy provider wallet does not match configured identity")]
    Identity,
    #[error("proxy provider could not initialize serving or retained recovery state")]
    Setup,
    #[error("proxy provider control transport failed; retained requests must be recovered")]
    Transport,
    #[error("proxy provider supervisor failed; retained requests must be recovered")]
    Task,
}
pub type Result<T> = std::result::Result<T, Error>;
fn require(ok: bool) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Configuration)
    }
}

struct Connection {
    http: Arc<HttpConnection>,
    monitor: Monitor,
}
struct Route {
    spec: config::Route,
    adapter: Arc<Adapter>,
    tokenizer: Option<Arc<Source>>,
}
#[derive(Clone, Default, Serialize)]
pub struct Health {
    pub serving: serving::dispatch::Health,
    pub recovery: recovery::Health,
    pub presence: presence::Health,
    pub routes: BTreeMap<Digest, health::Snapshot>,
    pub declarations: BTreeMap<Digest, DeclarationStatus>,
}
/// Owns every route and its shared capacity authority. Dropping closes the local
/// control scope; no persisted lease or financial hold is silently released.
pub struct Provider {
    identity: attempts::Identity,
    bridge: mayhem_bridge::ScBridgeConfig,
    wire: exchange::Limits,
    registrations: Vec<Registration>,
    limits: config::Limits,
    recovery: recovery::Runner,
    presence: presence::Runner,
    declarations: declarations::Runner,
    capacity: Arc<capacity::Authority>,
    monitors: BTreeMap<Digest, Monitor>,
    seed: u64,
}
impl Prepared {
    /// Unlock/verify the normal Core wallet before creating any provider stores.
    /// No inference or ledger write is started. Approved tokenizer validation
    /// uses the contained local worker before capacity configuration/serving.
    pub fn open(self, signer: Arc<Authority>) -> Result<Provider> {
        if signer.identity() != &self.identity {
            return Err(Error::Identity);
        }
        let Self {
            config,
            identity,
            bridge,
            connections,
            routes,
            financial,
            seed,
            requires_existing_capacity,
        } = self;
        private_dir(&config.state_dir)?;
        let workdir = config.state_dir.join("decoder-work");
        private_dir(&workdir)?;
        let paid_pool = Arc::new(
            Pool::new(&config.worker_program, &workdir, config.limits.worker)
                .map_err(|_| Error::Setup)?,
        );
        // Recovery has separate decoder headroom. An occupied inference pool
        // cannot consume these operator-approved probe permits.
        let probe_pool = Arc::new(
            paid_pool
                .with_independent_limits(config.limits.recovery_worker)
                .map_err(|_| Error::Setup)?,
        );
        let settlement_pool = Arc::new(
            paid_pool
                .with_independent_limits(config.limits.settlement_worker)
                .map_err(|_| Error::Setup)?,
        );
        // Parse approved tokenizer data only in the contained executable, before
        // creating/reconfiguring capacity or enabling presence/admission.
        for route in &routes {
            if let Some(source) = &route.tokenizer {
                source.validate(&paid_pool).map_err(|_| Error::Setup)?;
            }
        }
        let capacity = Arc::new(
            (if requires_existing_capacity {
                capacity::Authority::open_existing
            } else {
                capacity::Authority::open
            })(
                config.state_dir.join("capacity.redb"),
                identity.clone(),
                capacity::Limits {
                    max_groups: config.limits.max_groups,
                    max_routes: config.limits.max_routes,
                    max_leases: config.limits.max_leases,
                    max_evidence_age: Duration::from_millis(config.health.evidence_ttl_ms),
                },
            )
            .map_err(|_| Error::Setup)?,
        );
        for group in &config.allocations {
            capacity
                .configure_allocation_group(group.id.clone(), group.ceiling)
                .map_err(|_| Error::Setup)?;
        }
        for connection in &config.connections {
            let live = connections.get(&connection.group).ok_or(Error::Setup)?;
            capacity
                .configure_group(connection.group.clone(), connection.ceiling)
                .map_err(|_| Error::Setup)?;
            capacity
                .configure_probe_budget(&connection.group, connection.probe_budget.clone())
                .map_err(|_| Error::Setup)?;
            capacity
                .bind_live(
                    capacity::Scope::Group(connection.group.clone()),
                    live.monitor.connection_source(),
                )
                .map_err(|_| Error::Setup)?;
        }
        let mut registrations = Vec::new();
        let mut recovery = Vec::new();
        let mut presence_entries = Vec::new();
        let mut declaration_entries = Vec::new();
        let mut monitors = BTreeMap::new();
        for Route {
            spec,
            adapter,
            tokenizer,
        } in routes
        {
            let connection = connections.get(&spec.connection).ok_or(Error::Setup)?;
            capacity
                .configure_route_with_constraints(
                    capacity::Route {
                        id: spec.id.clone(),
                        group: spec.connection.clone(),
                        lane: capacity::Lane::Proxy,
                        max_concurrency: spec.ceiling,
                    },
                    spec.constraints.clone(),
                )
                .map_err(|_| Error::Setup)?;
            capacity
                .bind_live(
                    capacity::Scope::Route(spec.id.clone()),
                    connection
                        .monitor
                        .route_source(&spec.id)
                        .map_err(|_| Error::Setup)?,
                )
                .map_err(|_| Error::Setup)?;
            let runtime = Arc::new(Runtime {
                adapter: adapter.clone(),
                connection: connection.http.clone(),
                capacity: capacity.clone(),
                route: spec.id.clone(),
                approved_policy: spec.settlement_policy.clone(),
            });
            // Stable route ID, not revision/price, owns retained financial data.
            let journal = Arc::new(
                Journal::open(
                    config
                        .state_dir
                        .join(format!("route-{}.redb", spec.id.as_str())),
                    identity.clone(),
                    config.limits.journal,
                )
                .map_err(|_| Error::Setup)?,
            );
            let controller = serving::Controller::new_supervised(
                runtime,
                journal,
                signer.clone(),
                financial.clone(),
                paid_pool.clone(),
                settlement_pool.clone(),
                config.limits.serving,
                connection.monitor.clone(),
                tokenizer.clone(),
            )
            .map_err(|_| Error::Setup)?;
            controller
                .proposals()
                .install_declarations(spec.data_handling.clone())
                .map_err(|_| Error::Configuration)?;
            if let Some(source) = spec.declaration_source {
                let live = Arc::new(crate::declaration::live::Live::new(source.subject.clone()));
                controller
                    .proposals()
                    .install_live_declarations(live.clone())
                    .map_err(|_| Error::Configuration)?;
                declaration_entries.push(declarations::Entry {
                    route: spec.id.clone(),
                    source,
                    live,
                    source_status: Arc::new(std::sync::Mutex::new("not_read")),
                });
            }
            for offer in spec.offers {
                presence_entries.push(presence::Entry {
                    route: spec.id.clone(),
                    monitor: connection.monitor.clone(),
                    query: financial::offer::Query {
                        rail: *offer.accepted_rails.first().ok_or(Error::Configuration)?,
                        settlement_policy_hash: spec
                            .settlement_policy
                            .digest()
                            .map_err(|_| Error::Configuration)?,
                        offer: offer.clone(),
                    },
                });
                registrations
                    .push(Registration::new(&offer, controller.clone()).map_err(|_| Error::Setup)?);
            }
            if let Some(probe) = spec.recovery {
                let body = serde_json::to_vec(&probe.request).map_err(|_| Error::Configuration)?;
                let mut owner = probes::Controller::new(
                    connection.http.clone(),
                    adapter.clone(),
                    probe_pool.clone(),
                    capacity.clone(),
                    connection.monitor.clone(),
                    spec.id.clone(),
                    spec.connection,
                    &body,
                    probe.streaming,
                    probes::Limits {
                        max_request_bytes: adapter.limits().request_bytes,
                        max_response_bytes: adapter.limits().response_bytes,
                        max_output_tokens: probe.max_output_tokens,
                        timeout: Duration::from_millis(probe.timeout_ms),
                        storage_workers: 1,
                    },
                )
                .map_err(|_| Error::Configuration)?;
                if let Some(source) = tokenizer {
                    owner = owner
                        .with_tokenizer(source)
                        .map_err(|_| Error::Configuration)?;
                }
                recovery.push(recovery::Entry {
                    route: spec.id.clone(),
                    monitor: connection.monitor.clone(),
                    owner: Arc::new(owner),
                });
            }
            monitors.insert(spec.id, connection.monitor.clone());
        }
        let recovery = recovery::Runner::new(
            recovery,
            config.limits.recovery_worker.max_children,
            Duration::from_millis(config.limits.recovery_interval_ms),
        );
        let presence = presence::Runner {
            entries: presence_entries,
            financial: Arc::new(
                financial
                    .presence_reader(config.limits.financial_reads)
                    .map_err(|_| Error::Setup)?,
            ),
            publisher: Arc::new(std::sync::Mutex::new(
                crate::presence::Publisher::new(signer, capacity.clone())
                    .map_err(|_| Error::Setup)?,
            )),
            bridge: bridge.clone(),
            network: config.network,
            concurrency: config.limits.financial_reads,
            health_ttl_ms: config.health.evidence_ttl_ms,
        };
        Ok(Provider {
            identity,
            bridge,
            wire: exchange::Limits {
                max_message_bytes: config.bridge.logical_message_bytes,
            },
            registrations,
            limits: config.limits,
            recovery,
            presence,
            declarations: declarations::Runner {
                entries: declaration_entries,
            },
            capacity,
            monitors,
            seed,
        })
    }
}
impl Provider {
    /// Uses the same live monitor and durable occupancy as acceptance; this is
    /// local diagnostic evidence, not a signed public availability claim.
    pub fn route_status(&self, id: &Digest) -> Result<(health::Snapshot, capacity::Status)> {
        let view = self
            .monitors
            .get(id)
            .ok_or(Error::Configuration)?
            .snapshot(id)
            .map_err(|_| Error::Setup)?;
        let slots = self.capacity.status(id).map_err(|_| Error::Setup)?;
        Ok((view, slots))
    }
    pub async fn run(
        self,
        mut stop: watch::Receiver<bool>,
        updates: watch::Sender<Health>,
    ) -> Result<()> {
        if *stop.borrow() {
            return Ok(());
        }
        let connect = Listener::connect(self.bridge, self.identity, self.wire);
        let listener = tokio::select! {
            result = connect => result.map_err(|_| Error::Transport)?,
            _ = stopped(&mut stop) => return Ok(()),
        };
        let dispatcher = Dispatcher::new(
            listener,
            self.registrations,
            serving::dispatch::Limits {
                sessions: self.limits.sessions,
                registrations: self.limits.registrations,
                observation_wait: Duration::from_millis(self.limits.observation_ms),
            },
            self.limits.maintenance,
            self.seed,
        )
        .map_err(|_| Error::Setup)?;
        let (shutdown, stopping) = watch::channel(false);
        let (serving_updates, serving_health) =
            watch::channel(serving::dispatch::Health::default());
        let (recovery_updates, recovery_health) = watch::channel(recovery::Health::default());
        let (presence_updates, presence_health) = watch::channel(presence::Health::default());
        let serving = dispatcher.run(stopping.clone(), serving_updates);
        let recovery = self.recovery.run(stopping.clone(), recovery_updates);
        let presence = self.presence.run(stopping.clone(), presence_updates);
        let mut declaration_stop = stopping.clone();
        let declarations = async {
            let result = self.declarations.run(stopping).await;
            self.declarations.clear();
            // Metadata failure cannot stop accepted inference or settlement.
            // Keep this branch pending until the actual provider shutdown.
            stopped(&mut declaration_stop).await;
            result
        };
        tokio::pin!(serving, recovery, presence, declarations);
        let mut sampling = tokio::time::interval(Duration::from_millis(self.limits.observation_ms));
        sampling.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! {
                _ = stopped(&mut stop) => {
                    shutdown.send_replace(true);
                    let (a,b,c,d) = tokio::join!(&mut serving, &mut recovery, &mut presence, &mut declarations);
                    break a.map_err(|_| Error::Task).and(b).and(c).and(d);
                },
                a = &mut serving => {
                    shutdown.send_replace(true);
                    let (b,c,d) = tokio::join!(&mut recovery,&mut presence,&mut declarations);
                    break a.map_err(|_| Error::Transport).and(b).and(c).and(d);
                },
                b = &mut recovery => {
                    shutdown.send_replace(true);
                    let _ = tokio::join!(&mut serving,&mut presence,&mut declarations);
                    break b.and(Err(Error::Task));
                },
                c = &mut presence => {
                    shutdown.send_replace(true);
                    let _ = tokio::join!(&mut serving,&mut recovery,&mut declarations);
                    break c.and(Err(Error::Transport));
                },
                d = &mut declarations => {
                    shutdown.send_replace(true);
                    let _ = tokio::join!(&mut serving,&mut recovery,&mut presence);
                    break d.and(Err(Error::Task));
                },
                _ = sampling.tick() => {
                    updates.send_replace(Health { serving: serving_health.borrow().clone(), recovery: recovery_health.borrow().clone(), presence:presence_health.borrow().clone(), routes: snapshots(&self.monitors), declarations: self.declarations.snapshots() });
                }
            }
        };
        updates.send_replace(Health {
            serving: serving_health.borrow().clone(),
            recovery: recovery_health.borrow().clone(),
            presence: presence_health.borrow().clone(),
            routes: snapshots(&self.monitors),
            declarations: self.declarations.snapshots(),
        });
        result
    }
}
fn snapshots(monitors: &BTreeMap<Digest, Monitor>) -> BTreeMap<Digest, health::Snapshot> {
    monitors
        .iter()
        .filter_map(|(id, monitor)| monitor.snapshot(id).ok().map(|view| (id.clone(), view)))
        .collect()
}
async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow_and_update() {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}
fn private_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        // Do not recursively create permissive parents or follow a final symlink.
        match std::fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err(Error::Protection),
        }
        let m = std::fs::symlink_metadata(path).map_err(|_| Error::Protection)?;
        if !m.is_dir() || m.mode() & 0o077 != 0 || m.uid() != rustix::process::geteuid().as_raw() {
            return Err(Error::Protection);
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        windows::private_dir(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(Error::Protection)
    }
}
