//! Proxy request parsing and read-only candidate resolution. These objects are
//! NOT payment authorizations: the gateway owner must durably bind its HTTP job,
//! authenticated key budget and buyer attempt before starting paid negotiation.
//! No native routing, signature, reservation or model call happens in this module.

use super::proxy_control::ProxyControl;
use mayhem_proto::proxy::{ProxyEndpoint, ProxyOffer, PROXY_MAX_SAFE_INTEGER};
use mayhem_proxy::{
    attempts::Digest,
    directory::PublishedOffer,
    financial::quote::{Lifetimes, PriceLimits},
    presence::Eligibility,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Semaphore;

const SELECTOR_PREFIX: &str = "proxy/offer/";
const MAX_CONTROLS_BYTES: usize = 16 * 1024;
static READS: Semaphore = Semaphore::const_new(8);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid explicit proxy request or policy")]
    Invalid,
    #[error("proxy catalog or selected offer is unavailable")]
    Catalog,
    #[error("proxy offer does not satisfy the request constraints")]
    Constraints,
    #[error("proxy offer exceeds the authorized price limits")]
    Price,
    #[error("proxy operator verification is unavailable")]
    Verification,
    #[error("proxy request metadata reads are busy")]
    Busy,
    #[error("proxy provider is not currently eligible: {0:?}")]
    Availability(Eligibility),
}

type Result<T> = std::result::Result<T, Error>;

/// Stable exact-offer selector. A display name can never select the native lane
/// or a different provider. Revisions change accepted terms, not this identity.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct Selector {
    market: Digest,
    provider: Digest,
    slot: Digest,
}

impl Selector {
    pub fn parse(model: &str) -> Result<Option<Self>> {
        if !model.starts_with("proxy/") {
            return Ok(None);
        }
        let parts = model.strip_prefix(SELECTOR_PREFIX).ok_or(Error::Invalid)?;
        if parts.len() != 64 * 3 + 2 {
            return Err(Error::Invalid);
        }
        let mut parts = parts.split('/');
        let mut next =
            || Digest::new(parts.next().ok_or(Error::Invalid)?).map_err(|_| Error::Invalid);
        let result = Self {
            market: next()?,
            provider: next()?,
            slot: next()?,
        };
        if parts.next().is_some() {
            return Err(Error::Invalid);
        }
        Ok(Some(result))
    }

    pub fn id(&self) -> String {
        format!(
            "{}/{}/{}",
            self.market.as_str(),
            self.provider.as_str(),
            self.slot.as_str()
        )
    }

    pub fn model(&self) -> String {
        format!("{SELECTOR_PREFIX}{}", self.id())
    }

    pub fn market(&self) -> &Digest {
        &self.market
    }
    pub fn provider(&self) -> &Digest {
        &self.provider
    }
    pub fn slot(&self) -> &Digest {
        &self.slot
    }
}

/// Explicit caller price/rail controls, to be intersected with the authenticated
/// account/key policy by the gateway owner. Account balance is never consent.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Controls {
    pub prices: PriceLimits,
    pub rail: mayhem_proto::proxy::ProxyRail,
    pub settlement_policy_hash: Digest,
    pub output_units: Option<u64>,
    pub minimum_context: Option<u32>,
    pub minimum_tokens_per_second: Option<u32>,
    #[serde(default)]
    pub require_verified_operator: bool,
}

/// The operator's resolved policy is passed separately; it is never read from a
/// model's prompt or copied from an upstream proposal. Its revision fences HTTP
/// idempotency even when the underlying provider request stays identical.
#[derive(Clone)]
pub struct Policy {
    revision: Digest,
    settlement_policy_hash: Digest,
    lifetimes: Lifetimes,
    request_bytes: usize,
}

impl Policy {
    pub fn new(
        revision: Digest,
        settlement_policy_hash: Digest,
        lifetimes: Lifetimes,
        request_bytes: usize,
    ) -> Result<Self> {
        if request_bytes == 0
            || request_bytes > 256 * 1024 * 1024
            || lifetimes.acceptance_epochs >= lifetimes.reservation_epochs
            || lifetimes.reservation_epochs > PROXY_MAX_SAFE_INTEGER
            || lifetimes.receipt_grace_epochs > PROXY_MAX_SAFE_INTEGER
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            revision,
            settlement_policy_hash,
            lifetimes,
            request_bytes,
        })
    }

    pub fn lifetimes(&self) -> Lifetimes {
        self.lifetimes
    }
}

/// Owned normalized envelope. Deliberately not Debug/Serialize: ordinary logs
/// must not accidentally print prompts, tools or buyer spending policy.
pub struct Request {
    selector: Selector,
    endpoint: ProxyEndpoint,
    controls: Controls,
    body: Value,
    bytes: Vec<u8>,
    policy: Policy,
}

impl Request {
    /// Call before native catalog normalization. A native request without proxy
    /// controls returns None unchanged; any malformed proxy selection fails here.
    pub fn parse(endpoint: ProxyEndpoint, mut raw: Value, policy: &Policy) -> Result<Option<Self>> {
        let model = raw
            .get("model")
            .and_then(Value::as_str)
            .ok_or(Error::Invalid)?;
        let Some(selector) = Selector::parse(model)? else {
            return if raw.get("proxy").is_some() {
                Err(Error::Invalid)
            } else {
                Ok(None)
            };
        };
        let envelope = raw.as_object_mut().ok_or(Error::Invalid)?;
        let controls = envelope.remove("proxy").ok_or(Error::Invalid)?;
        if serde_json::to_vec(&controls)
            .map_err(|_| Error::Invalid)?
            .len()
            > MAX_CONTROLS_BYTES
        {
            return Err(Error::Invalid);
        }
        let controls: Controls = serde_json::from_value(controls).map_err(|_| Error::Invalid)?;
        if controls.settlement_policy_hash != policy.settlement_policy_hash
            || controls.prices.max_total_spend_au == 0
            || controls.prices.rates.is_empty()
            || controls.prices.rates.len() > 32
            || controls.minimum_context == Some(0)
            || controls.minimum_tokens_per_second == Some(0)
            || controls
                .output_units
                .is_some_and(|n| n == 0 || n > PROXY_MAX_SAFE_INTEGER)
        {
            return Err(Error::Invalid);
        }
        // Decisions charge their defined outcomes, not an invented token rate.
        // Generative requests must explicitly authorize a bounded output amount.
        if endpoint == ProxyEndpoint::Decisions {
            if controls.output_units.is_some() || controls.minimum_tokens_per_second.is_some() {
                return Err(Error::Invalid);
            }
        } else if controls.output_units.is_none() {
            return Err(Error::Invalid);
        }
        let bytes = serde_json::to_vec(&raw).map_err(|_| Error::Invalid)?;
        if bytes.len() > policy.request_bytes {
            return Err(Error::Invalid);
        }
        Ok(Some(Self {
            selector,
            endpoint,
            controls,
            body: raw,
            bytes,
            policy: policy.clone(),
        }))
    }

    pub fn selector(&self) -> &Selector {
        &self.selector
    }
    pub fn endpoint(&self) -> ProxyEndpoint {
        self.endpoint
    }
    pub fn controls(&self) -> &Controls {
        &self.controls
    }
    pub fn provider_request(&self) -> &[u8] {
        &self.bytes
    }
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// The caller supplies a digest derived from authenticated buyer/key identity,
    /// never from a client body. Semantic JSON ordering is normalized. Every price,
    /// rail, selector, endpoint and policy control participates; changing a cap or
    /// payment currency cannot silently replay another authorization's result.
    pub fn fingerprint(&self, authenticated_owner: &Digest) -> Result<Digest> {
        let value = serde_json::json!({
            "version":1,"owner":authenticated_owner,"selector":self.selector,
            "endpoint":self.endpoint,"controls":self.controls,"request":self.body,
            "policy": {"revision":self.policy.revision,
                "settlement_policy_hash":self.policy.settlement_policy_hash,
                "request_bytes":self.policy.request_bytes,
                "acceptance_epochs":self.policy.lifetimes.acceptance_epochs,
                "reservation_epochs":self.policy.lifetimes.reservation_epochs,
                "receipt_grace_epochs":self.policy.lifetimes.receipt_grace_epochs}
        });
        let bytes = mayhem_proto::stable_json_bytes(&value).map_err(|_| Error::Invalid)?;
        let mut hash = blake3::Hasher::new_derive_key("mayhem/proxy/gateway-request/v1");
        hash.update(&(bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
        Digest::new(hash.finalize().to_hex().to_string()).map_err(|_| Error::Invalid)
    }

    fn check_candidate(
        &self,
        candidate: &PublishedOffer,
        status: Eligibility,
    ) -> Result<Candidate> {
        if candidate.id != self.selector.id()
            || candidate.lane != "proxy"
            || !candidate.active
            || !candidate.catalog_eligible
            || candidate.offer.market_id != self.selector.market.as_str()
            || candidate.offer.provider_pubkey != self.selector.provider.as_str()
            || candidate.offer.slot_id().map_err(|_| Error::Catalog)? != self.selector.slot.as_str()
        {
            return Err(Error::Catalog);
        }
        candidate
            .offer
            .validate_for_membership(&candidate.market, &candidate.membership)
            .map_err(|_| Error::Catalog)?;
        if candidate.offer.endpoint != self.endpoint
            || !candidate.offer.accepted_rails.contains(&self.controls.rail)
            || self
                .controls
                .minimum_context
                .is_some_and(|n| candidate.membership.served_context < n)
        {
            return Err(Error::Constraints);
        }
        // Public directory currently has no authenticated verification evidence.
        // Never satisfy a T4 request from a provider label or claimed model name.
        if self.controls.require_verified_operator {
            return Err(Error::Verification);
        }
        self.controls
            .prices
            .permits(&candidate.offer)
            .map_err(|_| Error::Price)?;
        if status != Eligibility::Available {
            return Err(Error::Availability(status));
        }
        let contract = candidate
            .membership
            .endpoints
            .iter()
            .find(|contract| contract.endpoint == self.endpoint)
            .ok_or(Error::Catalog)?;
        Ok(Candidate {
            offer: candidate.offer.clone(),
            endpoint_contract: Digest::new(&contract.contract_hash).map_err(|_| Error::Catalog)?,
            recipe_hash: Digest::new(&candidate.membership.recipe_hash)
                .map_err(|_| Error::Catalog)?,
        })
    }
}

/// Read-only candidate evidence, not a capacity lease or spending permission.
/// The buyer controller must revalidate its fresh quote and signed proposal.
pub struct Candidate {
    pub offer: ProxyOffer,
    pub endpoint_contract: Digest,
    pub recipe_hash: Digest,
}

/// At most eight synchronous bounded selection reads, with no admission queue or
/// all-market subscription changes. The permit survives HTTP cancellation until
/// the actual storage task finishes. No per-token reads or historical scans.
pub async fn resolve(control: Arc<ProxyControl>, request: Arc<Request>) -> Result<Candidate> {
    let permit = READS.try_acquire().map_err(|_| Error::Busy)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let candidate = control
            .catalog()
            .read()
            .map_err(|_| Error::Catalog)?
            .proxy_offer(&request.selector.id(), super::now_millis_u64())
            .map_err(|_| Error::Catalog)?
            .ok_or(Error::Catalog)?;
        let status = control
            .presence()
            .status(
                &request.selector.market,
                &request.selector.provider,
                &request.selector.slot,
                request.controls.minimum_tokens_per_second,
            )
            .map_err(|_| Error::Catalog)?;
        request.check_candidate(&candidate, status)
    })
    .await
    .map_err(|_| Error::Catalog)?
}

#[cfg(test)]
mod tests;
