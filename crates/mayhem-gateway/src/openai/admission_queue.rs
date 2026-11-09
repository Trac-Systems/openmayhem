//! Bounded, in-process FIFO admission across overlapping provider routes.
//! Providers remain authoritative for capacity shared with other gateways.

use super::RecoverMutex;
use crate::provider_table::ProviderKey;
use serde_json::{json, Value};
use std::time::Instant;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::Notify;

const MAX_REQUESTS: usize = 1024;
const MAX_ROUTE_INTERESTS: usize = 16_384;

#[derive(Debug, Default)]
struct State {
    next: u64,
    requests: BTreeMap<u64, Entry>,
    active: BTreeMap<ProviderKey, u32>,
    interests: usize,
    jobs: BTreeMap<String, u64>,
    waiting: BTreeMap<ProviderKey, BTreeSet<u64>>,
}

#[derive(Debug)]
struct Entry {
    routes: BTreeSet<ProviderKey>,
    claimed: Option<ProviderKey>,
    job: Option<String>,
    queued_at: Instant,
    claimed_at: Option<Instant>,
    counted: bool,
    paused: bool,
}

#[derive(Debug)]
pub(super) struct AdmissionQueue {
    state: Mutex<State>,
    pub(super) generation: Arc<AtomicU64>,
    pub(super) notify: Arc<Notify>,
}

impl AdmissionQueue {
    pub(super) fn new(generation: Arc<AtomicU64>, notify: Arc<Notify>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
            generation,
            notify,
        })
    }

    #[cfg(test)]
    pub(super) fn register(self: &Arc<Self>, routes: BTreeSet<ProviderKey>) -> Option<Ticket> {
        self.register_job(routes, None)
    }

    pub(super) fn register_job(
        self: &Arc<Self>,
        routes: BTreeSet<ProviderKey>,
        job: Option<&str>,
    ) -> Option<Ticket> {
        let mut state = self.state.lock_recover("route admission queue");
        if state.requests.len() >= MAX_REQUESTS
            || state.interests.saturating_add(routes.len()) > MAX_ROUTE_INTERESTS
        {
            return None;
        }
        let id = state.next.checked_add(1)?;
        state.next = id;
        state.interests += routes.len();
        let job = job.filter(|job| job.len() <= 128).map(str::to_owned);
        if let Some(job) = &job {
            state.jobs.insert(job.clone(), id);
        }
        add_waiter(&mut state, id, &routes);
        state.requests.insert(
            id,
            Entry {
                routes,
                claimed: None,
                job,
                queued_at: Instant::now(),
                claimed_at: None,
                counted: false,
                paused: false,
            },
        );
        Some(Ticket(Arc::new(TicketInner {
            queue: Arc::clone(self),
            id,
        })))
    }

    /// Called only after the owning job has been authorized by the gateway.
    pub(super) fn job_status(&self, job: &str) -> Option<Value> {
        let state = self.state.lock_recover("route admission queue");
        let entry = state.requests.get(state.jobs.get(job)?)?;
        Some(
            json!({ "state": if entry.claimed.is_some() { "provider_selected" } else { "waiting_for_provider" },
            "wait_ms": entry.claimed_at.unwrap_or_else(Instant::now).saturating_duration_since(entry.queued_at).as_millis() }),
        )
    }

    fn changed(&self) {
        self.generation.fetch_add(1, Ordering::Release);
        self.notify.notify_waiters();
    }

    #[cfg(test)]
    pub(super) fn request_count(&self) -> usize {
        self.state
            .lock_recover("route admission queue")
            .requests
            .len()
    }
}

#[derive(Debug)]
struct TicketInner {
    queue: Arc<AdmissionQueue>,
    id: u64,
}

#[derive(Clone, Debug)]
pub(super) struct Ticket(Arc<TicketInner>);

impl PartialEq for Ticket {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone, Debug)]
pub(super) struct Handle(std::sync::Weak<TicketInner>);

impl Handle {
    pub(super) fn provider_accepted(&self, route: &ProviderKey) {
        if let Some(inner) = self.0.upgrade() { Ticket(inner).provider_accepted(route); }
    }
    pub(super) fn pause(&self) {
        if let Some(inner) = self.0.upgrade() { Ticket(inner).pause(); }
    }
    pub(super) fn resume(&self) {
        if let Some(inner) = self.0.upgrade() { Ticket(inner).resume(); }
    }
}

impl Ticket {
    pub(super) fn handle(&self) -> Handle { Handle(Arc::downgrade(&self.0)) }
    /// Refresh only the bounded set of routes this request can eventually use.
    /// A stale price/capability constraint must not hold unrelated requests back.
    pub(super) fn update_routes(&self, routes: BTreeSet<ProviderKey>) -> bool {
        let mut state = self.0.queue.state.lock_recover("route admission queue");
        let Some(entry) = state.requests.get(&self.0.id) else {
            return false;
        };
        if entry.routes == routes && !entry.paused {
            return true;
        }
        let interests = state.interests - entry.routes.len() + routes.len();
        if interests > MAX_ROUTE_INTERESTS {
            return false;
        }
        let old_routes = entry.routes.clone();
        let waiting = entry.claimed.is_none();
        if waiting {
            remove_waiter(&mut state, self.0.id, &old_routes);
            add_waiter(&mut state, self.0.id, &routes);
        }
        state.interests = interests;
        let entry = state.requests.get_mut(&self.0.id).expect("live ticket");
        entry.routes = routes; entry.paused = false;
        drop(state);
        self.0.queue.changed();
        true
    }

    /// Capacity on the provider is authoritative after authenticated acceptance.
    /// This local permit only serializes competing admission handshakes; receipt
    /// persistence and canonical settlement must never hold it indefinitely.
    pub(super) fn provider_accepted(&self, route: &ProviderKey) {
        let mut state = self.0.queue.state.lock_recover("route admission queue");
        let Some(entry) = state.requests.get_mut(&self.0.id) else { return; };
        if entry.claimed.as_ref() != Some(route) || !entry.counted { return; }
        entry.counted = false;
        release_count(&mut state, route);
        drop(state); self.0.queue.changed();
    }

    pub(super) fn resume(&self) {
        let mut state = self.0.queue.state.lock_recover("route admission queue");
        let Some(entry) = state.requests.get_mut(&self.0.id) else { return; };
        if !entry.paused { return; }
        entry.paused = false;
        let routes = entry.routes.clone();
        if entry.claimed.is_none() { add_waiter(&mut state, self.0.id, &routes); }
        drop(state); self.0.queue.changed();
    }

    pub(super) fn pause(&self) {
        let mut state = self.0.queue.state.lock_recover("route admission queue");
        let Some(entry) = state.requests.get_mut(&self.0.id) else { return; };
        entry.paused = true;
        let routes = entry.routes.clone();
        remove_waiter(&mut state, self.0.id, &routes);
        drop(state); self.0.queue.changed();
    }

    pub(super) fn can_attempt(&self, route: &ProviderKey) -> bool {
        let state = self.0.queue.state.lock_recover("route admission queue");
        // Retry selection can run before the old attempt's RAII guard drops.
        // This only returns candidates; try_claim still forbids double admission.
        state
            .requests
            .get(&self.0.id)
            .is_some_and(|entry| entry.claimed.is_some() && entry.routes.contains(route))
            || is_first(&state, self.0.id, route)
    }

    /// Reserve one slot atomically. Heartbeats can already include our own
    /// sessions, so compare their active count with ours instead of adding it.
    pub(super) fn try_claim(
        &self,
        route: &ProviderKey,
        active: u32,
        max: u32,
        free: u32,
    ) -> Option<Lease> {
        let mut state = self.0.queue.state.lock_recover("route admission queue");
        if !is_first(&state, self.0.id, route)
            || free == 0
            || state.active.get(route).copied().unwrap_or(0).max(active) >= max
        {
            return None;
        }
        let entry = state.requests.get_mut(&self.0.id)?;
        entry.claimed = Some(route.clone());
        entry.counted = true;
        entry.claimed_at = Some(Instant::now());
        let interests = entry.routes.clone();
        remove_waiter(&mut state, self.0.id, &interests);
        *state.active.entry(route.clone()).or_default() += 1;
        drop(state);
        self.0.queue.changed();
        Some(Lease {
            ticket: self.clone(),
            route: route.clone(),
        })
    }
}

fn release_count(state: &mut State, route: &ProviderKey) {
    if let Some(count) = state.active.get_mut(route) {
        *count -= 1;
        if *count == 0 { state.active.remove(route); }
    }
}

fn add_waiter(state: &mut State, id: u64, routes: &BTreeSet<ProviderKey>) {
    for route in routes {
        state.waiting.entry(route.clone()).or_default().insert(id);
    }
}

fn remove_waiter(state: &mut State, id: u64, routes: &BTreeSet<ProviderKey>) {
    for route in routes {
        if let Some(waiting) = state.waiting.get_mut(route) {
            waiting.remove(&id);
            if waiting.is_empty() {
                state.waiting.remove(route);
            }
        }
    }
}

fn is_first(state: &State, id: u64, route: &ProviderKey) -> bool {
    state
        .requests
        .get(&id)
        .is_some_and(|entry| !entry.paused && entry.claimed.is_none() && entry.routes.contains(route))
        && state.waiting.get(route).and_then(BTreeSet::first) == Some(&id)
}

impl Drop for TicketInner {
    fn drop(&mut self) {
        let mut state = self.queue.state.lock_recover("route admission queue");
        if let Some(entry) = state.requests.remove(&self.id) {
            remove_waiter(&mut state, self.id, &entry.routes);
            state.interests -= entry.routes.len();
            if let Some(job) = entry.job {
                if state.jobs.get(&job) == Some(&self.id) {
                    state.jobs.remove(&job);
                }
            }
        }
        drop(state);
        self.queue.changed();
    }
}

#[derive(Debug)]
pub(super) struct Lease {
    ticket: Ticket,
    route: ProviderKey,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let queue = &self.ticket.0.queue;
        let mut state = queue.state.lock_recover("route admission queue");
        if let Some(entry) = state.requests.get_mut(&self.ticket.0.id) {
            entry.claimed = None;
            entry.claimed_at = None;
            let routes = entry.routes.clone();
            let counted = entry.counted; entry.counted = false;
            let paused = entry.paused;
            if !paused { add_waiter(&mut state, self.ticket.0.id, &routes); }
            if counted { release_count(&mut state, &self.route); }
        }
        drop(state);
        queue.changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn queue() -> Arc<AdmissionQueue> {
        AdmissionQueue::new(Arc::new(AtomicU64::new(0)), Arc::new(Notify::new()))
    }
    fn route(name: &str) -> ProviderKey {
        ProviderKey::new(name, "enclave", "room")
    }
    fn join(queue: &Arc<AdmissionQueue>, names: &[&str]) -> Ticket {
        queue
            .register(names.iter().map(|name| route(name)).collect())
            .unwrap()
    }

    #[test]
    fn job_status_distinguishes_waiting_from_selection_and_drops_with_ticket() {
        let q = queue();
        let ticket = q
            .register_job(BTreeSet::from([route("a")]), Some("job-a"))
            .unwrap();
        assert_eq!(
            q.job_status("job-a").unwrap()["state"],
            "waiting_for_provider"
        );
        assert!(q.job_status("other-job").is_none());
        let lease = ticket.try_claim(&route("a"), 0, 1, 1).unwrap();
        assert_eq!(q.job_status("job-a").unwrap()["state"], "provider_selected");
        drop(lease);
        assert_eq!(
            q.job_status("job-a").unwrap()["state"],
            "waiting_for_provider"
        );
        drop(ticket);
        assert!(q.job_status("job-a").is_none());
        assert!(q.state.lock().unwrap().jobs.is_empty());
    }

    #[test]
    fn accepted_session_keeps_fifo_claim_but_does_not_hold_capacity_through_accounting() {
        let q = queue(); let a = join(&q, &["a"]); let b = join(&q, &["a"]);
        let first = a.try_claim(&route("a"), 0, 1, 1).unwrap();
        a.provider_accepted(&route("a")); a.provider_accepted(&route("a"));
        assert!(q.state.lock().unwrap().active.is_empty());
        // Provider heartbeat remains authoritative while generation is active.
        assert!(b.try_claim(&route("a"), 1, 1, 0).is_none());
        // The engine becomes free while the first receipt is still pending.
        let second = b.try_claim(&route("a"), 0, 1, 1).unwrap();
        drop(first);
        assert_eq!(q.state.lock().unwrap().active[&route("a")], 1);
        drop((a, b, second)); assert!(q.state.lock().unwrap().active.is_empty());
    }

    #[test]
    fn waiting_for_payment_evidence_does_not_block_other_customers() {
        let q = queue(); let a = join(&q, &["a"]); let b = join(&q, &["a"]);
        let first = a.try_claim(&route("a"), 0, 1, 1).unwrap();
        a.pause(); drop(first);
        let second = b.try_claim(&route("a"), 0, 1, 1).unwrap();
        assert!(a.update_routes(BTreeSet::from([route("a")])));
        assert!(!a.can_attempt(&route("unknown")));
        drop(second); drop(b);
        assert!(a.try_claim(&route("a"), 0, 1, 1).is_some());
    }

    #[test]
    fn older_request_keeps_priority_even_when_new_request_checks_first() {
        let q = queue();
        let old = join(&q, &["a"]);
        let new = join(&q, &["a"]);
        assert!(new.try_claim(&route("a"), 0, 2, 2).is_none());
        let first = old.try_claim(&route("a"), 0, 2, 2).unwrap();
        let second = new.try_claim(&route("a"), 0, 2, 2).unwrap();
        assert_eq!(q.state.lock().unwrap().active[&route("a")], 2);
        drop((first, second, old, new));
        assert!(q.state.lock().unwrap().requests.is_empty());
        assert!(q.state.lock().unwrap().active.is_empty());
        assert!(q.state.lock().unwrap().waiting.is_empty());
    }

    #[test]
    fn occupied_or_incompatible_provider_does_not_block_another_route() {
        let q = queue();
        let old = join(&q, &["a"]);
        let new = join(&q, &["a", "b"]);
        assert!(old.try_claim(&route("a"), 2, 2, 0).is_none());
        assert!(new.try_claim(&route("a"), 0, 2, 2).is_none());
        assert!(new.try_claim(&route("b"), 0, 2, 2).is_some());
    }

    #[test]
    fn stale_heartbeat_cannot_oversubscribe_local_slots_or_double_count_them() {
        let q = queue();
        let a = join(&q, &["a"]);
        let b = join(&q, &["a"]);
        let c = join(&q, &["a"]);
        let one = a.try_claim(&route("a"), 0, 2, 2).unwrap();
        let two = b.try_claim(&route("a"), 1, 2, 1).unwrap();
        assert!(c.try_claim(&route("a"), 0, 2, 2).is_none());
        drop(one);
        drop(a);
        assert!(c.try_claim(&route("a"), 1, 2, 1).is_some());
        drop(two);
    }

    #[test]
    fn clean_refusal_keeps_original_queue_position_and_cancel_removes_it() {
        let q = queue();
        let old = join(&q, &["a"]);
        let new = join(&q, &["a"]);
        drop(old.try_claim(&route("a"), 0, 1, 1).unwrap());
        assert!(!new.can_attempt(&route("a")));
        drop(old);
        assert!(new.try_claim(&route("a"), 0, 1, 1).is_some());
    }

    #[test]
    fn changed_price_or_capability_interest_unblocks_compatible_requests() {
        let q = queue();
        let old = join(&q, &["a"]);
        let new = join(&q, &["a"]);
        assert!(old.update_routes(BTreeSet::new()));
        assert!(new.try_claim(&route("a"), 0, 1, 1).is_some());
    }

    #[test]
    fn queue_memory_is_bounded_and_capacity_returns_after_cancellation() {
        let q = queue();
        let requests = (0..MAX_REQUESTS)
            .map(|_| join(&q, &["a"]))
            .collect::<Vec<_>>();
        assert!(q.register(BTreeSet::from([route("a")])).is_none());
        drop(requests);
        assert_eq!(q.state.lock().unwrap().interests, 0);
        assert!(q.register(BTreeSet::from([route("a")])).is_some());
        assert!(q
            .register(
                (0..MAX_ROUTE_INTERESTS + 1)
                    .map(|i| route(&i.to_string()))
                    .collect()
            )
            .is_none());
    }

    #[tokio::test]
    async fn aborting_a_queued_future_releases_its_position_and_wakes_waiters() {
        let q = queue();
        let old = join(&q, &["a"]);
        let new = join(&q, &["a"]);
        let task = tokio::spawn(async move {
            let _old = old;
            std::future::pending::<()>().await;
        });
        let notified = q.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        task.abort();
        let _ = task.await;
        tokio::time::timeout(std::time::Duration::from_secs(1), notified)
            .await
            .unwrap();
        assert!(new.can_attempt(&route("a")));
    }
}
