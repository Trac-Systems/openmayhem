//! Live admission sources. Binding a connection source to a physical pool is
//! valid only when that pool has exactly this connection/credential scope.
//! Independent credentials/native runtimes need independent health constraints;
//! they must not inherit this connection's authentication or quota failures.
use super::*;
use crate::capacity::{self, Evidence, Readiness, ReadinessSource};

struct Source {
    monitor: Monitor,
    route: Option<Digest>,
}
impl Monitor {
    /// Trusted startup binds this to the declared shared credential/API pool,
    /// never blindly to a broader physical-backend allocation.
    pub fn connection_source(&self) -> Arc<dyn ReadinessSource> {
        Arc::new(Source {
            monitor: self.clone(),
            route: None,
        })
    }
    pub fn route_source(&self, id: &Digest) -> Result<Arc<dyn ReadinessSource>> {
        self.snapshot(id)?;
        Ok(Arc::new(Source {
            monitor: self.clone(),
            route: Some(id.clone()),
        }))
    }
}
impl ReadinessSource for Source {
    fn revision(&self) -> capacity::Result<u64> {
        let data = self
            .monitor
            .inner
            .data
            .lock()
            .map_err(|_| capacity::Error::Storage)?;
        if data.revision == u64::MAX {
            return Err(capacity::Error::Storage);
        }
        Ok(data.revision)
    }
    fn evidence(&self) -> capacity::Result<Evidence> {
        let data = self
            .monitor
            .inner
            .data
            .lock()
            .map_err(|_| capacity::Error::Storage)?;
        let now = Instant::now();
        let policy = &self.monitor.inner.policy;
        let (state, allowance, age) = if let Some(id) = &self.route {
            let view = snapshot(&data, id, policy, now).map_err(|_| capacity::Error::Checking)?;
            (view.state, view.allowance, view.evidence_age_ms)
        } else {
            let gate = &data.connection;
            (
                gate.state,
                gate.allowance,
                gate.observed
                    .map(|at| millis(now.saturating_duration_since(at))),
            )
        };
        let Some(age) = age else {
            return Err(match state {
                State::Busy => capacity::Error::Busy,
                State::Unavailable => capacity::Error::Unavailable,
                _ => capacity::Error::Checking,
            });
        };
        Ok(Evidence {
            state: match state {
                State::Ready | State::Degraded if allowance > 0 => Readiness::Ready,
                State::Busy => Readiness::Busy,
                State::Unavailable | State::Degraded => Readiness::Unavailable,
                _ => Readiness::Checking,
            },
            allowance,
            age: Duration::from_millis(age),
            valid_for: Duration::from_millis(policy.evidence_ttl_ms),
        })
    }
}
