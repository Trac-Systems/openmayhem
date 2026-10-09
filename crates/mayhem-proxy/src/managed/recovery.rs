use super::*;
use std::collections::BTreeSet;
use tokio::task::JoinSet;

pub(super) struct Entry {
    pub route: Digest,
    pub monitor: Monitor,
    pub owner: Arc<probes::Controller>,
}
#[derive(Clone, Default, Serialize)]
pub struct Health {
    pub running: bool,
    pub active: usize,
    pub completed: u64,
    pub failed: u64,
}
pub(super) struct Runner {
    entries: Vec<Entry>,
    concurrency: usize,
    interval: Duration,
}
impl Runner {
    pub fn new(entries: Vec<Entry>, concurrency: usize, interval: Duration) -> Self {
        Self {
            entries,
            concurrency,
            interval,
        }
    }
    pub async fn run(
        self,
        mut stop: watch::Receiver<bool>,
        updates: watch::Sender<Health>,
    ) -> Result<()> {
        let mut pending: JoinSet<(usize, bool)> = JoinSet::new();
        let mut active = BTreeSet::new();
        let mut cursor = 0;
        let mut timer = tokio::time::interval(self.interval);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut health = Health {
            running: true,
            ..Health::default()
        };
        updates.send_replace(health.clone());
        let result = loop {
            tokio::select! {
                biased;
                _ = stopped(&mut stop) => break Ok(()),
                value = pending.join_next(), if !pending.is_empty() => {
                    match value {
                        Some(Ok((id, success))) => {
                            active.remove(&id);
                            health.completed = health.completed.saturating_add(1);
                            if !success { health.failed = health.failed.saturating_add(1); }
                        },
                        _ => break Err(Error::Task),
                    }
                },
                _ = timer.tick() => {
                    // One bounded visit to the configured routes; never receipt
                    // history. Rotate after the last examined route for fairness.
                    for _ in 0..self.entries.len() {
                        if pending.len() >= self.concurrency { break; }
                        let i = cursor;
                        cursor = (cursor + 1) % self.entries.len();
                        let entry = &self.entries[i];
                        if active.contains(&i) { continue; }
                        let Ok(view) = entry.monitor.snapshot(&entry.route) else { continue };
                        if view.allowance > 0 || view.recovery_after_ms > 0 || view.recovery_in_progress { continue; }
                        active.insert(i);
                        let owner = entry.owner.clone();
                        pending.spawn(async move { (i, owner.run().await.is_ok()) });
                    }
                }
            }
            health.active = pending.len();
            updates.send_replace(health.clone());
        };
        // Started probes have their own explicit duration budget and durable
        // unknown-outcome handling. Do not abandon their completion publication.
        while let Some(value) = pending.join_next().await {
            health.completed = health.completed.saturating_add(1);
            if !matches!(value, Ok((_, true))) {
                health.failed = health.failed.saturating_add(1);
            }
        }
        health.running = false;
        health.active = 0;
        updates.send_replace(health);
        result
    }
}
