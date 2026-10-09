//! Explicit opt-in integration-test access to the production presence publisher.
//! No alternate signing, evidence validation or eligibility implementation.
use crate::{attempts::Digest, capacity, financial, health, presence, signing, Result};
use std::sync::Arc;

pub struct PresencePublisher(presence::Publisher);
impl PresencePublisher {
    pub fn new(
        signer: Arc<signing::Authority>,
        capacity: Arc<capacity::Authority>,
    ) -> Result<Self> {
        presence::Publisher::new(signer, capacity).map(Self)
    }
    pub fn issue(
        &mut self,
        observation: &financial::offer::Observation,
        route: &Digest,
        monitor: &health::Monitor,
        health_ttl_ms: u64,
        previous: Option<&presence::Signed>,
        refresh_due: bool,
    ) -> Result<Option<presence::Signed>> {
        self.0.issue(
            observation,
            route,
            monitor,
            health_ttl_ms,
            previous,
            refresh_due,
        )
    }
    pub fn withdraw(
        &mut self,
        previous: &presence::Signed,
        draining: bool,
    ) -> Result<presence::Signed> {
        self.0.withdraw(previous, draining)
    }
}
