//! Background catalog refresh, isolated from token generation and settlement.
//! State notifications coalesce in a watch channel; there is no growing event log.

use crate::{
    catalog::{Catalog, RefreshOutcome},
    discovery::DiscoveryClient,
    require, Error, Result,
};
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::watch, time::Instant};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RefreshPolicy {
    pub interval_ms: u64,
    pub page_pause_ms: u64,
    pub retry_initial_ms: u64,
    pub retry_max_ms: u64,
    pub jitter_percent: u8,
}

impl Default for RefreshPolicy {
    fn default() -> Self {
        Self {
            interval_ms: 30_000,
            page_pause_ms: 10,
            retry_initial_ms: 1_000,
            retry_max_ms: 30_000,
            jitter_percent: 20,
        }
    }
}

impl RefreshPolicy {
    pub fn validate(&self) -> Result<()> {
        // Control-plane scheduling only, never a cap on inference duration or
        // catalog size. A zero interval would busy-loop while the peer is down.
        require(
            self.interval_ms > 0
                && self.page_pause_ms > 0
                && self.retry_initial_ms > 0
                && self.retry_max_ms >= self.retry_initial_ms
                && self.jitter_percent <= 50
                && [self.interval_ms, self.page_pause_ms, self.retry_max_ms]
                    .iter()
                    .all(|n| *n <= 86_400_000),
            "invalid proxy catalog refresh policy",
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Starting,
    Refreshing,
    Ready,
    Degraded,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Health {
    pub phase: Phase,
    pub consecutive_failures: u32,
    pub error_code: Option<&'static str>,
    pub retry_in_ms: Option<u64>,
    pub committed_length: Option<u64>,
}

impl Default for Health {
    fn default() -> Self {
        Self {
            phase: Phase::Starting,
            consecutive_failures: 0,
            error_code: None,
            retry_in_ms: None,
            committed_length: None,
        }
    }
}

pub fn error_code(error: &Error) -> &'static str {
    match error {
        Error::Identity => "catalog_identity_mismatch",
        Error::Database(_) | Error::Io(_) => "catalog_storage_error",
        Error::Task => "catalog_worker_failed",
        Error::FinancialAdmissionUnavailable => "proxy_financial_unavailable",
        Error::RefreshBusy | Error::StaleRefresh => "catalog_refresh_changed",
        Error::DirectoryCursorExpired => "proxy_directory_cursor_expired",
        Error::DirectoryCursorInvalid => "proxy_directory_cursor_invalid",
        Error::Transport(_) => "discovery_transport_error",
        Error::Invalid(_) | Error::Json(_) => "discovery_invalid_response",
        Error::ProviderRequest(_) | Error::ProviderCapacity(_) => "provider_control_unavailable",
        Error::Http { code, .. } => match code.as_str() {
            "proxy_discovery_busy" => "proxy_discovery_busy",
            "proxy_discovery_timeout" => "proxy_discovery_timeout",
            "proxy_cursor_invalidated" => "proxy_cursor_invalidated",
            "proxy_cursor_expired" => "proxy_cursor_expired",
            _ => "proxy_discovery_unavailable",
        },
    }
}

pub(crate) struct Schedule {
    policy: RefreshPolicy,
    state: u64,
}

impl Schedule {
    pub(crate) fn new(policy: RefreshPolicy, state: u64) -> Self {
        Self { policy, state }
    }
    pub(crate) fn jitter(&mut self, delay: u64) -> u64 {
        // Scheduling noise is not a contract input or security randomness. The
        // caller's seed makes tests reproducible and desynchronizes controllers.
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let spread = delay * u64::from(self.policy.jitter_percent) / 100;
        delay
            .saturating_sub(spread)
            .saturating_add(self.state % (spread * 2 + 1))
            .max(1)
    }

    pub(crate) fn retry(&mut self, failures: u32) -> u64 {
        let multiplier = 1u64 << failures.saturating_sub(1).min(32);
        let base = self
            .policy
            .retry_initial_ms
            .saturating_mul(multiplier)
            .min(self.policy.retry_max_ms);
        self.jitter(base).min(self.policy.retry_max_ms)
    }

    fn after_commit(&mut self, started: Instant) -> Instant {
        let interval = Duration::from_millis(self.jitter(self.policy.interval_ms));
        // Start-to-start cadence: RPC/storage time consumes the interval. A slow
        // refresh never queues catch-up work; allow one new step after a minimum
        // yield, still protected by the catalog's single outstanding refresh.
        (started + interval).max(Instant::now() + Duration::from_millis(1))
    }
}

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

pub async fn run(
    catalog: Arc<Catalog>,
    client: DiscoveryClient,
    policy: RefreshPolicy,
    seed: u64,
    shutdown: watch::Receiver<bool>,
    updates: watch::Sender<Health>,
) -> Result<()> {
    run_refresh(
        |observed_at_ms| catalog.refresh_page(&client, observed_at_ms),
        unix_ms,
        policy,
        seed,
        shutdown,
        updates,
    )
    .await
}

async fn run_refresh<F, R>(
    mut refresh: F,
    clock: impl Fn() -> u64,
    policy: RefreshPolicy,
    seed: u64,
    mut shutdown: watch::Receiver<bool>,
    updates: watch::Sender<Health>,
) -> Result<()>
where
    F: FnMut(u64) -> R,
    R: std::future::Future<Output = Result<RefreshOutcome>>,
{
    policy.validate()?;
    let mut schedule = Schedule {
        policy,
        state: seed,
    };
    let mut health = Health::default();
    let mut deadline = Instant::now();
    loop {
        if *shutdown.borrow() || shutdown.has_changed().is_err() {
            break;
        }
        tokio::select! {
            _ = shutdown.changed() => continue,
            _ = tokio::time::sleep_until(deadline) => {}
        }
        let started = Instant::now();
        // Keep evidence anchored before the RPC. Completion must never renew an
        // old observation, including a long multi-page traversal.
        let observed_at_ms = clock();
        let result = tokio::select! {
            _ = shutdown.changed() => continue,
            result = refresh(observed_at_ms) => result,
        };
        match result {
            Ok(RefreshOutcome::Staged(_)) => {
                health.phase = Phase::Refreshing;
                health.consecutive_failures = 0;
                health.error_code = None;
                deadline = Instant::now() + Duration::from_millis(schedule.policy.page_pause_ms);
            }
            Ok(RefreshOutcome::Committed(status)) => {
                health.phase = Phase::Ready;
                health.consecutive_failures = 0;
                health.error_code = None;
                health.committed_length = status.committed.map(|c| c.proof.signed_length);
                deadline = schedule.after_commit(started);
            }
            Ok(RefreshOutcome::Invalidated) => {
                health.phase = Phase::Degraded;
                health.error_code = Some("proxy_cursor_invalidated");
                health.consecutive_failures = health.consecutive_failures.saturating_add(1);
                deadline = Instant::now()
                    + Duration::from_millis(schedule.retry(health.consecutive_failures));
            }
            Err(error) => {
                health.phase = Phase::Degraded;
                health.error_code = Some(error_code(&error));
                health.consecutive_failures = health.consecutive_failures.saturating_add(1);
                if matches!(
                    error,
                    Error::Identity | Error::Database(_) | Error::Io(_) | Error::Task
                ) {
                    health.retry_in_ms = None;
                    updates.send_replace(health);
                    return Err(error);
                }
                deadline = Instant::now()
                    + Duration::from_millis(schedule.retry(health.consecutive_failures));
            }
        }
        health.retry_in_ms = Some(
            deadline
                .saturating_duration_since(Instant::now())
                .as_millis() as u64,
        );
        updates.send_replace(health.clone());
    }
    health.phase = Phase::Stopped;
    health.retry_in_ms = None;
    updates.send_replace(health);
    Ok(())
}

#[cfg(test)]
#[path = "supervisor/timing_tests.rs"]
mod timing_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backoff_is_bounded_resets_and_has_reproducible_jitter() {
        let mut plain = Schedule {
            policy: RefreshPolicy {
                jitter_percent: 0,
                ..Default::default()
            },
            state: 7,
        };
        assert_eq!(
            (1..8).map(|n| plain.retry(n)).collect::<Vec<_>>(),
            [1000, 2000, 4000, 8000, 16000, 30000, 30000]
        );
        assert_eq!(plain.retry(u32::MAX), 30000);
        assert_eq!(plain.retry(1), 1000);
        let mut one = Schedule {
            policy: RefreshPolicy::default(),
            state: 123,
        };
        let mut two = Schedule {
            policy: RefreshPolicy::default(),
            state: 123,
        };
        let mut values = std::collections::BTreeSet::new();
        for _ in 0..50 {
            let value = one.jitter(1000);
            assert_eq!(value, two.jitter(1000));
            assert!((800..=1200).contains(&value));
            values.insert(value);
        }
        assert!(values.len() > 1);
    }
}
