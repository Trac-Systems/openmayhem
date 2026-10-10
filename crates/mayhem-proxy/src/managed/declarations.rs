//! Local metadata only. One bounded background read per configured source; no
//! inference-path I/O, history traversal, registry call or financial mutation.
use super::*;
use crate::declaration::live::{Inspection, Live};
use std::sync::Mutex;

#[derive(Clone, Serialize)]
pub struct Status {
    #[serde(flatten)]
    pub declaration: Inspection,
    pub source_status: &'static str,
}

#[derive(Clone)]
pub(super) struct Entry {
    pub route: Digest,
    pub source: crate::setup::DeclarationSource,
    pub live: Arc<Live>,
    pub source_status: Arc<Mutex<&'static str>>,
}
pub(super) struct Runner {
    pub entries: Vec<Entry>,
}
fn now() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
}
impl Entry {
    fn refresh(&self) {
        let time = now();
        let result = time
            .ok_or(crate::setup::Error::Invalid)
            .and_then(|now| self.source.refresh(now, self.live.expired_identity()));
        let source_status = match &result {
            Ok(Some(_)) => "read",
            Ok(None) => "expired_or_not_yet_valid",
            Err(crate::setup::Error::Busy) => "busy",
            Err(crate::setup::Error::Missing) => "missing",
            Err(crate::setup::Error::Protection) => "protection_rejected",
            Err(crate::setup::Error::Conflict) => "revision_conflict",
            Err(crate::setup::Error::Storage | crate::setup::Error::CommitUnknown) => {
                "storage_unavailable"
            }
            Err(_) => "invalid_record",
        };
        if let Ok(mut state) = self.source_status.lock() {
            *state = source_status;
        }
        let signed = result.ok().flatten();
        if time.is_none() || self.live.update(signed, time.unwrap_or(0)).is_err() {
            let _ = self.live.update(None, 0);
        }
    }
}
impl Runner {
    pub fn snapshots(&self) -> BTreeMap<Digest, Status> {
        let now = now().unwrap_or(0);
        self.entries
            .iter()
            .map(|e| {
                (
                    e.route.clone(),
                    Status {
                        declaration: e.live.inspect(now),
                        source_status: e
                            .source_status
                            .lock()
                            .map(|v| *v)
                            .unwrap_or("state_unavailable"),
                    },
                )
            })
            .collect()
    }
    pub fn clear(&self) {
        for entry in &self.entries {
            let _ = entry.live.update(None, 0);
        }
    }
    pub async fn run(&self, mut stop: watch::Receiver<bool>) -> Result<()> {
        let mut timer = tokio::time::interval(Duration::from_secs(5));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = stopped(&mut stop) => break,
                _ = timer.tick() => {
                    if self.entries.is_empty() { continue; }
                    let entries = self.entries.clone();
                    // At most one metadata task in flight. All paths/counts come
                    // from bounded trusted config, never a provider response.
                    if tokio::task::spawn_blocking(move || { for entry in entries { entry.refresh(); } }).await.is_err() {
                        for entry in &self.entries { let _ = entry.live.update(None, 0); }
                    }
                }
            }
        }
        for entry in &self.entries {
            let _ = entry.live.update(None, 0);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
