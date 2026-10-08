//! Proxy-lane records. These do not relax native catalog or attestation validation.
//!
//! Digests commit to a domain-separated, bounded canonical JSON representation.
//! Validation here checks record structure/bindings, not signatures, entitlement,
//! live capacity, or independent evidence. Admission must check those separately.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{decimal_u128, MoneyAu};

pub const PROXY_SCHEMA_VERSION: u32 = 1;
pub const PROXY_MARKET_DOMAIN: &str = "mayhem/proxy/market/v1";
pub const PROXY_MEMBERSHIP_DOMAIN: &str = "mayhem/proxy/membership/v1";
pub const PROXY_OFFER_DOMAIN: &str = "mayhem/proxy/offer/v1";
pub const PROXY_ADMISSION_DOMAIN: &str = "mayhem/proxy/admission/v1";
pub const PROXY_ADMISSION_FEE_AU: MoneyAu = 10_000_000_000_000_000_000;
pub const PROXY_MAX_RECORD_BYTES: usize = 16_384;
pub const PROXY_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyLane {
    Proxy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyFamily {
    Llm,
    Decisions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum ProxyEndpoint {
    #[serde(rename = "mayhem_decisions")]
    Decisions,
    #[serde(rename = "openai_chat_completions")]
    Chat,
    #[serde(rename = "openai_completions")]
    Completions,
    #[serde(rename = "openai_responses")]
    Responses,
}

impl ProxyEndpoint {
    pub fn family(self) -> ProxyFamily {
        match self {
            Self::Decisions => ProxyFamily::Decisions,
            _ => ProxyFamily::Llm,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyRail {
    Fiat,
    Tap,
    Tnk,
}

/// Claimed lineage, never evidence of the weights an upstream actually executes.
/// Empty exact identity/revision/quantization strings mean undisclosed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyModelClaim {
    pub family_id: String,
    pub model_id: String,
    pub revision: String,
    pub quantization: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyEndpointContract {
    pub endpoint: ProxyEndpoint,
    pub contract_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyMeteringContract {
    pub policy_hash: String,
    /// Sorted, distinct units. A policy hash must resolve to supported semantics
    /// before admission; choosing a unit name alone cannot create a billing rule.
    pub units: Vec<String>,
}

/// Immutable service identity; mutable labels, prices, hardware and URLs are absent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyMarketDescriptor {
    #[serde(deserialize_with = "safe_u32")]
    pub schema_version: u32,
    pub lane: ProxyLane,
    pub creator_pubkey: String,
    pub slug: String,
    pub model: ProxyModelClaim,
    pub family: ProxyFamily,
    pub endpoints: Vec<ProxyEndpointContract>,
    pub metering: ProxyMeteringContract,
    pub pricing: ProxyPricing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyPricing {
    ProviderOffers,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyAdmissionPurpose {
    ProxyAdmissionFee,
}

/// The payment verifier signs this only after finality and a durable,
/// purpose-separated evidence claim. A valid signature is not proof of unused
/// evidence: ingress and contract also check canonical consumption/revocation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyAdmissionPermit {
    #[serde(deserialize_with = "safe_u32")]
    pub schema_version: u32,
    pub lane: ProxyLane,
    pub purpose: ProxyAdmissionPurpose,
    pub network_id: String,
    #[serde(deserialize_with = "safe_u32")]
    pub contract_version: u32,
    pub provider_pubkey: String,
    pub issuer_pubkey: String,
    pub entitlement_id: String,
    pub fee_policy_hash: String,
    pub invoice_commitment: String,
    pub evidence_commitment: String,
    pub initial_operation_digest: String,
    pub nonce: String,
    #[serde(deserialize_with = "safe_u64")]
    pub issuance_revision: u64,
    pub rail: ProxyRail,
    #[serde(with = "decimal_u128")]
    pub accepted_amount: MoneyAu,
    #[serde(with = "decimal_u128")]
    pub accepted_value_au: MoneyAu,
    #[serde(deserialize_with = "safe_u64")]
    pub valid_from_epoch: u64,
    #[serde(deserialize_with = "safe_u64")]
    pub expires_after_epoch: u64,
}

/// Public membership commits to private connection configuration, never its URL/key.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyMembership {
    #[serde(deserialize_with = "safe_u32")]
    pub schema_version: u32,
    pub lane: ProxyLane,
    pub market_id: String,
    pub provider_pubkey: String,
    #[serde(deserialize_with = "safe_u64")]
    pub revision: u64,
    pub endpoints: Vec<ProxyEndpointContract>,
    #[serde(deserialize_with = "safe_u32")]
    pub served_context: u32,
    #[serde(deserialize_with = "safe_u32")]
    pub max_concurrency: u32,
    pub recipe_hash: String,
    #[serde(deserialize_with = "safe_u64")]
    pub connection_revision: u64,
    pub capacity_group: String,
    pub accepted_rails: Vec<ProxyRail>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyRate {
    pub unit: String,
    #[serde(with = "decimal_u128")]
    pub per_unit_au: MoneyAu,
    #[serde(deserialize_with = "safe_u64")]
    pub granularity: u64,
}

/// Independent rate revision for one provider/market/endpoint/submarket.
/// Epoch/time acceptance validity belongs to a quote, not the durable offer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyOffer {
    #[serde(deserialize_with = "safe_u32")]
    pub schema_version: u32,
    pub lane: ProxyLane,
    pub market_id: String,
    pub provider_pubkey: String,
    #[serde(deserialize_with = "safe_u64")]
    pub membership_revision: u64,
    #[serde(deserialize_with = "safe_u64")]
    pub revision: u64,
    pub endpoint: ProxyEndpoint,
    pub ctx_bracket: String,
    /// Empty for no outcome class; otherwise the public decision-contract digest.
    pub outcome_class: String,
    pub metering_policy_hash: String,
    pub rates: Vec<ProxyRate>,
    #[serde(with = "decimal_u128")]
    pub per_request_au: MoneyAu,
    #[serde(with = "decimal_u128")]
    pub min_session_au: MoneyAu,
    pub accepted_rails: Vec<ProxyRail>,
}

// JSON's integral values may be spelled 1, 1.0 or 1e0. Normalize all three
// identically to JavaScript while rejecting fractions and unsafe integers.
fn safe_u64<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    struct SafeInteger;
    impl serde::de::Visitor<'_> for SafeInteger {
        type Value = u64;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a nonnegative safe JSON integer")
        }
        fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<u64, E> {
            if value <= PROXY_MAX_SAFE_INTEGER {
                Ok(value)
            } else {
                Err(E::custom("unsafe proxy integer"))
            }
        }
        fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<u64, E> {
            self.visit_u64(u64::try_from(value).map_err(|_| E::custom("negative proxy integer"))?)
        }
        fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<u64, E> {
            if value.is_finite()
                && value >= 0.0
                && value.fract() == 0.0
                && value <= PROXY_MAX_SAFE_INTEGER as f64
            {
                Ok(value as u64)
            } else {
                Err(E::custom("unsafe proxy integer"))
            }
        }
    }
    deserializer.deserialize_any(SafeInteger)
}

fn safe_u32<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    u32::try_from(safe_u64(deserializer)?)
        .map_err(|_| serde::de::Error::custom("proxy integer exceeds u32"))
}

fn ensure(valid: bool, message: &str) -> Result<(), String> {
    if valid {
        Ok(())
    } else {
        Err(message.to_owned())
    }
}

fn hex_digest(value: &str) -> Result<(), String> {
    ensure(
        value.len() == 64
            && value
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "proxy digest/key must be lowercase 64-hex",
    )
}

fn identifier(value: &str) -> Result<(), String> {
    ensure(!value.is_empty() && value.len() <= 64 && value.as_bytes()[0].is_ascii_lowercase()
        && value.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_'),
        "proxy identifier must start with a lowercase letter and contain at most 64 ASCII identifier bytes")
}

fn revision(value: u64) -> Result<(), String> {
    ensure(
        value > 0 && value <= PROXY_MAX_SAFE_INTEGER,
        "proxy revision must be a positive safe integer",
    )
}

fn rails(values: &[ProxyRail]) -> Result<(), String> {
    ensure(
        !values.is_empty() && values.len() <= 3 && values.windows(2).all(|w| w[0] < w[1]),
        "proxy rails must be nonempty, distinct and sorted",
    )
}

fn endpoints(values: &[ProxyEndpointContract]) -> Result<(), String> {
    ensure(
        !values.is_empty()
            && values.len() <= 3
            && values.windows(2).all(|w| w[0].endpoint < w[1].endpoint),
        "proxy endpoints must be nonempty, distinct and sorted",
    )?;
    for endpoint in values {
        hex_digest(&endpoint.contract_hash)?;
    }
    Ok(())
}

fn units(values: &[String]) -> Result<(), String> {
    ensure(
        !values.is_empty() && values.len() <= 16 && values.windows(2).all(|w| w[0] < w[1]),
        "proxy metering units must be nonempty, distinct and sorted",
    )?;
    for value in values {
        identifier(value)?;
    }
    Ok(())
}

impl ProxyMarketDescriptor {
    pub fn validate(&self) -> Result<(), String> {
        ensure(
            self.schema_version == PROXY_SCHEMA_VERSION,
            "unsupported proxy schema version",
        )?;
        hex_digest(&self.creator_pubkey)?;
        identifier(&self.slug)?;
        identifier(&self.model.family_id)?;
        for text in [
            &self.model.model_id,
            &self.model.revision,
            &self.model.quantization,
        ] {
            ensure(
                text.len() <= 512 && !text.chars().any(char::is_control),
                "invalid proxy model claim",
            )?;
        }
        endpoints(&self.endpoints)?;
        ensure(
            self.endpoints
                .iter()
                .all(|e| e.endpoint.family() == self.family),
            "proxy endpoint/family mismatch",
        )?;
        hex_digest(&self.metering.policy_hash)?;
        units(&self.metering.units)?;
        canonical_body(self).map(|_| ())
    }

    pub fn id(&self) -> Result<String, String> {
        self.validate()?;
        digest(PROXY_MARKET_DOMAIN, self)
    }

    pub fn public_handle(&self) -> Result<String, String> {
        self.validate()?;
        Ok(format!("proxy/{}/{}", self.creator_pubkey, self.slug))
    }

    pub fn signing_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        signing_bytes(PROXY_MARKET_DOMAIN, self)
    }
}

impl ProxyMembership {
    pub fn validate(&self) -> Result<(), String> {
        ensure(
            self.schema_version == PROXY_SCHEMA_VERSION,
            "unsupported proxy schema version",
        )?;
        for value in [
            &self.market_id,
            &self.provider_pubkey,
            &self.recipe_hash,
            &self.capacity_group,
        ] {
            hex_digest(value)?;
        }
        revision(self.revision)?;
        revision(self.connection_revision)?;
        endpoints(&self.endpoints)?;
        ensure(
            self.served_context > 0 && self.max_concurrency > 0,
            "proxy context and concurrency must be positive",
        )?;
        rails(&self.accepted_rails)?;
        canonical_body(self).map(|_| ())
    }

    pub fn validate_for_market(&self, market: &ProxyMarketDescriptor) -> Result<(), String> {
        self.validate()?;
        ensure(
            self.market_id == market.id()?,
            "proxy membership market mismatch",
        )?;
        ensure(
            self.endpoints.iter().all(|e| market.endpoints.contains(e)),
            "proxy membership endpoint contract mismatch",
        )
    }

    pub fn digest(&self) -> Result<String, String> {
        self.validate()?;
        digest(PROXY_MEMBERSHIP_DOMAIN, self)
    }

    pub fn signing_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        signing_bytes(PROXY_MEMBERSHIP_DOMAIN, self)
    }
}

impl ProxyAdmissionPermit {
    pub fn validate(&self) -> Result<(), String> {
        ensure(
            self.schema_version == PROXY_SCHEMA_VERSION,
            "unsupported proxy schema version",
        )?;
        ensure(
            !self.network_id.is_empty()
                && self.network_id.len() <= 128
                && self.network_id.bytes().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_'
                }),
            "invalid proxy admission network",
        )?;
        ensure(
            self.contract_version > 0,
            "invalid proxy admission contract version",
        )?;
        for value in [
            &self.provider_pubkey,
            &self.issuer_pubkey,
            &self.entitlement_id,
            &self.fee_policy_hash,
            &self.invoice_commitment,
            &self.evidence_commitment,
            &self.initial_operation_digest,
            &self.nonce,
        ] {
            hex_digest(value)?;
        }
        revision(self.issuance_revision)?;
        revision(self.valid_from_epoch)?;
        revision(self.expires_after_epoch)?;
        ensure(
            self.expires_after_epoch >= self.valid_from_epoch,
            "invalid proxy admission epoch window",
        )?;
        ensure(
            self.accepted_amount > 0 && self.accepted_value_au == PROXY_ADMISSION_FEE_AU,
            "proxy admission must attest exactly the agreed fee allocation",
        )?;
        canonical_body(self).map(|_| ())
    }

    pub fn digest(&self) -> Result<String, String> {
        self.validate()?;
        digest(PROXY_ADMISSION_DOMAIN, self)
    }

    pub fn signing_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        signing_bytes(PROXY_ADMISSION_DOMAIN, self)
    }
}

impl ProxyOffer {
    pub fn validate(&self) -> Result<(), String> {
        ensure(
            self.schema_version == PROXY_SCHEMA_VERSION,
            "unsupported proxy schema version",
        )?;
        for value in [
            &self.market_id,
            &self.provider_pubkey,
            &self.metering_policy_hash,
        ] {
            hex_digest(value)?;
        }
        revision(self.membership_revision)?;
        revision(self.revision)?;
        identifier(&self.ctx_bracket)?;
        if !self.outcome_class.is_empty() {
            hex_digest(&self.outcome_class)?;
            ensure(
                self.endpoint == ProxyEndpoint::Decisions,
                "outcome class requires a decisions endpoint",
            )?;
        }
        ensure(
            !self.rates.is_empty() && self.rates.len() <= 16,
            "invalid proxy rate count",
        )?;
        units(
            &self
                .rates
                .iter()
                .map(|r| r.unit.clone())
                .collect::<Vec<_>>(),
        )?;
        for rate in &self.rates {
            ensure(
                rate.granularity > 0 && rate.granularity <= PROXY_MAX_SAFE_INTEGER,
                "proxy rate granularity must be a positive safe integer",
            )?;
        }
        ensure(
            self.per_request_au > 0
                || self.min_session_au > 0
                || self.rates.iter().any(|r| r.per_unit_au > 0),
            "free proxy offers require a separately supported accounting policy",
        )?;
        rails(&self.accepted_rails)?;
        canonical_body(self).map(|_| ())
    }

    pub fn validate_for_membership(
        &self,
        market: &ProxyMarketDescriptor,
        member: &ProxyMembership,
    ) -> Result<(), String> {
        self.validate()?;
        member.validate_for_market(market)?;
        ensure(
            self.market_id == member.market_id
                && self.provider_pubkey == member.provider_pubkey
                && self.membership_revision == member.revision,
            "proxy offer membership binding mismatch",
        )?;
        ensure(
            member.endpoints.iter().any(|e| e.endpoint == self.endpoint),
            "proxy offer endpoint unavailable in membership",
        )?;
        ensure(
            self.metering_policy_hash == market.metering.policy_hash,
            "proxy offer metering policy mismatch",
        )?;
        ensure(
            self.rates
                .iter()
                .map(|r| &r.unit)
                .eq(market.metering.units.iter()),
            "proxy offer must price every metering unit exactly once",
        )?;
        ensure(
            self.accepted_rails
                .iter()
                .all(|r| member.accepted_rails.contains(r)),
            "proxy offer rail not enabled by membership",
        )
    }

    /// Call against the durable revision high-water mark, including withdrawals.
    pub fn validate_revision_after(&self, last_revision: u64) -> Result<(), String> {
        self.validate()?;
        ensure(self.revision > last_revision, "stale proxy offer revision")
    }

    pub fn digest(&self) -> Result<String, String> {
        self.validate()?;
        digest(PROXY_OFFER_DOMAIN, self)
    }

    pub fn signing_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        signing_bytes(PROXY_OFFER_DOMAIN, self)
    }

    /// Metering input must be cumulative for one logical request/session and already
    /// independently validated by the endpoint policy. Do not add this subtotal
    /// across stream chunks/checkpoints: that repeats fixed charges and rounding.
    /// Every supplied unit needs an explicit price; unknown usage never becomes free.
    /// This is an exact offer subtotal, not permission to settle or omit platform fees.
    pub fn cost(&self, usage: &BTreeMap<String, u64>) -> Result<MoneyAu, String> {
        self.validate()?;
        ensure(usage.len() <= self.rates.len(), "unpriced proxy usage unit")?;
        let mut cost = self.per_request_au;
        for (unit, count) in usage {
            ensure(
                *count <= PROXY_MAX_SAFE_INTEGER,
                "proxy usage exceeds safe integer range",
            )?;
            let rate = self
                .rates
                .iter()
                .find(|r| &r.unit == unit)
                .ok_or("unpriced proxy usage unit")?;
            let numerator = rate
                .per_unit_au
                .checked_mul(u128::from(*count))
                .ok_or("proxy cost overflow")?;
            let divisor = u128::from(rate.granularity);
            // Avoid (numerator + divisor - 1), which can overflow a valid amount.
            let amount = numerator / divisor + u128::from(numerator % divisor != 0);
            cost = cost.checked_add(amount).ok_or("proxy cost overflow")?;
        }
        Ok(cost.max(self.min_session_au))
    }
}

fn digest<T: Serialize>(domain: &str, value: &T) -> Result<String, String> {
    Ok(blake3::hash(&signing_bytes(domain, value)?)
        .to_hex()
        .to_string())
}

fn signing_bytes<T: Serialize>(domain: &str, value: &T) -> Result<Vec<u8>, String> {
    let mut bytes = domain.as_bytes().to_vec();
    bytes.push(0);
    bytes.extend_from_slice(&canonical_body(value)?);
    Ok(bytes)
}

/// No floats, unsafe JS integers, unknown fields or optional-field omission in the
/// signed records. Object keys are ASCII and sorted; arrays preserve validated order.
fn canonical_body<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    fn ordered(value: Value) -> Result<Value, String> {
        match value {
            Value::Object(map) => {
                let sorted = map.into_iter().collect::<BTreeMap<_, _>>();
                let mut result = serde_json::Map::new();
                for (key, value) in sorted {
                    result.insert(key, ordered(value)?);
                }
                Ok(Value::Object(result))
            }
            Value::Array(values) => Ok(Value::Array(
                values.into_iter().map(ordered).collect::<Result<_, _>>()?,
            )),
            Value::Number(ref n) if n.as_u64().is_none_or(|v| v > PROXY_MAX_SAFE_INTEGER) => {
                Err("unsafe proxy canonical number".into())
            }
            value => Ok(value),
        }
    }
    let value = serde_json::to_value(value).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec(&ordered(value)?).map_err(|e| e.to_string())?;
    ensure(
        bytes.len() <= PROXY_MAX_RECORD_BYTES,
        "proxy record exceeds size bound",
    )?;
    Ok(bytes)
}
