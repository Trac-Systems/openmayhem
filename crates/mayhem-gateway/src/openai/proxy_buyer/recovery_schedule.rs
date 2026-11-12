//! Scheduling hints only: durable pending jobs remain the source of truth.
//! Eviction/restart forgets delay, never an obligation or financial evidence.
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};

pub(super) const PAGE: usize = 64;
const RETRIES: usize = PAGE;
pub(super) const TICK: Duration = Duration::from_secs(2);
const MAX_DELAY: Duration = Duration::from_secs(60);

struct Retry {
    id: String,
    delay: Duration,
    due: Instant,
}

#[derive(Default)]
pub(super) struct Schedule {
    cursor: Option<String>,
    active: Option<String>,
    quiet_until: Option<Instant>,
    retries: VecDeque<Retry>,
}

impl Schedule {
    pub(super) fn after(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    pub(super) fn available(&self, now: Instant) -> bool {
        self.active.is_none() && self.quiet_until.is_none_or(|until| now >= until)
    }

    /// Consume cooling IDs, but leave an eligible candidate unconsumed until
    /// admission succeeds (or its existing foreground owner is discovered).
    /// In particular, never jump to the tail of a partly processed page.
    pub(super) fn candidate(&mut self, page: &[String], now: Instant) -> Option<String> {
        if !self.available(now) {
            return None;
        }
        for id in page {
            if self
                .retries
                .iter()
                .all(|retry| retry.id != *id || now >= retry.due)
            {
                return Some(id.clone());
            }
            self.visited(id);
        }
        if page.len() < PAGE {
            self.cursor = None;
        }
        None
    }

    pub(super) fn visited(&mut self, id: &str) {
        self.cursor = Some(id.to_owned());
    }

    pub(super) fn start(&mut self, id: &str, now: Instant) -> bool {
        if !self.available(now)
            || self
                .retries
                .iter()
                .any(|retry| retry.id == id && now < retry.due)
        {
            return false;
        }
        self.active = Some(id.to_owned());
        true
    }

    pub(super) fn finish(&mut self, id: &str, pending: bool, now: Instant) {
        if self.active.as_deref() != Some(id) {
            return;
        }
        self.active = None;
        // A quiet window after *completion* also gives foreground admissions
        // a turn with sessions=1, even when the pending page is always full.
        self.quiet_until = Some(now + TICK);
        let previous = self
            .retries
            .iter()
            .position(|retry| retry.id == id)
            .and_then(|index| self.retries.remove(index));
        if pending {
            let delay = previous.map_or(TICK, |retry| retry.delay.saturating_mul(2).min(MAX_DELAY));
            if self.retries.len() == RETRIES {
                self.retries.pop_front();
            }
            self.retries.push_back(Retry {
                id: id.to_owned(),
                delay,
                due: now + delay,
            });
        }
    }
}

/// Probe the foreground reserve atomically with background admission. Keeping
/// just one permit lets foreground use all remaining slots; there is only one
/// background owner. A single-session runtime must share its sole slot.
pub(super) fn claim_slot(
    slots: &Arc<Semaphore>,
    sessions: usize,
) -> Result<OwnedSemaphorePermit, TryAcquireError> {
    let mut permit = slots
        .clone()
        .try_acquire_many_owned(sessions.min(2) as u32)?;
    if sessions > 1 {
        drop(permit.split(1).expect("foreground reserve was acquired"));
    }
    Ok(permit)
}

#[cfg(test)]
mod tests;
