//! One independently scheduled owner. Each step visits at most one bounded page
//! per source, rotates fairly, and exposes only coalesced counts; no history log.
use super::*;
use crate::supervisor::{unix_ms, RefreshPolicy, Schedule};
use serde::Serialize;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub page_size: usize,
    pub schedule: RefreshPolicy,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Unsigned,
    Signing,
    Retirements,
    Execution,
}
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub source: Source,
    pub examined: usize,
    pub recovered: usize,
    pub expired: usize,
    pub pruned: usize,
    pub failed: usize,
    pub more: bool,
}
#[derive(Clone, Debug, Serialize, Default)]
pub struct Health {
    pub running: bool,
    pub steps: u64,
    pub failures: u64,
    pub last: Option<Report>,
    pub retry_in_ms: Option<u64>,
}
pub struct Runner {
    owner: Arc<Inner>,
    _permit: OwnedSemaphorePermit,
    policy: Policy,
    schedule: Schedule,
    next: usize,
    cursors: [Option<Digest>; 4],
}
impl Controller {
    pub fn maintenance(&self, policy: Policy, seed: u64) -> Result<Runner> {
        if !(1..=64).contains(&policy.page_size) {
            return Err(Error::Configuration);
        }
        policy.schedule.validate()?;
        let permit = self
            .inner
            .maintenance
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        Ok(Runner {
            owner: self.inner.clone(),
            _permit: permit,
            schedule: Schedule::new(policy.schedule.clone(), seed),
            policy,
            next: 0,
            cursors: std::array::from_fn(|_| None),
        })
    }
}
impl Runner {
    pub async fn page(&mut self) -> Result<Report> {
        let index = self.next;
        self.next = (self.next + 1) % 4;
        let limit = self.policy.page_size;
        let source = [
            Source::Unsigned,
            Source::Signing,
            Source::Retirements,
            Source::Execution,
        ][index];
        let mut report = Report {
            source,
            examined: 0,
            recovered: 0,
            expired: 0,
            pruned: 0,
            failed: 0,
            more: false,
        };
        match self.owner.proposals.expire_unsigned_page(limit).await {
            Ok(expired) => report.expired = expired,
            Err(_) => report.failed += 1,
        }
        let after = self.cursors[index].clone();
        match source {
            Source::Unsigned => {
                let page = self
                    .owner
                    .proposals
                    .reconcile_unsigned(after, limit)
                    .await?;
                report.examined = page.examined;
                report.recovered = page.released;
                report.failed += page.failed;
                self.cursors[index] = page.next_after;
            }
            Source::Signing => {
                let page = self
                    .owner
                    .proposals
                    .reconcile_signing(after, limit, unix_ms())
                    .await?;
                report.examined = page.examined;
                report.recovered = page.released;
                report.failed += page.failed;
                self.cursors[index] = page.next_after;
            }
            Source::Retirements => {
                let page = self
                    .owner
                    .proposals
                    .reconcile_retirements(after, limit, unix_ms())
                    .await?;
                report.examined = page.examined;
                report.recovered = page.closed;
                report.failed += page.failed;
                self.cursors[index] = page.next_after;
            }
            Source::Execution => {
                let page = self
                    .owner
                    .recovery_executor
                    .recovery_page(after, limit)
                    .await?;
                report.examined = page.records.len();
                for record in page.records {
                    // Live execution owns its state. Maintenance must never race a
                    // stream to publish a terminal outcome or consume its capacity.
                    if self.owner.cancellation(&record.invocation)?.is_some() {
                        continue;
                    }
                    match self
                        .owner
                        .recovery_executor
                        .resume_saved(&record.invocation, record.attempt)
                        .await
                    {
                        Ok(true) => report.recovered += 1,
                        Ok(false) => (),
                        Err(_) => report.failed += 1,
                    }
                }
                self.cursors[index] = page.next_after;
            }
        }
        report.more = self.cursors[index].is_some();
        // Indexed expiry touches closed rows only; unknown holds never age out.
        match self.owner.recovery_executor.prune(unix_ms(), limit).await {
            Ok(pruned) => report.pruned = pruned,
            Err(_) => report.failed += 1,
        }
        Ok(report)
    }
    pub async fn run(
        mut self,
        mut stop: watch::Receiver<bool>,
        updates: watch::Sender<Health>,
    ) -> Result<()> {
        let mut health = Health {
            running: true,
            ..Health::default()
        };
        let mut failed = 0u32;
        loop {
            if *stop.borrow() {
                break;
            }
            // Finish a started bounded recovery operation on graceful shutdown;
            // abandoning an ACK does not make the durable obligation disappear.
            let result = self.page().await;
            health.steps = health.steps.saturating_add(1);
            let good = result.as_ref().is_ok_and(|page| page.failed == 0);
            if good {
                failed = 0;
            } else {
                failed = failed.saturating_add(1);
                health.failures = health.failures.saturating_add(1);
            }
            let more = result.as_ref().is_ok_and(|page| page.more);
            health.last = result.ok();
            let delay = if !good {
                self.schedule.retry(failed)
            } else {
                self.schedule.jitter(if more {
                    self.policy.schedule.page_pause_ms
                } else {
                    self.policy.schedule.interval_ms
                })
            };
            health.retry_in_ms = Some(delay);
            updates.send_replace(health.clone());
            tokio::select! {
                _=tokio::time::sleep(Duration::from_millis(delay))=>(),
                _=super::connection::stopped(&mut stop)=>break,
            }
        }
        health.running = false;
        health.retry_in_ms = None;
        updates.send_replace(health);
        Ok(())
    }
}
