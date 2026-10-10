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
    /// At most one closed-vocabulary diagnosis per configured route. No history,
    /// prompts, credentials, upstream URLs or arbitrary error strings.
    pub failures: BTreeMap<Digest, Failure>,
}

#[derive(Clone, Serialize)]
pub struct Failure {
    pub code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<crate::connector::failure::Failure>,
}

fn failure(error: probes::ProbeError) -> Failure {
    use crate::execution::Error as E;
    use probes::ProbeError as P;
    let code = match error {
        P::Configuration => "probe_configuration",
        P::Health(health::Error::RecoveryBusy) => "recovery_not_due",
        P::Health(_) => "health_unavailable",
        P::Execution(E::Capacity(capacity::Error::ProbeBudget)) => "probe_budget_exhausted",
        P::Execution(E::Capacity(capacity::Error::InUse | capacity::Error::ExistingWork)) => {
            "retained_work"
        }
        P::Execution(E::Capacity(_)) => "capacity_unavailable",
        P::Execution(E::Upstream(value) | E::Decoder(crate::worker::Error::Upstream(value))) => {
            return Failure {
                code: "upstream_failure",
                upstream: Some(value),
            };
        }
        P::Execution(E::Decoder(crate::worker::Error::Start)) => "decoder_start",
        P::Execution(E::Decoder(crate::worker::Error::Identity)) => "decoder_identity",
        P::Execution(E::Decoder(crate::worker::Error::ProcessingTimeout)) => {
            "decoder_processing_timeout"
        }
        P::Execution(E::Decoder(_)) => "decoder_failure",
        P::Execution(E::Endpoint(_)) => "endpoint_validation",
        P::Execution(E::RecoveryRequired | E::ExistingResult) => "retained_work",
        P::Execution(E::Cancelled) => "cancelled",
        P::Execution(E::Journal(_) | E::StorageCapacity | E::StorageWorker) => {
            "storage_unavailable"
        }
        P::Execution(E::Configuration | E::Binding) => "probe_configuration",
        P::Execution(E::Financial(_)) => "financial_unavailable",
        P::Execution(E::TransportWorker) => "transport_worker",
    };
    Failure {
        code,
        upstream: None,
    }
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
        let mut pending: JoinSet<(usize, Option<Failure>)> = JoinSet::new();
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
                        Some(Ok((id, failure))) => {
                            active.remove(&id);
                            health.completed = health.completed.saturating_add(1);
                            if let Some(failure) = failure {
                                health.failed = health.failed.saturating_add(1);
                                health.failures.insert(self.entries[id].route.clone(), failure);
                            } else {
                                health.failures.remove(&self.entries[id].route);
                            }
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
                        pending.spawn(async move { (i, owner.run().await.err().map(failure)) });
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
            match value {
                Ok((id, Some(failure))) => {
                    health.failed = health.failed.saturating_add(1);
                    health
                        .failures
                        .insert(self.entries[id].route.clone(), failure);
                }
                Ok((id, None)) => {
                    health.failures.remove(&self.entries[id].route);
                }
                Err(_) => {
                    health.failed = health.failed.saturating_add(1);
                }
            }
        }
        health.running = false;
        health.active = 0;
        updates.send_replace(health);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_diagnosis_keeps_budget_and_transport_failures_distinct_without_raw_errors() {
        use crate::{
            connector::failure::{Code, Execution, Scope, Stage},
            execution::Error,
        };
        let budget = failure(probes::ProbeError::Execution(Error::Capacity(
            capacity::Error::ProbeBudget,
        )));
        assert_eq!(budget.code, "probe_budget_exhausted");
        let raw = failure(probes::ProbeError::Execution(Error::Financial(
            crate::invalid("secret prompt https://private.example/key"),
        )));
        let encoded = serde_json::to_string(&raw).unwrap();
        assert_eq!(encoded, r#"{"code":"financial_unavailable"}"#);
        let upstream = failure(probes::ProbeError::Execution(Error::Upstream(
            crate::connector::failure::Failure::new(
                Code::UpstreamTimeout,
                Scope::Model,
                Stage::ResponseBody,
                Execution::Unknown,
            ),
        )));
        let value = serde_json::to_value(&upstream).unwrap();
        assert_eq!(value["upstream"]["execution"], "unknown");
        assert_eq!(value["upstream"]["code"], "upstream_timeout");
        assert_eq!(
            failure(probes::ProbeError::Execution(Error::Decoder(
                crate::worker::Error::Start
            )))
            .code,
            "decoder_start"
        );
    }
}
