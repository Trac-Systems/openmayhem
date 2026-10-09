//! Retained buyer intent, checked again against the actual selected publication.
//! A profile never grants evidence, model identity, capacity or spending authority.
use crate::{
    directory::PublishedOffer,
    financial::quote::PriceLimits,
    registry::{self, Control, Predicate},
    require, Result,
};
use mayhem_proto::{
    proxy::{ProxyEndpoint, ProxyLane, ProxyRail, PROXY_MAX_SAFE_INTEGER},
    MoneyAu,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    ExactOffer {
        offer_id: String,
    },
    ExactMarket {
        market_id: String,
    },
    Category {
        family_ids: Vec<String>,
        variants: Vec<String>,
        tags: Vec<String>,
        #[serde(deserialize_with = "nullable")]
        market_allowlist: Option<Vec<String>>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Providers {
    #[serde(deserialize_with = "nullable")]
    pub allow: Option<Vec<String>>,
    pub deny: Vec<String>,
    pub require_verified_operator: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettlementPolicy {
    pub rail: ProxyRail,
    pub settlement_policy_hash: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Constraints {
    #[serde(deserialize_with = "nullable")]
    pub minimum_context: Option<u32>,
    #[serde(deserialize_with = "nullable")]
    pub minimum_tokens_per_second: Option<u32>,
    #[serde(deserialize_with = "nullable")]
    pub output_units: Option<u64>,
    pub capabilities: Vec<Predicate>,
    pub request_controls: Vec<Control>,
    pub data_handling: Vec<Predicate>,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ranking {
    LowestEstimatedCost,
    PreferredSpeed,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Continuity {
    RetainCompatible,
    ReselectPerRequest,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub schema_version: u32,
    pub lane: ProxyLane,
    pub endpoint: ProxyEndpoint,
    pub target: Target,
    pub providers: Providers,
    pub allowed_rails: Vec<ProxyRail>,
    pub prices: PriceLimits,
    // Retained in the intent and fingerprint. Retail enforces this cap in its
    // existing account authorization; Core wholesale AU are not retail dollars.
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub max_retail_cost_micro: MoneyAu,
    pub settlement_policies: Vec<SettlementPolicy>,
    pub constraints: Constraints,
    pub ranking: Ranking,
    pub continuity: Continuity,
}
fn nullable<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}
fn ordered<T: Ord>(v: &[T]) -> bool {
    v.windows(2).all(|p| p[0] < p[1])
}
fn hashes(v: &[String], min: usize) -> bool {
    v.len() >= min && v.len() <= 128 && ordered(v) && v.iter().all(|s| crate::discovery::hex(s))
}
fn ids(v: &[String]) -> bool {
    v.len() <= 64 && ordered(v) && v.iter().all(|s| registry::identifier(s))
}
fn family(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
}
fn rail(r: ProxyRail) -> &'static str {
    match r {
        ProxyRail::Fiat => "fiat",
        ProxyRail::Tap => "tap",
        ProxyRail::Tnk => "tnk",
    }
}
fn operator(op: registry::Operator) -> &'static str {
    match op {
        registry::Operator::Eq => "eq",
        registry::Operator::Gte => "gte",
        registry::Operator::Lte => "lte",
        registry::Operator::ContainsAll => "contains_all",
    }
}
impl Policy {
    pub fn validate(&self) -> Result<()> {
        let c = &self.constraints;
        require(
            self.schema_version == 1
                && self.lane == ProxyLane::Proxy
                && serde_json::to_vec(self)?.len() <= 16 * 1024
                && self.max_retail_cost_micro > 0
                && self.max_retail_cost_micro <= i64::MAX as u128,
            "invalid routing profile envelope",
        )?;
        require(
            self.providers.allow.as_ref().is_none_or(|v| hashes(v, 1))
                && hashes(&self.providers.deny, 0)
                && self
                    .providers
                    .allow
                    .as_ref()
                    .is_none_or(|v| v.iter().all(|id| !self.providers.deny.contains(id))),
            "invalid routing provider restrictions",
        )?;
        match &self.target {
            Target::ExactOffer { offer_id } => {
                let parts: Vec<_> = offer_id.split('/').collect();
                require(
                    offer_id.len() == 194
                        && parts.len() == 3
                        && parts.iter().all(|s| crate::discovery::hex(s)),
                    "invalid exact routing offer",
                )?;
                require(
                    self.provider_allowed(parts[1]),
                    "profile excludes its exact provider",
                )?;
            }
            Target::ExactMarket { market_id } => require(
                crate::discovery::hex(market_id),
                "invalid exact routing market",
            )?,
            Target::Category {
                family_ids,
                variants,
                tags,
                market_allowlist,
            } => require(
                !family_ids.is_empty()
                    && family_ids.len() <= 64
                    && ordered(family_ids)
                    && family_ids.iter().all(|s| family(s))
                    && ids(variants)
                    && ids(tags)
                    && market_allowlist.as_ref().is_none_or(|v| hashes(v, 1)),
                "invalid routing category",
            )?,
        }
        require(
            !self.allowed_rails.is_empty()
                && self.allowed_rails.len() <= 3
                && ordered(
                    &self
                        .allowed_rails
                        .iter()
                        .copied()
                        .map(rail)
                        .collect::<Vec<_>>(),
                )
                && self.settlement_policies.len() == self.allowed_rails.len()
                && self
                    .settlement_policies
                    .iter()
                    .zip(&self.allowed_rails)
                    .all(|(p, r)| p.rail == *r && crate::discovery::hex(&p.settlement_policy_hash)),
            "invalid routing rail policies",
        )?;
        let p = &self.prices;
        require(
            p.max_total_spend_au > 0
                && p.per_request_au <= p.max_total_spend_au
                && p.min_session_au <= p.max_total_spend_au
                && !p.rates.is_empty()
                && p.rates.len() <= 16
                && ordered(&p.rates.iter().map(|r| &r.unit).collect::<Vec<_>>())
                && p.rates.iter().all(|r| {
                    family(&r.unit) && r.granularity > 0 && r.granularity <= PROXY_MAX_SAFE_INTEGER
                }),
            "invalid routing price limits",
        )?;
        require(
            c.minimum_context != Some(0)
                && c.minimum_tokens_per_second != Some(0)
                && c.output_units
                    .is_none_or(|v| v > 0 && v <= PROXY_MAX_SAFE_INTEGER)
                && if self.endpoint == ProxyEndpoint::Decisions {
                    c.output_units.is_none() && c.minimum_tokens_per_second.is_none()
                } else {
                    c.output_units.is_some()
                },
            "invalid routing context/output constraints",
        )?;
        let mut references = BTreeMap::new();
        for predicates in [&c.capabilities, &c.data_handling] {
            require(
                predicates.len() <= 32
                    && ordered(
                        &predicates
                            .iter()
                            .map(|p| (&p.field_id, operator(p.operator)))
                            .collect::<Vec<_>>(),
                    ),
                "invalid routing predicates",
            )?;
            for p in predicates {
                p.validate()?;
                Self::reference(&mut references, &p.field_id, p.schema_revision)?;
            }
        }
        require(
            c.request_controls.len() <= 32
                && ordered(
                    &c.request_controls
                        .iter()
                        .map(|v| &v.field_id)
                        .collect::<Vec<_>>(),
                ),
            "invalid routing controls",
        )?;
        for c in &c.request_controls {
            c.validate()?;
            Self::reference(&mut references, &c.field_id, c.schema_revision)?;
        }
        Ok(())
    }
    fn reference(refs: &mut BTreeMap<String, u32>, id: &str, rev: u32) -> Result<()> {
        require(
            refs.insert(id.into(), rev).is_none_or(|old| old == rev),
            "mixed routing field revisions",
        )
    }
    fn provider_allowed(&self, id: &str) -> bool {
        !self.providers.deny.iter().any(|s| s == id)
            && self
                .providers
                .allow
                .as_ref()
                .is_none_or(|v| v.iter().any(|s| s == id))
    }
    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        let mut bytes = b"mayhem/proxy/routing-profile/v1\0".to_vec();
        bytes.extend(mayhem_proto::stable_json_bytes(&serde_json::to_value(
            self,
        )?)?);
        Ok(blake3::hash(&bytes).to_hex().to_string())
    }
    /// Current exact-offer execution must not relax a saved profile after a
    /// category resolver or UI selects a supplier. Fresh presence is separate.
    pub fn check_offer(
        &self,
        candidate: &PublishedOffer,
        endpoint: ProxyEndpoint,
        selected_rail: ProxyRail,
    ) -> Result<()> {
        self.validate()?;
        require(
            candidate.lane == "proxy"
                && candidate.active
                && candidate.catalog_eligible
                && self.endpoint == endpoint
                && candidate.offer.endpoint == endpoint
                && self.allowed_rails.contains(&selected_rail)
                && candidate.offer.accepted_rails.contains(&selected_rail)
                && self.provider_allowed(&candidate.offer.provider_pubkey),
            "selected supplier violates routing profile",
        )?;
        let target = match &self.target {
            Target::ExactOffer { offer_id } => candidate.id == *offer_id,
            Target::ExactMarket { market_id } => candidate.offer.market_id == *market_id,
            Target::Category {
                family_ids,
                market_allowlist,
                ..
            } => {
                family_ids.contains(&candidate.market.model.family_id)
                    && market_allowlist
                        .as_ref()
                        .is_none_or(|v| v.contains(&candidate.offer.market_id))
            }
        };
        require(
            target
                && self
                    .constraints
                    .minimum_context
                    .is_none_or(|n| candidate.membership.served_context >= n),
            "selected model violates routing profile",
        )?;
        self.prices.permits(&candidate.offer)
    }
    /// Until the authenticated registry/evidence resolver is wired, these
    /// requirements remain explicit errors, never silently ignored constraints.
    pub fn requires_evidence_resolution(&self) -> bool {
        self.providers.require_verified_operator
            || !self.constraints.capabilities.is_empty()
            || !self.constraints.data_handling.is_empty()
            || !self.constraints.request_controls.is_empty()
            || matches!(&self.target, Target::Category { variants, tags, .. } if !variants.is_empty() || !tags.is_empty())
    }
}
