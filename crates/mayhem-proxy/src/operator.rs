//! Fresh canonical operator identity observations, never inference attestation.
//! The configured local peer authenticates the canonical admin service. No
//! provider label, endpoint response or persisted/deserialized claim grants KYB.
use crate::{
    attempts::Digest,
    discovery::{Context, Identity, Proof},
    invalid, require, Error, Result,
};
use serde::{Deserialize, Serialize};
use std::{
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;
use url::Url;

pub const MAX_AGE_MS: u64 = 15_000;
const MAX_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Verified,
    NotVerified,
    NotRegistered,
    Revoked,
    Inactive,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Operator {
    status: Status,
    proof_hash: Option<Digest>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    ok: bool,
    schema_version: u32,
    lane: mayhem_proto::proxy::ProxyLane,
    requester: Digest,
    provider_pubkey: Digest,
    request_nonce: Digest,
    context: Context,
    proof: Proof,
    operator: Operator,
}

/// Constructed only by a fresh authenticated read, and intentionally not
/// deserializable. Reuse within one attempt does not extend its original age.
pub struct Observation {
    provider: Digest,
    status: Status,
    proof: Proof,
    epoch: u64,
    observed_at_ms: u64,
    deadline: Instant,
}
impl Observation {
    pub fn status(&self) -> Status {
        self.status
    }
    pub fn expires_at_ms(&self) -> u64 {
        self.observed_at_ms.saturating_add(MAX_AGE_MS)
    }
    pub fn permits(&self, provider: &Digest) -> bool {
        self.provider == *provider
            && self.status == Status::Verified
            && Instant::now() < self.deadline
            && now_ms().is_ok_and(|now| now >= self.observed_at_ms && now < self.expires_at_ms())
    }
    pub fn proof(&self) -> &Proof {
        &self.proof
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

pub struct Reader {
    http: reqwest::Client,
    endpoint: Url,
    identity: Identity,
    reads: Semaphore,
    high_water: Mutex<Option<(Proof, u64)>>,
}
impl Reader {
    pub fn new(rpc_base: &str, identity: Identity, timeout: Duration) -> Result<Self> {
        identity.validate()?;
        require(!timeout.is_zero(), "operator read timeout must be bounded")?;
        let base = Url::parse(&format!("{}/", rpc_base.trim_end_matches('/')))
            .map_err(|_| invalid("invalid operator peer RPC URL"))?;
        require(
            matches!(base.scheme(), "http" | "https")
                && base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none(),
            "invalid operator peer RPC URL",
        )?;
        Ok(Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(timeout.min(Duration::from_millis(MAX_AGE_MS)))
                .build()
                .map_err(Error::Transport)?,
            endpoint: base
                .join("proxy/operator-state")
                .map_err(|_| invalid("invalid operator RPC path"))?,
            identity,
            reads: Semaphore::new(4),
            high_water: Mutex::new(None),
        })
    }
    pub async fn read(
        &self,
        provider: &Digest,
        minimum: &Proof,
        minimum_epoch: u64,
    ) -> Result<Observation> {
        minimum.validate()?;
        require(
            crate::discovery::safe_integer(minimum_epoch),
            "invalid operator minimum epoch",
        )?;
        let _permit = self
            .reads
            .try_acquire()
            .map_err(|_| invalid("operator read capacity is busy"))?;
        let observed_at_ms = now_ms()?;
        let started = Instant::now();
        let mut random = [0u8; 32];
        getrandom::fill(&mut random)
            .map_err(|_| invalid("operator request randomness unavailable"))?;
        let nonce = Digest::new(blake3::hash(&random).to_hex().as_str())
            .map_err(|_| invalid("operator nonce digest rejected"))?;
        let mut response = self
            .http
            .post(self.endpoint.clone())
            .json(&serde_json::json!({
                "provider_pubkey": provider, "request_nonce": nonce,
            }))
            .send()
            .await
            .map_err(Error::Transport)?;
        require(
            response.status().is_success(),
            "operator canonical service unavailable",
        )?;
        require(
            response
                .content_length()
                .is_none_or(|v| v <= MAX_BYTES as u64),
            "operator response exceeds bound",
        )?;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(Error::Transport)? {
            require(
                body.len().saturating_add(chunk.len()) <= MAX_BYTES,
                "operator response exceeds bound",
            )?;
            body.extend_from_slice(&chunk);
        }
        let wire: Wire = serde_json::from_slice(&body)?;
        let _authenticated_requester = wire.requester;
        require(
            wire.ok
                && wire.schema_version == 1
                && wire.provider_pubkey == *provider
                && wire.request_nonce == nonce
                && wire.context.identity() == self.identity
                && crate::discovery::safe_integer(wire.context.epoch)
                && wire.context.epoch >= minimum_epoch
                && wire.proof.follows(minimum)
                && (wire.operator.status == Status::Verified) == wire.operator.proof_hash.is_some(),
            "operator observation identity differs",
        )?;
        let _proxy_lane = wire.lane;
        wire.proof.validate()?;
        require(
            started.elapsed() < Duration::from_millis(MAX_AGE_MS)
                && now_ms().is_ok_and(|now| {
                    now >= observed_at_ms && now < observed_at_ms.saturating_add(MAX_AGE_MS)
                }),
            "operator observation expired",
        )?;
        let mut high_water = self
            .high_water
            .lock()
            .map_err(|_| invalid("operator proof state unavailable"))?;
        require(
            high_water.as_ref().is_none_or(|(proof, epoch)| {
                wire.proof.follows(proof) && wire.context.epoch >= *epoch
            }),
            "operator observation regressed",
        )?;
        *high_water = Some((wire.proof.clone(), wire.context.epoch));
        Ok(Observation {
            provider: provider.clone(),
            status: wire.operator.status,
            proof: wire.proof,
            epoch: wire.context.epoch,
            observed_at_ms,
            deadline: started + Duration::from_millis(MAX_AGE_MS),
        })
    }
}
fn now_ms() -> Result<u64> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| invalid("operator clock unavailable"))?
        .as_millis();
    u64::try_from(now)
        .ok()
        .filter(|v| crate::discovery::safe_integer(*v))
        .ok_or_else(|| invalid("operator clock unavailable"))
}
