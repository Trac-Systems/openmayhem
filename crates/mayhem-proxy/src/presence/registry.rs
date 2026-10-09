use super::*;
use crate::{catalog::CatalogRead, discovery::CATALOG_PREFIX};
use mayhem_proto::proxy::{ProxyMarketDescriptor, ProxyMembership};
use serde_json::Value;

/// Current canonical registration, not supplied by a heartbeat. Keep short-lived;
/// routing must re-resolve it from the latest complete catalog snapshot. Final
/// acceptance still verifies the exact payment terms and atomically takes a slot.
pub struct Registered {
    pub(super) network: discovery::Identity,
    pub(super) offer: ProxyOffer,
    pub(super) member: ProxyMembership,
    pub(super) expires_ms: u64,
    pub(super) observed_ms: u64,
    pub(super) deadline: Instant,
}
impl Registered {
    pub fn read(
        catalog: &CatalogRead,
        market: &Digest,
        provider: &Digest,
        slot: &Digest,
        now_ms: u64,
    ) -> Result<Self> {
        let read_started = Instant::now();
        let status = catalog.status();
        require(
            status.discovery_is_fresh(now_ms, CATALOG_AGE_MS),
            "proxy catalog is not current",
        )?;
        let committed = status
            .committed
            .ok_or_else(|| invalid("proxy catalog missing"))?;
        let network = committed.context.identity();
        let get = |key: &str| catalog.get(&format!("{CATALOG_PREFIX}{key}"));
        let required = |key: &str| get(key)?.ok_or_else(|| invalid("proxy registration missing"));
        let current = required("network/current")?;
        let mut expected = serde_json::to_value(&network)?;
        expected["enabled"] = Value::Bool(true);
        require(current == expected, "proxy network disabled or changed")?;
        let owner = required(&format!("providers/{}", provider.as_str()))?;
        let admission = owner["admission_id"]
            .as_str()
            .ok_or_else(|| invalid("proxy admission missing"))?;
        require(
            get(&format!("provider_status/{}", provider.as_str()))?.is_none()
                && get(&format!("admission_status/{admission}"))?.is_none(),
            "proxy admission revoked",
        )?;
        let member = required(&format!(
            "memberships/{}/{}",
            market.as_str(),
            provider.as_str()
        ))?;
        let offer = required(&format!(
            "offers/{}/{}/{}",
            market.as_str(),
            provider.as_str(),
            slot.as_str()
        ))?;
        require(
            member["active"] == true && offer["active"] == true,
            "proxy route withdrawn",
        )?;
        let member: ProxyMembership = serde_json::from_value(member["member"].clone())?;
        let offer: ProxyOffer = serde_json::from_value(offer["offer"].clone())?;
        let descriptor: ProxyMarketDescriptor =
            serde_json::from_value(required(&format!("markets/{}", market.as_str()))?)?;
        offer
            .validate_for_membership(&descriptor, &member)
            .map_err(invalid)?;
        require(
            offer.market_id == market.as_str()
                && offer.provider_pubkey == provider.as_str()
                && offer.slot_id().map_err(invalid)? == slot.as_str(),
            "proxy registered route differs",
        )?;
        // Catalog policies are authoritative; a signed provider cannot reactivate
        // a disabled family, endpoint contract or metering policy.
        let family = &descriptor.model.family_id;
        require(
            required(&format!("families/{family}"))?["enabled"] == true,
            "proxy family disabled",
        )?;
        let contract = member
            .endpoints
            .iter()
            .find(|e| e.endpoint == offer.endpoint)
            .ok_or_else(|| invalid("proxy endpoint missing"))?;
        let endpoint = required(&format!("endpoints/{}", contract.contract_hash))?;
        require(
            endpoint["enabled"] == true
                && endpoint["endpoint"] == serde_json::to_value(offer.endpoint)?
                && endpoint["max_context"]
                    .as_u64()
                    .is_some_and(|max| u64::from(member.served_context) <= max)
                && endpoint["ctx_brackets"]
                    .as_array()
                    .is_some_and(|items| items.contains(&Value::String(offer.ctx_bracket.clone())))
                && endpoint["outcome_classes"].as_array().is_some_and(|items| {
                    items.contains(&Value::String(offer.outcome_class.clone()))
                })
                && required(&format!("metering/{}", offer.metering_policy_hash))?["enabled"]
                    == true,
            "proxy endpoint or metering policy disabled",
        )?;
        let observed_ms = committed.observed_at_ms.unwrap_or(0);
        let expires_ms = observed_ms.saturating_add(CATALOG_AGE_MS);
        Ok(Self {
            network,
            offer,
            member,
            expires_ms,
            observed_ms,
            deadline: read_started
                + std::time::Duration::from_millis(expires_ms.saturating_sub(now_ms)),
        })
    }
    pub fn offer(&self) -> &ProxyOffer {
        &self.offer
    }
    pub(super) fn check(&self, body: &Body, now_ms: u64) -> Result<()> {
        require(
            now_ms >= self.observed_ms
                && now_ms < self.expires_ms
                && Instant::now() < self.deadline
                && body.network == self.network
                && body.provider.as_str() == self.offer.provider_pubkey
                && body.market.as_str() == self.offer.market_id
                && body.slot.as_str() == self.offer.slot_id().map_err(invalid)?
                && body.offer.as_str() == self.offer.digest().map_err(invalid)?
                && body.offer_revision == self.offer.revision
                && body.membership.as_str() == self.member.digest().map_err(invalid)?
                && body.membership_revision == self.member.revision
                && body.allowance <= self.member.max_concurrency,
            "proxy presence differs from current canonical registration",
        )
    }
}
