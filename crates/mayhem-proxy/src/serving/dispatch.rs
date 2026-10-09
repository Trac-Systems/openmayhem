//! Explicit opt-in, trusted registrations only. No buyer-selected URLs/runtimes.
use super::*;
use mayhem_proto::proxy::{ProxyEndpoint, ProxyOffer};
use negotiation::opening::{Counts, Listener};
use serde::Serialize;
use tokio::task::JoinSet;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    market: String,
    endpoint: ProxyEndpoint,
    context: String,
    outcome: String,
    metering: String,
}
impl Key {
    fn of(offer: &ProxyOffer) -> Self {
        Self {
            market: offer.market_id.clone(),
            endpoint: offer.endpoint,
            context: offer.ctx_bracket.clone(),
            outcome: offer.outcome_class.clone(),
            metering: offer.metering_policy_hash.clone(),
        }
    }
}
/// Rates and revision are not dispatcher identity: fresh negotiation validates
/// them canonically, and recovery retains the originally accepted revision.
pub struct Registration {
    key: Key,
    controller: Controller,
}
impl Registration {
    pub fn new(offer: &ProxyOffer, controller: Controller) -> Result<Self> {
        offer.validate().map_err(|_| Error::Configuration)?;
        if offer.provider_pubkey != controller.inner.identity.controller_pubkey.as_str()
            || offer.endpoint != controller.inner.endpoint
        {
            return Err(Error::Configuration);
        }
        Ok(Self {
            key: Key::of(offer),
            controller,
        })
    }
}
#[derive(Clone, Copy)]
pub struct Limits {
    pub sessions: usize,
    pub registrations: usize,
    pub observation_wait: Duration,
}
#[derive(Clone, Default, Serialize)]
pub struct Health {
    pub running: bool,
    pub active: usize,
    pub accepted: u64,
    pub rejected: u64,
    pub completed: u64,
    pub failed: u64,
    pub maintenance_steps: u64,
    pub maintenance_failures: u64,
    pub openings: Counts,
}
pub struct Dispatcher {
    listener: Listener,
    routes: BTreeMap<Key, Controller>,
    recovery: Vec<maintenance::Runner>,
    limits: Limits,
}
impl Dispatcher {
    pub fn new(
        listener: Listener,
        registrations: Vec<Registration>,
        limits: Limits,
        maintenance: maintenance::Policy,
        seed: u64,
    ) -> Result<Self> {
        if limits.sessions == 0
            || limits.sessions > 4096
            || limits.registrations == 0
            || limits.registrations > 4096
            || registrations.is_empty()
            || registrations.len() > limits.registrations
            || limits.observation_wait.is_zero()
            || limits.observation_wait > Duration::from_secs(60)
        {
            return Err(Error::Configuration);
        }
        let mut routes = BTreeMap::new();
        let mut unique = std::collections::BTreeSet::new();
        let mut recovery = Vec::new();
        for entry in registrations {
            if &entry.controller.inner.identity != listener.identity()
                || routes.contains_key(&entry.key)
            {
                return Err(Error::Configuration);
            }
            // Clones registered for several submarkets still own only one runner.
            if unique.insert(Arc::as_ptr(&entry.controller.inner) as usize) {
                recovery.push(entry.controller.maintenance(
                    maintenance.clone(),
                    seed.wrapping_add(recovery.len() as u64),
                )?);
            }
            routes.insert(entry.key, entry.controller);
        }
        Ok(Self {
            listener,
            routes,
            recovery,
            limits,
        })
    }
    pub async fn run(
        self,
        mut stop: watch::Receiver<bool>,
        updates: watch::Sender<Health>,
    ) -> Result<()> {
        let Self {
            mut listener,
            routes,
            recovery,
            limits,
        } = self;
        let (shutdown, stopping) = watch::channel(false);
        let mut maintenance = JoinSet::new();
        let mut observations = Vec::new();
        for runner in recovery {
            let stop = stopping.clone();
            let (health, observed) = watch::channel(maintenance::Health::default());
            observations.push(observed);
            maintenance.spawn(runner.run(stop, health));
        }
        let mut sampling = tokio::time::interval(limits.observation_wait);
        sampling.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut sessions = JoinSet::new();
        let mut descriptions = JoinSet::new();
        let mut health = Health {
            running: true,
            ..Health::default()
        };
        updates.send_replace(health.clone());
        let result = loop {
            tokio::select! {
                _=connection::stopped(&mut stop)=>break Ok(()),
                _=maintenance.join_next(),if !maintenance.is_empty()=>break Err(Error::Task),
                _=sampling.tick()=>{
                    health.maintenance_steps=0;health.maintenance_failures=0;
                    for observation in &observations {
                        let value=observation.borrow();
                        health.maintenance_steps=health.maintenance_steps.saturating_add(value.steps);
                        health.maintenance_failures=health.maintenance_failures.saturating_add(value.failures);
                    }
                },
                Some(result)=sessions.join_next(),if !sessions.is_empty()=>{
                    health.completed=health.completed.saturating_add(1);
                    if !matches!(result,Ok(Ok(_))) {health.failed=health.failed.saturating_add(1)}
                },
                Some(_)=descriptions.join_next(),if !descriptions.is_empty()=>{},
                result=listener.next_opening(limits.observation_wait)=>{
                    match result {
                        Ok(negotiation::opening::Opening::Descriptor(incoming))=>{
                            if descriptions.len() < crate::descriptor::READS {
                                if let Some(owner) = routes.get(&Key::of(&incoming.context().offer)) {
                                    let reader = owner.proposals().clone();
                                    descriptions.spawn(async move { reader.describe(incoming).await });
                                }
                            }
                        },
                        Ok(negotiation::opening::Opening::Negotiation(incoming))=>{
                            let owner=routes.get(&Key::of(&incoming.context().offer));
                            let accepted=if sessions.len()<limits.sessions {
                                owner.map(|owner|owner.accept(incoming))
                            } else {None};
                            match accepted {
                                Some(Ok(handle))=>{
                                    health.accepted=health.accepted.saturating_add(1);
                                    let mut stopping=stopping.clone();
                                    sessions.spawn(async move {
                                        let disconnect=handle.stop.clone();
                                        let work=handle.wait();tokio::pin!(work);
                                        tokio::select! {
                                            result=&mut work=>result,
                                            _=connection::stopped(&mut stopping)=>{
                                                disconnect.send_replace(true);
                                                work.await
                                            }
                                        }
                                    });
                                },
                                _=>health.rejected=health.rejected.saturating_add(1),
                            }
                        },
                        Err(exchange::Error::Interrupted | exchange::Error::Transport(mayhem_bridge::BridgeError::Timeout))=>(),
                        Err(error)=>break Err(Error::Transport(error)),
                    }
                }
            }
            health.active = sessions.len();
            health.openings = listener.counts();
            updates.send_replace(health.clone());
        };
        drop(listener);
        // Descriptor tasks only read and have a finite total deadline.
        while descriptions.join_next().await.is_some() {}
        // Withdraw admission first; ask connections to close and allow their
        // durable JSON/result owners and a started recovery page to finish.
        shutdown.send_replace(true);
        while let Some(result) = sessions.join_next().await {
            health.completed = health.completed.saturating_add(1);
            if !matches!(result, Ok(Ok(_))) {
                health.failed = health.failed.saturating_add(1)
            }
        }
        let mut recovery_failed = false;
        while let Some(result) = maintenance.join_next().await {
            if !matches!(result, Ok(Ok(()))) {
                recovery_failed = true
            }
        }
        health.running = false;
        health.active = 0;
        updates.send_replace(health);
        result?;
        if recovery_failed {
            Err(Error::Task)
        } else {
            Ok(())
        }
    }
}
