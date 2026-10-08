//! Explicit proxy financial wire records. Validation and signatures do not reserve
//! money or prove canonical admission, usage, capacity or payout readiness. Shared
//! accounting must independently authorize those facts. Native receipts are unchanged.

use super::*;

pub const TERMS_DOMAIN: &str = "mayhem/proxy/spend-terms/v1";
pub const BUYER_TERMS_DOMAIN: &str = "mayhem/proxy/buyer-spend-authorization/v1";
pub const PROVIDER_TERMS_DOMAIN: &str = "mayhem/proxy/provider-spend-acceptance/v1";
pub const RECEIPT_DOMAIN: &str = "mayhem/proxy/usage-receipt/v1";
pub const BUYER_RECEIPT_DOMAIN: &str = "mayhem/proxy/buyer-usage-ack/v1";
pub const PROVIDER_RECEIPT_DOMAIN: &str = "mayhem/proxy/provider-usage-receipt/v1";
pub const SETTLEMENT_POLICY_DOMAIN: &str = "mayhem/proxy/settlement-policy/v1";

/// No default partial/refusal/cancellation charging rule. A policy must be
/// explicitly configured, approved and pinned by both parties before acceptance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxySettlementPolicy {
    #[serde(deserialize_with = "safe_u32")]
    pub schema_version: u32,
    pub lane: ProxyLane,
    pub payable_outcomes: Vec<ProxyReceiptOutcome>,
    pub allow_checkpoints: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_hold_expiry"
    )]
    pub hold_expiry: Option<ProxyHoldExpiry>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyReceiptOutcome {
    Cancelled,
    Complete,
    Partial,
    Refused,
    Running,
}

impl ProxySettlementPolicy {
    pub fn validate(&self) -> Result<(), String> {
        ensure(
            self.schema_version == 1,
            "unsupported proxy settlement policy",
        )?;
        let v = &self.payable_outcomes;
        ensure(
            !v.is_empty()
                && v.len() <= 4
                && v.windows(2).all(|w| w[0] < w[1])
                && v.contains(&ProxyReceiptOutcome::Complete)
                && !v.contains(&ProxyReceiptOutcome::Running),
            "invalid payable proxy outcomes",
        )?;
        canonical_body(self).map(|_| ())
    }
    pub fn digest(&self) -> Result<String, String> {
        self.validate()?;
        digest(SETTLEMENT_POLICY_DOMAIN, self)
    }
}

/// Exactly one attempt's frozen offer and authorization. Monetary amounts use
/// existing AU denomination on the selected rail. `max_spend_au` is the computed
/// gross offer cost of `max_usage`, not an unrelated caller-supplied dollar hold.
/// Existing fee/FX/backing rules are committed by payment_terms_hash/rules_ver;
/// this record creates no extra platform fee and does not convert payment rails.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxySpendTerms {
    #[serde(deserialize_with = "safe_u32")]
    pub schema_version: u32,
    pub lane: ProxyLane,
    pub network_id: String,
    pub msb_bootstrap: String,
    pub subnet_bootstrap: String,
    #[serde(deserialize_with = "safe_u32")]
    pub contract_version: u32,
    pub buyer_pubkey: String,
    pub billing_id: String,
    #[serde(deserialize_with = "safe_u64")]
    pub billing_attempt: u64,
    pub session_id: String,
    pub reservation_id: String,
    #[serde(deserialize_with = "safe_u64")]
    pub billing_epoch: u64,
    #[serde(deserialize_with = "safe_u64")]
    pub acceptance_expires_after_epoch: u64,
    #[serde(deserialize_with = "safe_u64")]
    pub reservation_expires_after_epoch: u64,
    #[serde(deserialize_with = "safe_u64")]
    pub reservation_receipt_grace_epochs: u64,
    pub payout_revision: String,
    pub request_hash: String,
    pub endpoint_contract: String,
    pub recipe_hash: String,
    pub connection_digest: String,
    #[serde(deserialize_with = "safe_u64")]
    pub connection_revision: u64,
    pub capacity_lease: String,
    pub offer: ProxyOffer,
    pub rail: ProxyRail,
    #[serde(deserialize_with = "safe_u32")]
    pub served_context: u32,
    pub settlement_policy_hash: String,
    pub payment_terms_hash: String,
    #[serde(deserialize_with = "safe_u32")]
    pub rules_ver: u32,
    #[serde(deserialize_with = "safe_usage")]
    pub max_usage: BTreeMap<String, u64>,
    #[serde(with = "decimal_u128")]
    pub max_spend_au: MoneyAu,
    #[serde(with = "decimal_u128")]
    pub prior_spend_au: MoneyAu,
    /// Other unresolved exposure for this logical billing authorization, across
    /// providers/rails. It consumes budget but is never claimed as delivered work.
    #[serde(with = "decimal_u128")]
    pub prior_reserved_au: MoneyAu,
    #[serde(with = "decimal_u128")]
    pub max_total_spend_au: MoneyAu,
}

impl ProxySpendTerms {
    pub fn validate(&self) -> Result<(), String> {
        ensure(
            self.schema_version == 1 && self.contract_version > 0 && self.rules_ver > 0,
            "unsupported proxy spend version",
        )?;
        ensure(
            !self.network_id.is_empty()
                && self.network_id.len() <= 128
                && self.network_id.bytes().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_'
                }),
            "invalid proxy spend network",
        )?;
        for h in [
            &self.msb_bootstrap,
            &self.subnet_bootstrap,
            &self.buyer_pubkey,
            &self.billing_id,
            &self.session_id,
            &self.reservation_id,
            &self.payout_revision,
            &self.request_hash,
            &self.endpoint_contract,
            &self.recipe_hash,
            &self.connection_digest,
            &self.capacity_lease,
            &self.settlement_policy_hash,
            &self.payment_terms_hash,
        ] {
            hex_digest(h)?;
        }
        for n in [
            self.billing_attempt,
            self.billing_epoch,
            self.connection_revision,
        ] {
            revision(n)?;
        }
        ensure(
            self.acceptance_expires_after_epoch >= self.billing_epoch
                && self.acceptance_expires_after_epoch < self.reservation_expires_after_epoch
                && self.reservation_expires_after_epoch <= PROXY_MAX_SAFE_INTEGER
                && self.reservation_receipt_grace_epochs
                    <= PROXY_MAX_SAFE_INTEGER - self.reservation_expires_after_epoch,
            "invalid proxy reservation lifetime",
        )?;
        self.offer.validate()?;
        ensure(
            self.offer.accepted_rails.contains(&self.rail),
            "proxy offer rail mismatch",
        )?;
        validate_usage(&self.offer, &self.max_usage)?;
        ensure(
            self.max_usage.values().any(|v| *v > 0),
            "empty proxy authorized usage",
        )?;
        let maximum = self.offer.cost(&self.max_usage)?;
        ensure(
            maximum > 0 && maximum == self.max_spend_au,
            "proxy reserve differs from priced usage bound",
        )?;
        let exposure = self
            .prior_spend_au
            .checked_add(self.prior_reserved_au)
            .and_then(|v| v.checked_add(maximum))
            .ok_or("proxy spend overflow")?;
        ensure(
            exposure <= self.max_total_spend_au,
            "proxy authorization budget exceeded",
        )?;
        canonical_body(self).map(|_| ())
    }
    pub fn digest(&self) -> Result<String, String> {
        self.validate()?;
        digest(TERMS_DOMAIN, self)
    }
    pub fn buyer_signing_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        signing_bytes(BUYER_TERMS_DOMAIN, self)
    }
    pub fn provider_signing_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        signing_bytes(PROVIDER_TERMS_DOMAIN, self)
    }
    /// Only for NEW acceptance, with records from the canonical snapshot. Old
    /// receipts must use retained terms, never today's offer or quote validity.
    pub fn validate_new_acceptance(
        &self,
        market: &ProxyMarketDescriptor,
        member: &ProxyMembership,
        current_offer: &ProxyOffer,
        policy: &ProxySettlementPolicy,
        epoch: u64,
    ) -> Result<(), String> {
        self.validate()?;
        self.offer.validate_for_membership(market, member)?;
        ensure(self.offer == *current_offer, "proxy offer was superseded")?;
        ensure(
            epoch == self.billing_epoch && epoch <= self.acceptance_expires_after_epoch,
            "proxy quote is not valid for active billing epoch",
        )?;
        ensure(
            policy.digest()? == self.settlement_policy_hash,
            "proxy settlement policy mismatch",
        )?;
        ensure(
            member.recipe_hash == self.recipe_hash
                && member.connection_revision == self.connection_revision
                && member.served_context == self.served_context
                && member.endpoints.iter().any(|e| {
                    e.endpoint == self.offer.endpoint && e.contract_hash == self.endpoint_contract
                }),
            "proxy spend membership mismatch",
        )
    }
}

/// Cumulative quantities for this attempt only, not vendor usage counters.
/// Money includes one application of fixed/minimum charges for this attempt.
/// Cross-attempt authorization uses prior_spend_au; other prices cannot reset it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyReceiptBody {
    #[serde(deserialize_with = "safe_u32")]
    pub schema_version: u32,
    pub lane: ProxyLane,
    pub accepted_terms: String,
    #[serde(deserialize_with = "safe_u64")]
    pub seq: u64,
    #[serde(rename = "final")]
    pub final_receipt: bool,
    pub outcome: ProxyReceiptOutcome,
    pub result_hash: String,
    pub observation_hash: String,
    #[serde(deserialize_with = "safe_usage")]
    pub usage: BTreeMap<String, u64>,
    #[serde(with = "decimal_u128")]
    pub au_owed_cum: MoneyAu,
    #[serde(with = "decimal_u128")]
    pub billing_au_owed_cum: MoneyAu,
    #[serde(deserialize_with = "safe_u64")]
    pub at_ms: u64,
}

fn safe_usage<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, u64>, D::Error> {
    #[derive(Deserialize)]
    struct Count(#[serde(deserialize_with = "safe_u64")] u64);
    Ok(BTreeMap::<String, Count>::deserialize(d)?
        .into_iter()
        .map(|(k, v)| (k, v.0))
        .collect())
}

fn validate_usage(offer: &ProxyOffer, usage: &BTreeMap<String, u64>) -> Result<(), String> {
    ensure(
        usage.keys().eq(offer.rates.iter().map(|r| &r.unit))
            && usage.values().all(|v| *v <= PROXY_MAX_SAFE_INTEGER),
        "proxy usage must include every priced unit exactly once",
    )
}

impl ProxyReceiptBody {
    pub fn validate(&self) -> Result<(), String> {
        ensure(self.schema_version == 1, "unsupported proxy receipt schema")?;
        hex_digest(&self.accepted_terms)?;
        hex_digest(&self.result_hash)?;
        hex_digest(&self.observation_hash)?;
        revision(self.seq)?;
        ensure(
            self.at_ms <= PROXY_MAX_SAFE_INTEGER,
            "invalid proxy receipt time",
        )?;
        ensure(
            self.final_receipt != (self.outcome == ProxyReceiptOutcome::Running),
            "invalid proxy receipt finality",
        )?;
        units(&self.usage.keys().cloned().collect::<Vec<_>>())?;
        ensure(
            self.usage.values().all(|v| *v <= PROXY_MAX_SAFE_INTEGER),
            "unsafe proxy usage",
        )?;
        canonical_body(self).map(|_| ())
    }
    pub fn digest(&self) -> Result<String, String> {
        self.validate()?;
        digest(RECEIPT_DOMAIN, self)
    }
    pub fn buyer_signing_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        signing_bytes(BUYER_RECEIPT_DOMAIN, self)
    }
    pub fn provider_signing_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        signing_bytes(PROVIDER_RECEIPT_DOMAIN, self)
    }
    pub fn validate_for(
        &self,
        terms: &ProxySpendTerms,
        policy: &ProxySettlementPolicy,
        previous: Option<&Self>,
    ) -> Result<(), String> {
        self.validate()?;
        ensure(
            self.accepted_terms == terms.digest()?,
            "proxy receipt terms mismatch",
        )?;
        ensure(
            policy.digest()? == terms.settlement_policy_hash,
            "proxy receipt policy mismatch",
        )?;
        ensure(
            if self.final_receipt {
                policy.payable_outcomes.contains(&self.outcome)
            } else {
                policy.allow_checkpoints
            },
            "proxy outcome is not payable under accepted policy",
        )?;
        validate_usage(&terms.offer, &self.usage)?;
        ensure(
            self.usage.iter().all(|(k, v)| *v <= terms.max_usage[k]),
            "proxy usage exceeds authorization",
        )?;
        ensure(
            self.au_owed_cum == terms.offer.cost(&self.usage)?
                && self.au_owed_cum <= terms.max_spend_au,
            "proxy receipt price mismatch",
        )?;
        let cumulative = terms
            .prior_spend_au
            .checked_add(self.au_owed_cum)
            .ok_or("proxy receipt amount overflow")?;
        ensure(
            cumulative == self.billing_au_owed_cum && cumulative <= terms.max_total_spend_au,
            "proxy cumulative authorization exceeded",
        )?;
        if let Some(prev) = previous {
            prev.validate_for(terms, policy, None)?;
            if self == prev {
                return Ok(());
            } // exact replay is harmless, not a new debit
            ensure(
                !prev.final_receipt
                    && self.seq > prev.seq
                    && self.at_ms >= prev.at_ms
                    && self.au_owed_cum >= prev.au_owed_cum
                    && self.usage.iter().all(|(k, v)| *v >= prev.usage[k]),
                "proxy receipt head cannot advance",
            )?;
        }
        Ok(())
    }
}

fn signature(value: &str) -> Result<(), String> {
    ensure(
        value.len() == 128
            && value
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "invalid proxy signature encoding",
    )
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxySpendAuthorization {
    pub terms: ProxySpendTerms,
    pub buyer_sig: String,
    pub provider_sig: String,
}
impl ProxySpendAuthorization {
    pub fn verify(&self, verify: impl Fn(&str, &[u8], &str) -> bool) -> Result<(), String> {
        signature(&self.buyer_sig)?;
        signature(&self.provider_sig)?;
        ensure(
            verify(
                &self.buyer_sig,
                &self.terms.buyer_signing_bytes()?,
                &self.terms.buyer_pubkey,
            ) && verify(
                &self.provider_sig,
                &self.terms.provider_signing_bytes()?,
                &self.terms.offer.provider_pubkey,
            ),
            "proxy spend signature rejected",
        )?;
        canonical_body(self).map(|_| ())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyUsageReceipt {
    pub body: ProxyReceiptBody,
    pub buyer_sig: String,
    pub provider_sig: String,
}
impl ProxyUsageReceipt {
    pub fn verify(
        &self,
        terms: &ProxySpendTerms,
        policy: &ProxySettlementPolicy,
        previous: Option<&ProxyReceiptBody>,
        verify: impl Fn(&str, &[u8], &str) -> bool,
    ) -> Result<(), String> {
        self.body.validate_for(terms, policy, previous)?;
        signature(&self.buyer_sig)?;
        signature(&self.provider_sig)?;
        ensure(
            verify(
                &self.buyer_sig,
                &self.body.buyer_signing_bytes()?,
                &terms.buyer_pubkey,
            ) && verify(
                &self.provider_sig,
                &self.body.provider_signing_bytes()?,
                &terms.offer.provider_pubkey,
            ),
            "proxy receipt signature rejected",
        )?;
        canonical_body(self).map(|_| ())
    }
}

pub const CLOSURE_DOMAIN: &str = "mayhem/proxy/reservation-closure/v1";
pub const BUYER_CLOSURE_DOMAIN: &str = "mayhem/proxy/buyer-reservation-closure/v1";
pub const PROVIDER_CLOSURE_DOMAIN: &str = "mayhem/proxy/provider-reservation-closure/v1";
pub const EXPIRY_DOMAIN: &str = "mayhem/proxy/reservation-expiry/v1";
pub const BUYER_EXPIRY_DOMAIN: &str = "mayhem/proxy/buyer-reservation-expiry/v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyHoldExpiry {
    ReleaseUnfinalizedAndBlockRetry,
}

// Absence preserves old policy digests; explicit null is not a policy.
fn present_hold_expiry<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<ProxyHoldExpiry>, D::Error> {
    ProxyHoldExpiry::deserialize(d).map(Some)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyClosureOutcome {
    NotExecuted,
    Cancelled,
    Failed,
    CompletedUnbilled,
}

/// A mutual charge waiver with known execution outcome. This wire record alone
/// cannot establish that an opaque upstream has actually stopped; the controller
/// must retain and verify its evidence before signing or releasing capacity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyClosureBody {
    #[serde(deserialize_with = "safe_u32")]
    pub schema_version: u32,
    pub lane: ProxyLane,
    pub accepted_terms: String,
    pub outcome: ProxyClosureOutcome,
    pub evidence_hash: String,
    #[serde(deserialize_with = "safe_u64")]
    pub at_ms: u64,
}
impl ProxyClosureBody {
    pub fn validate(&self) -> Result<(), String> {
        ensure(
            self.schema_version == 1 && self.at_ms <= PROXY_MAX_SAFE_INTEGER,
            "invalid proxy closure schema/time",
        )?;
        hex_digest(&self.accepted_terms)?;
        hex_digest(&self.evidence_hash)?;
        canonical_body(self).map(|_| ())
    }
    pub fn digest(&self) -> Result<String, String> {
        self.validate()?;
        digest(CLOSURE_DOMAIN, self)
    }
    pub fn buyer_signing_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        signing_bytes(BUYER_CLOSURE_DOMAIN, self)
    }
    pub fn provider_signing_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        signing_bytes(PROVIDER_CLOSURE_DOMAIN, self)
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyReservationClosure {
    pub body: ProxyClosureBody,
    pub buyer_sig: String,
    pub provider_sig: String,
}
impl ProxyReservationClosure {
    pub fn verify(
        &self,
        terms: &ProxySpendTerms,
        verify: impl Fn(&str, &[u8], &str) -> bool,
    ) -> Result<(), String> {
        self.body.validate()?;
        ensure(
            self.body.accepted_terms == terms.digest()?,
            "proxy closure terms mismatch",
        )?;
        signature(&self.buyer_sig)?;
        signature(&self.provider_sig)?;
        ensure(
            verify(
                &self.buyer_sig,
                &self.body.buyer_signing_bytes()?,
                &terms.buyer_pubkey,
            ) && verify(
                &self.provider_sig,
                &self.body.provider_signing_bytes()?,
                &terms.offer.provider_pubkey,
            ),
            "proxy closure signature rejected",
        )?;
        canonical_body(self).map(|_| ())
    }
}

/// Releasing an expired financial hold does not resolve remote execution and
/// must not grant permission to redispatch or release an uncertain capacity slot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyExpiryBody {
    #[serde(deserialize_with = "safe_u32")]
    pub schema_version: u32,
    pub lane: ProxyLane,
    pub accepted_terms: String,
    #[serde(deserialize_with = "safe_u64")]
    pub observed_epoch: u64,
    #[serde(deserialize_with = "safe_u64")]
    pub at_ms: u64,
}
impl ProxyExpiryBody {
    pub fn validate(&self) -> Result<(), String> {
        ensure(
            self.schema_version == 1 && self.at_ms <= PROXY_MAX_SAFE_INTEGER,
            "invalid proxy expiry schema/time",
        )?;
        hex_digest(&self.accepted_terms)?;
        revision(self.observed_epoch)?;
        canonical_body(self).map(|_| ())
    }
    pub fn digest(&self) -> Result<String, String> {
        self.validate()?;
        digest(EXPIRY_DOMAIN, self)
    }
    pub fn buyer_signing_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        signing_bytes(BUYER_EXPIRY_DOMAIN, self)
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyReservationExpiry {
    pub body: ProxyExpiryBody,
    pub buyer_sig: String,
}
impl ProxyReservationExpiry {
    pub fn verify(
        &self,
        terms: &ProxySpendTerms,
        policy: &ProxySettlementPolicy,
        epoch: u64,
        verify: impl Fn(&str, &[u8], &str) -> bool,
    ) -> Result<(), String> {
        self.body.validate()?;
        ensure(
            self.body.accepted_terms == terms.digest()?,
            "proxy expiry terms mismatch",
        )?;
        ensure(
            policy.digest()? == terms.settlement_policy_hash
                && policy.hold_expiry == Some(ProxyHoldExpiry::ReleaseUnfinalizedAndBlockRetry),
            "proxy expiry is not enabled by accepted policy",
        )?;
        ensure(
            epoch <= PROXY_MAX_SAFE_INTEGER
                && self.body.observed_epoch <= epoch
                && self.body.observed_epoch
                    > terms.reservation_expires_after_epoch
                        + terms.reservation_receipt_grace_epochs,
            "proxy reservation receipt grace has not expired",
        )?;
        signature(&self.buyer_sig)?;
        ensure(
            verify(
                &self.buyer_sig,
                &self.body.buyer_signing_bytes()?,
                &terms.buyer_pubkey,
            ),
            "proxy expiry signature rejected",
        )?;
        canonical_body(self).map(|_| ())
    }
}
