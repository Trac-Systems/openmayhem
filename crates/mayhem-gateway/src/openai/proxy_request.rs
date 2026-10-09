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
const MAX_CONTROLS_BYTES: usize = 32 * 1024;
static READS: Semaphore = Semaphore::const_new(8);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid explicit proxy request or policy")]
    Invalid,
    #[error("proxy settlement policy differs from the configured policy")]
    SettlementPolicyMismatch,
    #[error("proxy catalog or selected offer is unavailable")]
    Catalog,
    #[error("proxy offer does not satisfy the request constraints")]
    Constraints,
    #[error("proxy offer exceeds the authorized price limits")]
    Price,
    #[error("proxy operator verification is unavailable")]
    Verification,
    #[error("routing profile requires authenticated capability or taxonomy evidence that is unavailable")]
    ProfileEvidence,
    #[error("proxy request metadata reads are busy")]
    Busy,
    #[error("proxy provider is not currently eligible: {0:?}")]
    Availability(Eligibility),
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryRelease {
    pub release_id: String,
    pub release_hash: Digest,
}
impl RegistryRelease {
    fn validate(&self) -> Result<()> {
        let id = self.release_id.as_bytes();
        if id.len() != 36
            || !id.iter().enumerate().all(|(i, c)| {
                if [8, 13, 18, 23].contains(&i) {
                    *c == b'-'
                } else {
                    c.is_ascii_digit() || (b'a'..=b'f').contains(c)
                }
            })
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}

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
    /// Exact retained buyer policy, never metadata copied from the supplier.
    /// Absence serializes exactly as before, preserving existing replay hashes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<mayhem_proxy::routing::Policy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_release: Option<RegistryRelease>,
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
    pub(crate) fn request_byte_limit(&self) -> usize {
        self.request_bytes
    }

    pub fn settlement_policy_hash(&self) -> &Digest {
        &self.settlement_policy_hash
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
    /// Settlement policy compatibility is checked separately so the HTTP owner
    /// can preserve an existing purchase or durably fence a rejected request ID.
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
        if let Some(pin) = &controls.registry_release {
            pin.validate()?;
            if controls.profile.as_ref().is_none_or(|p| {
                p.constraints.request_controls.is_empty() && p.constraints.capabilities.is_empty()
            }) {
                return Err(Error::Invalid);
            }
        }
        if controls.prices.max_total_spend_au == 0
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
        if let Some(profile) = &controls.profile {
            profile.validate().map_err(|_| Error::Invalid)?;
            if profile.endpoint != endpoint
                || !profile.allowed_rails.contains(&controls.rail)
                || !profile.settlement_policies.iter().any(|p| {
                    p.rail == controls.rail
                        && p.settlement_policy_hash == controls.settlement_policy_hash.as_str()
                })
                || controls.prices.max_total_spend_au > profile.prices.max_total_spend_au
                || profile
                    .constraints
                    .minimum_context
                    .is_some_and(|n| controls.minimum_context.unwrap_or(0) < n)
                || profile
                    .constraints
                    .minimum_tokens_per_second
                    .is_some_and(|n| controls.minimum_tokens_per_second.unwrap_or(0) < n)
                || profile
                    .constraints
                    .output_units
                    .is_some_and(|n| controls.output_units.is_none_or(|v| v > n))
            {
                return Err(Error::Constraints);
            }
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
    pub(super) fn provider_value(&self) -> &Value {
        &self.body
    }
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn check_settlement_policy(&self) -> Result<()> {
        if self.controls.settlement_policy_hash != self.policy.settlement_policy_hash {
            return Err(Error::SettlementPolicyMismatch);
        }
        Ok(())
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

    #[cfg(test)]
    fn check_candidate(
        &self,
        candidate: &PublishedOffer,
        status: Eligibility,
    ) -> Result<Candidate> {
        let result = self.check_offer(candidate)?;
        if status != Eligibility::Available {
            return Err(Error::Availability(status));
        }
        Ok(result)
    }
    #[cfg(test)]
    fn check_offer(&self, candidate: &PublishedOffer) -> Result<Candidate> {
        self.check_offer_membership(candidate, None, None)
    }
    fn check_offer_membership(
        &self,
        candidate: &PublishedOffer,
        membership: Option<&mayhem_proxy::registry::publication::taxonomy::Membership>,
        operator: Option<&mayhem_proxy::operator::Observation>,
    ) -> Result<Candidate> {
        self.check_settlement_policy()?;
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
        if let Some(profile) = &self.controls.profile {
            profile
                .check_offer_with_taxonomy(candidate, self.endpoint, self.controls.rail, membership)
                .map_err(|_| Error::Constraints)?;
            if super::proxy_buyer::evidence::unsupported(profile) {
                return Err(Error::ProfileEvidence);
            }
        }
        if candidate.offer.endpoint != self.endpoint
            || !candidate.offer.accepted_rails.contains(&self.controls.rail)
            || self
                .controls
                .minimum_context
                .is_some_and(|n| candidate.membership.served_context < n)
        {
            return Err(Error::Constraints);
        }
        if self.requires_operator()
            && !operator.is_some_and(|evidence| {
                Digest::new(&candidate.offer.provider_pubkey)
                    .is_ok_and(|provider| evidence.permits(&provider))
            })
        {
            return Err(Error::Verification);
        }
        self.controls
            .prices
            .permits(&candidate.offer)
            .map_err(|_| Error::Price)?;
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

    pub(crate) fn requires_operator(&self) -> bool {
        self.controls.require_verified_operator
            || self
                .controls
                .profile
                .as_ref()
                .is_some_and(|p| p.providers.require_verified_operator)
    }
}

/// One immutable catalog binding and an independent, truthful availability read.
pub struct EstimateCandidate {
    pub candidate: Candidate,
    pub published: PublishedOffer,
    pub network: mayhem_proxy::discovery::Identity,
    pub expires_at_ms: u64,
    pub availability: mayhem_proxy::presence::Observation,
}
pub async fn resolve_estimate(
    control: Arc<ProxyControl>,
    request: Arc<Request>,
) -> Result<EstimateCandidate> {
    let permit = READS.try_acquire().map_err(|_| Error::Busy)?;
    let read_control = control.clone();
    let read_request = request.clone();
    let (published, network, mut expires_at_ms, availability, proof, epoch) =
        tokio::task::spawn_blocking(move || {
            let control = read_control;
            let request = read_request;
            let _permit = permit;
            let now = super::now_millis_u64();
            let catalog = control.catalog().read().map_err(|_| Error::Catalog)?;
            let registered = mayhem_proxy::presence::Registered::read(
                &catalog,
                &request.selector.market,
                &request.selector.provider,
                &request.selector.slot,
                now,
            )
            .map_err(|_| Error::Catalog)?;
            let published = catalog
                .proxy_offer(&request.selector.id(), now)
                .map_err(|_| Error::Catalog)?
                .ok_or(Error::Catalog)?;
            let status = catalog.status();
            let committed = status.committed.as_ref().ok_or(Error::Catalog)?;
            let network = committed.context.identity();
            let expires_at_ms = committed
                .observed_at_ms
                .ok_or(Error::Catalog)?
                .saturating_add(mayhem_proxy::presence::CATALOG_AGE_MS);
            let availability = control
                .presence()
                .observe_registered(&registered, request.controls.minimum_tokens_per_second)
                .map_err(|_| Error::Catalog)?;
            Ok((
                published,
                network,
                expires_at_ms,
                availability,
                committed.proof.clone(),
                committed.context.epoch,
            ))
        })
        .await
        .map_err(|_| Error::Catalog)??;
    let membership = taxonomy_membership(&control, &request, &published, &network).await?;
    let operator = if request.requires_operator() {
        let provider = Digest::new(&published.offer.provider_pubkey).map_err(|_| Error::Catalog)?;
        let observed = control
            .operator()
            .read(&provider, &proof, epoch)
            .await
            .map_err(|_| Error::ProfileEvidence)?;
        expires_at_ms = expires_at_ms.min(observed.expires_at_ms());
        Some(observed)
    } else {
        None
    };
    let candidate =
        request.check_offer_membership(&published, membership.as_ref(), operator.as_ref())?;
    Ok(EstimateCandidate {
        candidate,
        published,
        network,
        expires_at_ms,
        availability,
    })
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
async fn taxonomy_membership(
    control: &ProxyControl,
    request: &Request,
    published: &PublishedOffer,
    network: &mayhem_proxy::discovery::Identity,
) -> Result<Option<mayhem_proxy::registry::publication::taxonomy::Membership>> {
    let Some(mayhem_proxy::routing::Policy {
        target: mayhem_proxy::routing::Target::TaxonomyCategory { taxonomy, .. },
        ..
    }) = &request.controls.profile
    else {
        return Ok(None);
    };
    let reader = control.registry().ok_or(Error::ProfileEvidence)?;
    let pin = reader
        .pin_taxonomy(taxonomy)
        .await
        .map_err(|_| Error::ProfileEvidence)?;
    pin.check_network(network)
        .map_err(|_| Error::ProfileEvidence)?;
    let model = mayhem_proxy::registry::publication::taxonomy::Model::from_offer(published);
    let mut matches = reader
        .taxonomy_match(&pin, taxonomy, &[model.clone()])
        .await
        .map_err(|_| Error::ProfileEvidence)?;
    let member = matches.pop().ok_or(Error::ProfileEvidence)?;
    if !member.contains(taxonomy, &model) {
        return Err(Error::Constraints);
    }
    Ok(Some(member))
}
pub async fn resolve(control: Arc<ProxyControl>, request: Arc<Request>) -> Result<Candidate> {
    // Same immutable publication/identity and default presence policy as quotes.
    // The exact administrative membership check occurs before new admission;
    // the caller performs retained original-job replay before invoking this.
    let selected = resolve_estimate(control, request).await?;
    if selected.availability.status != Eligibility::Available {
        return Err(Error::Availability(selected.availability.status));
    }
    let now = super::now_millis_u64();
    if now >= selected.expires_at_ms || selected.availability.expires_at_ms.is_none_or(|e| now >= e)
    {
        return Err(Error::Catalog);
    }
    Ok(selected.candidate)
}

#[cfg(test)]
mod tests;
