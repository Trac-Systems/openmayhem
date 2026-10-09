//! Gateway-owned selected-market presence lifecycle. Catalog hydration remains
//! caller-owned; this component never treats an old catalog as authorization.
//! Consumers share `Gateway::status` for routing and catalog availability. This
//! is an integration component, not wiring into the native gateway or public UI.

use super::{receive_bridge, unix_ms, Digest, Eligibility, ReceiverHealth, Registered, Table};
use crate::{catalog::Catalog, require, Error, Result};
use mayhem_bridge::ScBridgeConfig;
use serde::Serialize;
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::watch;

const RETRY_MIN: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(5);

#[derive(Clone, Default, Debug, Serialize)]
pub struct Health {
    pub running: bool,
    pub connected: bool,
    pub selected_markets: usize,
    pub attempts: u64,
    pub receiver_failures: u64,
    pub accepted: u64,
    pub rejected: u64,
}

/// Cloneable control/read handle. Selection is explicit, bounded, coalesced and
/// initially empty. The persistent Table is never recreated on transport failure
/// or subscription replacement, preserving withdrawals and controller fences.
#[derive(Clone)]
pub struct Gateway {
    catalog: Arc<Catalog>,
    table: Arc<Table>,
    max_markets: usize,
    markets: watch::Sender<Arc<BTreeSet<Digest>>>,
    running: Arc<AtomicBool>,
    health: watch::Receiver<Health>,
}

pub struct Supervisor {
    config: ScBridgeConfig,
    catalog: Arc<Catalog>,
    table: Arc<Table>,
    max_markets: usize,
    markets: watch::Receiver<Arc<BTreeSet<Digest>>>,
    running: Arc<AtomicBool>,
    updates: watch::Sender<Health>,
}

impl Gateway {
    /// Construct one supervisor, to be owned and joined by the gateway lifecycle.
    /// No socket or task is created here. Catalog refresh scheduling is separate.
    pub fn new(
        config: ScBridgeConfig,
        catalog: Arc<Catalog>,
        table: Arc<Table>,
        max_markets: usize,
    ) -> Result<(Self, Supervisor)> {
        require(max_markets > 0, "presence subscription quota is required")?;
        crate::exchange::channel::validate_config(
            &config,
            crate::exchange::Limits {
                max_message_bytes: super::MAX_BYTES,
            },
        )
        .map_err(|_| crate::invalid("invalid protected presence bridge"))?;
        let (markets, selected) = watch::channel(Arc::new(BTreeSet::new()));
        let (updates, health) = watch::channel(Health::default());
        let running = Arc::new(AtomicBool::new(false));
        Ok((
            Self {
                catalog: catalog.clone(),
                table: table.clone(),
                max_markets,
                markets,
                running: running.clone(),
                health,
            },
            Supervisor {
                config,
                catalog,
                table,
                max_markets,
                markets: selected,
                running,
                updates,
            },
        ))
    }

    /// Atomically replace the desired set. Removed markets fail closed in status
    /// immediately, even while the old transport finishes its bounded shutdown.
    /// Repeated identical sets do not reconnect. An empty set opens no connection.
    pub fn select(&self, markets: Vec<Digest>) -> Result<()> {
        require(
            markets.len() <= self.max_markets,
            "presence subscription quota exceeded",
        )?;
        let markets = Arc::new(markets.into_iter().collect::<BTreeSet<_>>());
        self.markets.send_if_modified(|current| {
            if *current == markets {
                false
            } else {
                *current = markets;
                true
            }
        });
        Ok(())
    }

    pub fn health(&self) -> watch::Receiver<Health> {
        self.health.clone()
    }

    /// The same fresh canonical registration and Table decision for routing and
    /// catalog/UI. No registration handle or eligibility result is cached here.
    /// This bounded synchronous lookup belongs off the async inference executor;
    /// call once per candidate/control decision, never for each output token.
    /// Availability is not a reservation or permission to charge the customer.
    pub fn status(
        &self,
        market: &Digest,
        provider: &Digest,
        slot: &Digest,
        min_tok_s: Option<u32>,
    ) -> Result<Eligibility> {
        // Keep the bounded read linearized with replacement of the selected set.
        let selected = self.markets.borrow();
        if !self.running.load(Ordering::Acquire) || !selected.contains(market) {
            return Ok(Eligibility::HeartbeatMissing);
        }
        let snapshot = self.catalog.read()?;
        let registered = match Registered::read(&snapshot, market, provider, slot, unix_ms()?) {
            Ok(value) => value,
            Err(Error::Invalid(_) | Error::Identity) => return Ok(Eligibility::CatalogUnavailable),
            Err(error) => return Err(error),
        };
        drop(snapshot);
        if &registered.network != self.table.network() {
            return Ok(Eligibility::CatalogUnavailable);
        }
        self.table.status(&registered, unix_ms()?, min_tok_s)
    }
    /// The caller just resolved this registration from one fresh catalog read.
    /// No selection/subscription or durable presence state is changed.
    pub fn observe_registered(
        &self,
        registered: &Registered,
        min_tok_s: Option<u32>,
    ) -> Result<super::Observation> {
        let now = unix_ms()?;
        let selected = self.markets.borrow();
        let market = Digest::new(registered.offer.market_id.clone())
            .map_err(|_| crate::invalid("invalid registered market"))?;
        if !self.running.load(Ordering::Acquire) || !selected.contains(&market) {
            return Ok(super::Observation::missing(
                Eligibility::HeartbeatMissing,
                now,
            ));
        }
        if &registered.network != self.table.network() {
            return Ok(super::Observation::missing(
                Eligibility::CatalogUnavailable,
                now,
            ));
        }
        self.table.observe(registered, now, min_tok_s)
    }
}

impl Supervisor {
    /// One receiver and at most its one blocking validation/write task. Signal
    /// stop (or drop every stop sender), then await completion; do not abort this
    /// future during a disk write. Subscription setup obeys bridge operation
    /// deadlines; a stop/reselection drains it before another receiver starts.
    pub async fn run(mut self, mut stop: watch::Receiver<bool>) -> Result<()> {
        let _running = Running {
            running: self.running.clone(),
            updates: self.updates.clone(),
        };
        self.running.store(true, Ordering::Release);
        let mut health = Health {
            running: true,
            ..Health::default()
        };
        let mut delay = RETRY_MIN;
        loop {
            if is_stopped(&stop) || self.markets.has_changed().is_err() {
                return Ok(());
            }
            let markets = self.markets.borrow_and_update().clone();
            health.selected_markets = markets.len();
            self.updates.send_replace(health.clone());
            if markets.is_empty() {
                tokio::select! {
                    biased;
                    _ = stopped(&mut stop) => return Ok(()),
                    _ = self.markets.changed() => {},
                }
                delay = RETRY_MIN;
                continue;
            }

            let (halt, halted) = watch::channel(false);
            let (updates, mut receiver_health) = watch::channel(ReceiverHealth::default());
            let receiver = receive_bridge(
                self.config.clone(),
                self.catalog.clone(),
                self.table.clone(),
                markets.iter().cloned().collect(),
                self.max_markets,
                halted,
                updates,
            );
            tokio::pin!(receiver);
            let accepted = health.accepted;
            let rejected = health.rejected;
            let started = tokio::time::Instant::now();
            let mut connected = false;
            health.attempts = health.attempts.saturating_add(1);
            self.updates.send_replace(health.clone());
            let interrupted = loop {
                tokio::select! {
                    biased;
                    _ = stopped(&mut stop) => break true,
                    _ = self.markets.changed() => break true,
                    _ = &mut receiver => break false,
                    _ = receiver_health.changed() => {
                        let current = receiver_health.borrow_and_update().clone();
                        connected |= current.connected;
                        merge(&mut health, &current, accepted, rejected);
                        self.updates.send_replace(health.clone());
                    },
                }
            };
            if interrupted {
                halt.send_replace(true);
                // Never detach receive_bridge's in-flight blocking write or let
                // old/new subscription receivers overlap on the shared Table.
                let _ = receiver.await;
            }
            merge(&mut health, &receiver_health.borrow(), accepted, rejected);
            health.connected = false;
            if !interrupted {
                health.receiver_failures = health.receiver_failures.saturating_add(1);
            }
            self.updates.send_replace(health.clone());
            if interrupted {
                delay = RETRY_MIN;
                continue;
            }
            if connected && started.elapsed() >= RETRY_MAX {
                delay = RETRY_MIN;
            }
            tokio::select! {
                biased;
                _ = stopped(&mut stop) => return Ok(()),
                _ = self.markets.changed() => { delay = RETRY_MIN; continue; },
                _ = tokio::time::sleep(delay) => {},
            }
            delay = (delay * 2).min(RETRY_MAX);
        }
    }
}

fn merge(health: &mut Health, current: &ReceiverHealth, accepted: u64, rejected: u64) {
    health.connected = current.connected;
    health.accepted = accepted.saturating_add(current.accepted);
    health.rejected = rejected.saturating_add(current.rejected);
}

fn is_stopped(stop: &watch::Receiver<bool>) -> bool {
    *stop.borrow() || stop.has_changed().is_err()
}

async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if is_stopped(stop) || stop.changed().await.is_err() {
            return;
        }
    }
}

struct Running {
    running: Arc<AtomicBool>,
    updates: watch::Sender<Health>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        self.updates.send_modify(|health| {
            health.running = false;
            health.connected = false;
        });
    }
}
