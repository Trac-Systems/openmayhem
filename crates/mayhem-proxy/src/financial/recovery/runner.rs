//! Paced recovery of the buyer's pending index, never a scan of receipt history.
//! Only saved approvals and original opt-in expiry terms can be signed here.
//! This controller neither dispatches inference nor releases provider capacity.
use super::*;
use crate::{signing::Authority, supervisor::RefreshPolicy};
use std::time::Duration;
use tokio::sync::watch;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub page_size: usize,
    pub schedule: RefreshPolicy,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            page_size: 16,
            schedule: RefreshPolicy::default(),
        }
    }
}
impl Policy {
    pub fn validate(&self) -> Result<()> {
        require(
            (1..=PAGE_BOUND).contains(&self.page_size),
            "invalid recovery page size",
        )?;
        self.schedule.validate()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Starting,
    Recovering,
    Waiting,
    Degraded,
    Stopped,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Page {
    pub checked: usize,
    pub resolved: usize,
    pub awaiting_wallet: usize,
    pub pruned: usize,
    pub more: bool,
    pub error_code: Option<&'static str>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Health {
    pub phase: Phase,
    pub page: Page,
    pub consecutive_failures: u32,
    pub next_check_ms: Option<u64>,
}
impl Default for Health {
    fn default() -> Self {
        Self {
            phase: Phase::Starting,
            page: Page::default(),
            consecutive_failures: 0,
            next_check_ms: None,
        }
    }
}

/// One runner owns its cursor. A failed record advances it as well, so one bad
/// request cannot starve later holds. There is no growing map of retry timers.
pub struct Runner {
    recovery: Arc<BuyerRecovery>,
    signer: Option<Authority>,
    policy: Policy,
    cursor: Option<Digest>,
    schedule: crate::supervisor::Schedule,
}
enum Step {
    Pending,
    AwaitingWallet,
    Resolved,
}

impl Runner {
    pub fn new(
        recovery: Arc<BuyerRecovery>,
        signer: Option<Authority>,
        policy: Policy,
        seed: u64,
    ) -> Result<Self> {
        policy.validate()?;
        if let Some(signer) = &signer {
            require(
                signer.identity() == &recovery.store.identity,
                "recovery wallet or network differs",
            )?;
        }
        let schedule = crate::supervisor::Schedule::new(policy.schedule.clone(), seed);
        Ok(Self {
            recovery,
            signer,
            policy,
            cursor: None,
            schedule,
        })
    }

    async fn step(&self, key: Digest) -> Result<Step> {
        let r = &self.recovery;
        let saved = r.recover(key.clone()).await?;
        if saved.confirmed.is_some()
            || saved
                .reservation
                .as_ref()
                .is_some_and(|v| v.expired_unadmitted)
        {
            return Ok(Step::Resolved);
        }
        // Also handles records whose reservation was already confirmed. The
        // existing journal replays only an unconfirmed original envelope.
        let o = match r
            .publish_reservation(key.clone(), crate::supervisor::unix_ms())
            .await
        {
            Ok(o) => o,
            Err(error) => {
                if r.recover(key.clone())
                    .await?
                    .reservation
                    .is_some_and(|v| v.expired_unadmitted)
                {
                    return Ok(Step::Resolved);
                }
                return Err(error);
            }
        };
        if o.financial_outcome()?.is_some() {
            return Ok(Step::Resolved);
        }
        if saved.outcome_approved && !saved.outcome_signed {
            if let Some(signer) = &self.signer {
                r.sign_approved(signer, key.clone()).await?;
            }
            // An unsigned approved receipt still needs the authenticated
            // provider exchange to deliver it; this worker cannot fabricate it.
        }
        if !o.expiry_eligible()? {
            return Ok(
                if saved.outcome_approved && !saved.outcome_signed && self.signer.is_none() {
                    Step::AwaitingWallet
                } else {
                    Step::Pending
                },
            );
        }
        let fresh = r
            .run(move |s| s.observe(&o, true, crate::supervisor::unix_ms()))
            .await?;
        if fresh.confirmed.is_some() {
            return Ok(Step::Resolved);
        }
        if fresh.signed.is_none() {
            let Some(signer) = &self.signer else {
                return Ok(Step::AwaitingWallet);
            };
            r.sign_expiry(signer, key.clone()).await?;
        }
        Ok(
            if r.publish_expiry(key, crate::supervisor::unix_ms())
                .await?
                .is_some()
            {
                Step::Resolved
            } else {
                Step::Pending
            },
        )
    }

    /// A bounded page for a supervisor or `--once` command. Financial errors are
    /// summarized without response bodies, credentials, requests or wallet paths.
    /// Storage failure is fatal: reopen/reconcile before attempting more writes.
    pub async fn page(&mut self) -> Result<Page> {
        let mut report = Page::default();
        let keys = self
            .recovery
            .pending(self.cursor.clone(), self.policy.page_size)
            .await?;
        for key in keys {
            self.cursor = Some(key.clone());
            report.checked += 1;
            match self.step(key).await {
                Ok(Step::Resolved) => report.resolved += 1,
                Ok(Step::AwaitingWallet) => report.awaiting_wallet += 1,
                Ok(Step::Pending) => {}
                Err(error) => {
                    if fatal(&error) {
                        return Err(error);
                    }
                    report.error_code = Some(error_code(&error));
                    // Shared RPC trouble must back off after one failure, not
                    // multiply into one immediate RPC timeout per saved hold.
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(self.policy.schedule.page_pause_ms)).await;
        }
        report.more = !self
            .recovery
            .pending(self.cursor.clone(), 1)
            .await?
            .is_empty();
        if !report.more {
            self.cursor = None;
        }
        report.pruned = self
            .recovery
            .prune(crate::supervisor::unix_ms(), self.policy.page_size)
            .await?;
        Ok(report)
    }

    /// Cancellation may leave an in-flight publication ambiguous. Its original
    /// durable intent survives, and next startup reconciles it; nothing is reset.
    pub async fn run(
        mut self,
        mut stop: watch::Receiver<bool>,
        updates: watch::Sender<Health>,
    ) -> Result<()> {
        let mut health = Health::default();
        let mut delay = 0;
        loop {
            if *stop.borrow() || stop.has_changed().is_err() {
                break;
            }
            if delay > 0 {
                tokio::select! {
                    biased;
                    _ = stop.changed() => continue,
                    _ = tokio::time::sleep(Duration::from_millis(delay)) => {},
                }
            }
            health.phase = Phase::Recovering;
            health.next_check_ms = None;
            updates.send_replace(health.clone());
            let result = tokio::select! {
                biased;
                _ = stop.changed() => continue,
                result = self.page() => result,
            };
            match result {
                Ok(page) => health.page = page,
                Err(error) => {
                    health.phase = Phase::Stopped;
                    health.page.error_code = Some(error_code(&error));
                    updates.send_replace(health);
                    return Err(error);
                }
            }
            if health.page.error_code.is_some() {
                health.phase = Phase::Degraded;
                health.consecutive_failures = health.consecutive_failures.saturating_add(1);
                delay = self.schedule.retry(health.consecutive_failures);
            } else {
                health.phase = Phase::Waiting;
                health.consecutive_failures = 0;
                delay = if health.page.more {
                    self.policy.schedule.page_pause_ms
                } else {
                    self.schedule.jitter(self.policy.schedule.interval_ms)
                };
            }
            health.next_check_ms = Some(crate::supervisor::unix_ms().saturating_add(delay));
            updates.send_replace(health.clone());
        }
        health.phase = Phase::Stopped;
        health.next_check_ms = None;
        updates.send_replace(health);
        Ok(())
    }
}
fn fatal(error: &Error) -> bool {
    matches!(
        error,
        Error::Database(_) | Error::Io(_) | Error::Task | Error::Identity
    )
}
fn error_code(error: &Error) -> &'static str {
    match error {
        Error::Database(_) | Error::Io(_) | Error::Task => "recovery_storage_failed",
        Error::Identity => "recovery_identity_mismatch",
        Error::Transport(_) | Error::Http { .. } => "financial_peer_unavailable",
        _ => "financial_evidence_rejected",
    }
}
