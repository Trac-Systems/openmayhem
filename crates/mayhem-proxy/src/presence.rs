//! Proxy-only ephemeral availability. No enclave claims, ledger mutation,
//! purchase authorization or payment-policy changes. Receivers use one shared
//! eligibility result for discovery and inference; signed presence is not a lease.
pub mod gateway;
mod registry;
mod table;
mod transport;
use crate::{
    attempts::Digest, capacity, discovery, financial, health, invalid, require, signing, Result,
};
use mayhem_proto::proxy::{ProxyEndpoint, ProxyOffer};
pub use registry::Registered;
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
pub use table::Table;
pub use transport::{receive_bridge, ReceiverHealth};

// Reuse native provider transport cadence and TTL. Health and canonical evidence
// impose additional, original expiry bounds, never renewed by sending again.
pub const HEARTBEAT_MS: u64 = 2_000;
pub const TTL_MS: u64 = 60_000;
pub const MAX_BYTES: usize = 8 * 1024;
pub const CATALOG_AGE_MS: u64 = 15_000;
const FUTURE_SKEW_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Ready,
    Busy,
    Unavailable,
    Checking,
    Draining,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Fresh,
    Busy,
    RateLimited,
    Authentication,
    PaymentRequired,
    Unreachable,
    ModelUnavailable,
    InvalidResponse,
    SlowGeneration,
    SlowResponse,
    NoEvidence,
    Stale,
    RecoveryRequired,
    UnverifiedThroughput,
    StaleThroughput,
    Draining,
}
impl From<health::Reason> for Reason {
    fn from(r: health::Reason) -> Self {
        use health::Reason as H;
        match r {
            H::Fresh => Self::Fresh,
            H::Busy => Self::Busy,
            H::RateLimited => Self::RateLimited,
            H::Authentication => Self::Authentication,
            H::PaymentRequired => Self::PaymentRequired,
            H::Unreachable => Self::Unreachable,
            H::ModelUnavailable => Self::ModelUnavailable,
            H::InvalidResponse => Self::InvalidResponse,
            H::SlowGeneration => Self::SlowGeneration,
            H::SlowResponse => Self::SlowResponse,
            H::NoEvidence => Self::NoEvidence,
            H::Stale => Self::Stale,
            H::RecoveryRequired => Self::RecoveryRequired,
            H::UnverifiedThroughput => Self::UnverifiedThroughput,
            H::StaleThroughput => Self::StaleThroughput,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Speed {
    pub tok_s_milli: u64,
    pub tokenizer: Digest,
    pub observed_ms: u64,
    pub expires_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Body {
    pub t: String,
    pub schema_version: u32,
    pub network: discovery::Identity,
    pub provider: Digest,
    pub market: Digest,
    pub slot: Digest,
    pub offer: Digest,
    pub membership: Digest,
    pub membership_revision: u64,
    pub offer_revision: u64,
    pub fence: u64,
    pub boot: Digest,
    pub sequence: u64,
    pub issued_ms: u64,
    pub expires_ms: u64,
    pub evidence_expires_ms: u64,
    pub state: State,
    pub reason: Reason,
    pub allowance: u32,
    pub free_slots: u32,
    pub speed: Option<Speed>,
}
impl Body {
    pub fn signing_bytes(&self) -> Result<Vec<u8>> {
        self.network.validate()?;
        require(
            self.t == "proxy.hb"
                && self.schema_version == 1
                && self.fence > 0
                && self.sequence > 0
                && self.membership_revision > 0
                && self.offer_revision > 0
                && self.issued_ms > 0
                && self.expires_ms > self.issued_ms
                && self.expires_ms - self.issued_ms <= TTL_MS
                && self.free_slots <= self.allowance
                && (self.state == State::Ready || self.free_slots == 0),
            "invalid proxy presence",
        )?;
        require(
            [
                self.fence,
                self.sequence,
                self.membership_revision,
                self.offer_revision,
                self.issued_ms,
                self.expires_ms,
                self.evidence_expires_ms,
            ]
            .into_iter()
            .all(discovery::safe_integer),
            "proxy presence integer exceeds interoperable range",
        )?;
        if let Some(s) = &self.speed {
            require(
                s.tok_s_milli > 0
                    && s.observed_ms <= self.issued_ms
                    && s.expires_ms > s.observed_ms
                    && s.expires_ms - s.observed_ms <= 3_600_000
                    && [s.tok_s_milli, s.observed_ms, s.expires_ms]
                        .into_iter()
                        .all(discovery::safe_integer),
                "invalid proxy speed evidence",
            )?;
        }
        let mut bytes = b"mayhem.proxy.presence.v1\0".to_vec();
        bytes.extend(serde_json::to_vec(self)?);
        require(
            bytes.len() <= MAX_BYTES - 256,
            "proxy presence exceeds bound",
        )?;
        Ok(bytes)
    }
    fn key(&self) -> String {
        format!(
            "{}/{}/{}",
            self.market.as_str(),
            self.provider.as_str(),
            self.slot.as_str()
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signed {
    pub body: Body,
    pub signature: String,
}
impl Signed {
    pub fn parse(bytes: &[u8], network: &discovery::Identity, now_ms: u64) -> Result<Self> {
        require(bytes.len() <= MAX_BYTES, "proxy presence exceeds bound")?;
        let signed: Self = serde_json::from_slice(bytes)?;
        signed.verify(network, now_ms)?;
        Ok(signed)
    }
    pub fn verify(&self, network: &discovery::Identity, now_ms: u64) -> Result<()> {
        let bytes = self.body.signing_bytes()?;
        require(
            &self.body.network == network
                && self.body.issued_ms <= now_ms.saturating_add(FUTURE_SKEW_MS)
                && self.body.expires_ms > now_ms
                && crate::receipts::verify_signature(
                    &self.signature,
                    &bytes,
                    self.body.provider.as_str(),
                ),
            "proxy presence identity, freshness or signature rejected",
        )
    }
}

/// No arbitrary public signing: managed startup supplies the fresh canonical
/// offer observation and its real shared capacity authority.
pub(crate) struct Publisher {
    signer: Arc<signing::Authority>,
    capacity: Arc<capacity::Authority>,
    started: Instant,
    wall_ms: u64,
    sequence: u64,
}
impl Publisher {
    pub fn new(
        signer: Arc<signing::Authority>,
        capacity: Arc<capacity::Authority>,
    ) -> Result<Self> {
        require(
            signer.identity() == capacity.identity(),
            "presence capacity identity differs",
        )?;
        Ok(Self {
            signer,
            capacity,
            started: Instant::now(),
            wall_ms: unix_ms()?,
            sequence: 0,
        })
    }
    pub fn issue(
        &mut self,
        observation: &financial::offer::Observation,
        route: &Digest,
        monitor: &health::Monitor,
        health_ttl_ms: u64,
        previous: Option<&Signed>,
        refresh_due: bool,
    ) -> Result<Option<Signed>> {
        let (network, offer, member, canonical_remaining) = observation.presence_binding()?;
        let issued_ms = self
            .wall_ms
            .checked_add(self.started.elapsed().as_millis() as u64)
            .ok_or_else(|| invalid("presence clock overflow"))?;
        require(
            canonical_remaining > 0 && health_ttl_ms > 0 && health_ttl_ms <= 3_600_000,
            "presence evidence expired",
        )?;
        let view = monitor
            .snapshot(route)
            .map_err(|_| invalid("presence health unavailable"))?;
        let slots = self
            .capacity
            .status(route)
            .map_err(|_| invalid("presence capacity unavailable"))?;
        let (fence, boot) = self.capacity.presence_fence();
        let evidence_expires_ms = view
            .evidence_age_ms
            .and_then(|age| health_ttl_ms.checked_sub(age))
            .map(|remaining| issued_ms.saturating_add(remaining))
            .unwrap_or(issued_ms);
        let speed = view
            .native_speed
            .as_ref()
            .filter(|s| s.tok_s.is_finite() && s.tok_s >= 0.001)
            .map(|s| Speed {
                tok_s_milli: (s.tok_s * 1000.0).floor() as u64,
                tokenizer: s.tokenizer.clone(),
                observed_ms: issued_ms.saturating_sub(s.age_ms),
                expires_ms: issued_ms
                    .saturating_sub(s.age_ms)
                    .saturating_add(s.valid_for_ms),
            });
        let allowance = view.allowance.min(member.max_concurrency);
        let free = slots.available.min(allowance);
        let state = if free > 0 {
            State::Ready
        } else {
            match slots.state {
                capacity::Readiness::Ready | capacity::Readiness::Busy => State::Busy,
                capacity::Readiness::Checking => State::Checking,
                capacity::Readiness::Unavailable => State::Unavailable,
            }
        };
        let reason = if state == State::Busy {
            Reason::Busy
        } else {
            view.reason.into()
        };
        let offer_digest = Digest::new(offer.digest().map_err(invalid)?)
            .map_err(|_| invalid("invalid presence offer"))?;
        let membership_digest = Digest::new(member.digest().map_err(invalid)?)
            .map_err(|_| invalid("invalid presence member"))?;
        if !refresh_due
            && previous.is_some_and(|old| {
                old.body.state == state
                    && old.body.offer == offer_digest
                    && old.body.membership == membership_digest
                    && old.body.reason == reason
                    && old.body.free_slots == free
                    && old.body.allowance == allowance
            })
        {
            return Ok(None);
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| invalid("presence sequence exhausted"))?;
        self.signer
            .presence(Body {
                t: "proxy.hb".into(),
                schema_version: 1,
                network,
                provider: Digest::new(&offer.provider_pubkey)
                    .map_err(|_| invalid("invalid presence provider"))?,
                market: Digest::new(&offer.market_id)
                    .map_err(|_| invalid("invalid presence market"))?,
                slot: Digest::new(offer.slot_id().map_err(invalid)?)
                    .map_err(|_| invalid("invalid presence slot"))?,
                offer: offer_digest,
                membership: membership_digest,
                membership_revision: member.revision,
                offer_revision: offer.revision,
                fence,
                boot,
                sequence: self.sequence,
                issued_ms,
                expires_ms: issued_ms.saturating_add(TTL_MS.min(canonical_remaining)),
                evidence_expires_ms,
                state,
                reason,
                allowance,
                free_slots: free,
                speed,
            })
            .map(Some)
    }
    pub fn withdraw(&mut self, previous: &Signed, draining: bool) -> Result<Signed> {
        let mut body = previous.body.clone();
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| invalid("presence sequence exhausted"))?;
        body.sequence = self.sequence;
        body.issued_ms = self
            .wall_ms
            .saturating_add(self.started.elapsed().as_millis() as u64);
        body.expires_ms = body.issued_ms.saturating_add(TTL_MS);
        body.state = if draining {
            State::Draining
        } else {
            State::Unavailable
        };
        body.reason = if draining {
            Reason::Draining
        } else {
            Reason::RecoveryRequired
        };
        body.free_slots = 0;
        body.allowance = 0;
        self.signer.presence(body)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Eligibility {
    Available,
    Busy,
    Unavailable,
    Checking,
    Draining,
    HeartbeatMissing,
    StaleEvidence,
    ThroughputUnverified,
    ThroughputFloor,
    ControllerConflict,
    CatalogUnavailable,
}
/// A point-in-time read of the same eligibility decision used by admission.
/// Expiry is an evidence bound, never a capacity lease or future availability.
#[derive(Clone, Debug, Serialize)]
pub struct Observation {
    pub status: Eligibility,
    pub observed_at_ms: u64,
    pub expires_at_ms: Option<u64>,
}
impl Observation {
    pub(crate) fn missing(status: Eligibility, observed_at_ms: u64) -> Self {
        Self {
            status,
            observed_at_ms,
            expires_at_ms: None,
        }
    }
}
/// Default and explicit speed floors follow the SAME function used by catalog
/// consumers. Unknown speed never satisfies an explicit customer requirement.
pub fn eligibility(
    body: &Body,
    offer: &ProxyOffer,
    now_ms: u64,
    min_tok_s: Option<u32>,
) -> Eligibility {
    if body.expires_ms <= now_ms {
        return Eligibility::HeartbeatMissing;
    }
    match body.state {
        State::Draining => return Eligibility::Draining,
        State::Unavailable => return Eligibility::Unavailable,
        State::Checking => return Eligibility::Checking,
        _ => {}
    }
    if body.evidence_expires_ms <= now_ms {
        return Eligibility::StaleEvidence;
    }
    if offer.endpoint != ProxyEndpoint::Decisions {
        let Some(speed) = &body.speed else {
            return Eligibility::ThroughputUnverified;
        };
        if speed.expires_ms <= now_ms || speed.observed_ms > now_ms {
            return Eligibility::ThroughputUnverified;
        }
        if speed.tok_s_milli < u64::from(min_tok_s.unwrap_or(5).max(5)) * 1000 {
            return Eligibility::ThroughputFloor;
        }
    } else if min_tok_s.is_some() {
        return Eligibility::ThroughputUnverified;
    }
    if body.state == State::Busy || body.free_slots == 0 {
        Eligibility::Busy
    } else {
        Eligibility::Available
    }
}
pub fn channel(network: &discovery::Identity, market: &Digest) -> Result<String> {
    network.validate()?;
    let digest = Digest::hash(
        "mayhem.proxy.presence.channel.v1",
        &[&serde_json::to_vec(network)?, market.as_str().as_bytes()],
    );
    Ok(format!("proxy-presence-{}", digest.as_str()))
}
fn unix_ms() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|n| n.as_millis() as u64)
        .map_err(|_| invalid("presence clock unavailable"))
}
