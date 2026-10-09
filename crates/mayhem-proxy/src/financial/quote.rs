//! Authenticated canonical inputs to a new purchase. Not a reservation, model
//! slot, signing instruction or dispatch permit. Never renew freshness from cache.
mod purchase;
use super::*;
use mayhem_proto::{
    proxy::{
        finance::ProxySpendTerms, ProxyMarketDescriptor, ProxyMembership, ProxyOffer, ProxyRail,
        ProxyRate,
    },
    MoneyAu,
};
pub use purchase::{Lifetimes, PreparedPurchase, PurchaseRequest, SessionBinding};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub offer: ProxyOffer,
    pub rail: ProxyRail,
    pub settlement_policy_hash: String,
    pub billing_id: String,
}
impl Query {
    fn validate(&self) -> Result<()> {
        self.offer.validate().map_err(invalid)?;
        require(
            hex(&self.settlement_policy_hash)
                && hex(&self.billing_id)
                && self.offer.accepted_rails.contains(&self.rail)
                && serde_json::to_vec(self)?.len() <= 32 * 1024 - 256,
            "invalid quote query",
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Funding {
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub balance_au: MoneyAu,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub reserved_au: MoneyAu,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub available_au: MoneyAu,
    pub chain_id: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Billing {
    pub latest_attempt: u64,
    pub latest_accepted_terms: String,
    pub active_reservation_id: Option<String>,
    pub retry_blocked: bool,
    pub request_hash: String,
    pub endpoint_contract: String,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub max_total_spend_au: MoneyAu,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub spent_au: MoneyAu,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub reserved_au: MoneyAu,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    ok: bool,
    schema_version: u32,
    lane: String,
    requester: String,
    request_nonce: String,
    offer: ProxyOffer,
    rail: ProxyRail,
    settlement_policy_hash: String,
    billing_id: String,
    context: Context,
    proof: Proof,
    billing_epoch: u64,
    market: ProxyMarketDescriptor,
    membership: ProxyMembership,
    settlement_policy: ProxySettlementPolicy,
    payout_revision: String,
    payment_terms_hash: String,
    rules_ver: u32,
    funding: Funding,
    billing: Option<Billing>,
}
/// Non-deserializable fresh observation from the configured trusted Core RPC.
/// Its proof is verified by the authenticated indexer service transport; this
/// client does not pretend to independently execute a Merkle proof.
pub struct Observation {
    wire: Wire,
    started: Instant,
}

/// Resolve these from the buyer's explicit account/project/request policy.
/// Prices cover every metering unit and fixed charge, never balance-as-consent.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceLimits {
    pub rates: Vec<ProxyRate>,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub per_request_au: MoneyAu,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub min_session_au: MoneyAu,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub max_total_spend_au: MoneyAu,
}
impl PriceLimits {
    pub fn permits(&self, offer: &ProxyOffer) -> Result<()> {
        offer.validate().map_err(invalid)?;
        require(
            self.max_total_spend_au > 0
                && self.rates.len() == offer.rates.len()
                && self.per_request_au >= offer.per_request_au
                && self.min_session_au >= offer.min_session_au,
            "offer exceeds buyer fixed-charge limits",
        )?;
        for (actual, cap) in offer.rates.iter().zip(&self.rates) {
            require(
                actual.unit == cap.unit
                    && cap.granularity > 0
                    && cap.granularity <= PROXY_MAX_SAFE_INTEGER,
                "buyer rate limits differ from priced units",
            )?;
            // Compare rational prices exactly without u128 multiplication overflow.
            // Remainders are <2^53, so their cross-products fit in u128.
            let (a, b) = (u128::from(actual.granularity), u128::from(cap.granularity));
            let (whole_a, whole_b) = (actual.per_unit_au / a, cap.per_unit_au / b);
            require(
                whole_a < whole_b
                    || (whole_a == whole_b
                        && (actual.per_unit_au % a) * b <= (cap.per_unit_au % b) * a),
                "offer exceeds buyer unit-price limit",
            )?;
        }
        Ok(())
    }
}
impl Observation {
    fn fresh(&self) -> Result<()> {
        require(
            self.started.elapsed() <= FRESHNESS,
            "quote observation expired; requote",
        )
    }
    pub fn funding(&self) -> Result<&Funding> {
        self.fresh()?;
        Ok(&self.wire.funding)
    }
    pub fn billing(&self) -> Result<Option<&Billing>> {
        self.fresh()?;
        Ok(self.wire.billing.as_ref())
    }
    pub fn epoch(&self) -> Result<u64> {
        self.fresh()?;
        Ok(self.wire.billing_epoch)
    }
    pub fn proof(&self) -> &Proof {
        &self.wire.proof
    }
    pub fn policy(&self) -> Result<&ProxySettlementPolicy> {
        self.fresh()?;
        Ok(&self.wire.settlement_policy)
    }
    pub fn payment_binding(&self) -> Result<(&str, &str, u32)> {
        self.fresh()?;
        Ok((
            &self.wire.payout_revision,
            &self.wire.payment_terms_hash,
            self.wire.rules_ver,
        ))
    }
    /// Validate an exact proposed purchase against a fresh canonical observation
    /// and independently resolved buyer prices. Does not sign or reserve anything.
    pub fn check_terms(&self, t: &ProxySpendTerms, limits: &PriceLimits) -> Result<()> {
        self.fresh()?;
        let w = &self.wire;
        limits.permits(&t.offer)?;
        t.validate_new_acceptance(
            &w.market,
            &w.membership,
            &w.offer,
            &w.settlement_policy,
            w.billing_epoch,
        )
        .map_err(invalid)?;
        require(
            t.network_id == w.context.network_id
                && t.msb_bootstrap == w.context.msb_bootstrap
                && t.subnet_bootstrap == w.context.subnet_bootstrap
                && t.contract_version == w.context.contract_version
                && t.buyer_pubkey == w.requester
                && t.rail == w.rail
                && t.billing_id == w.billing_id
                && t.payout_revision == w.payout_revision
                && t.payment_terms_hash == w.payment_terms_hash
                && t.rules_ver == w.rules_ver
                && t.max_total_spend_au == limits.max_total_spend_au
                && t.max_spend_au <= w.funding.available_au,
            "quote differs from buyer, payment binding or funding",
        )?;
        if let Some(b) = &w.billing {
            require(
                !b.retry_blocked
                    && b.active_reservation_id.is_none()
                    && b.reserved_au == 0
                    && b.latest_attempt.checked_add(1) == Some(t.billing_attempt)
                    && b.spent_au == t.prior_spend_au
                    && t.prior_reserved_au == 0
                    && b.max_total_spend_au == t.max_total_spend_au
                    && b.request_hash == t.request_hash
                    && b.endpoint_contract == t.endpoint_contract,
                "recover the existing purchase before a new attempt",
            )?;
        } else {
            require(
                t.billing_attempt == 1 && t.prior_spend_au == 0 && t.prior_reserved_au == 0,
                "new purchase cannot claim prior work or exposure",
            )?;
        }
        Ok(())
    }
}
impl Client {
    pub async fn quote(&self, query: &Query) -> Result<Observation> {
        query.validate()?;
        let _permit = self
            .slots
            .try_acquire()
            .map_err(|_| invalid("quote observation capacity unavailable"))?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| invalid("quote challenge unavailable"))?;
        let nonce = nonce.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let mut body = serde_json::to_value(query)?;
        body["request_nonce"] = json!(nonce);
        let started = Instant::now();
        let mut response = self
            .http
            .post(
                self.endpoint
                    .join("quote-state")
                    .map_err(|_| invalid("invalid quote endpoint"))?,
            )
            .json(&body)
            .send()
            .await
            .map_err(Error::Transport)?;
        require(
            response.status().is_success(),
            "canonical quote is unavailable; recover or requote",
        )?;
        require(
            response
                .content_length()
                .is_none_or(|n| n <= MAX_BYTES as u64),
            "quote response exceeds bound",
        )?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(Error::Transport)? {
            require(
                bytes.len().saturating_add(chunk.len()) <= MAX_BYTES,
                "quote response exceeds bound",
            )?;
            bytes.extend_from_slice(&chunk);
        }
        let w: Wire = serde_json::from_slice(&bytes)?;
        require(
            started.elapsed() <= FRESHNESS
                && w.ok
                && w.schema_version == 1
                && w.lane == "proxy"
                && w.requester == self.requester
                && w.request_nonce == nonce
                && w.context.identity() == self.identity
                && w.offer == query.offer
                && w.rail == query.rail
                && w.billing_id == query.billing_id
                && w.settlement_policy_hash == query.settlement_policy_hash
                && w.context.epoch < w.billing_epoch
                && w.billing_epoch <= PROXY_MAX_SAFE_INTEGER,
            "quote response binding differs",
        )?;
        w.proof.validate()?;
        w.offer
            .validate_for_membership(&w.market, &w.membership)
            .map_err(invalid)?;
        require(
            w.settlement_policy.digest().map_err(invalid)? == query.settlement_policy_hash
                && hex(&w.payout_revision)
                && hex(&w.payment_terms_hash)
                && w.rules_ver > 0
                && w.funding.reserved_au.checked_add(w.funding.available_au)
                    == Some(w.funding.balance_au)
                && if w.rail == ProxyRail::Tap {
                    w.funding
                        .chain_id
                        .is_some_and(|id| id > 0 && id <= PROXY_MAX_SAFE_INTEGER)
                } else {
                    w.funding.chain_id.is_none()
                },
            "quote payment state differs",
        )?;
        if let Some(b) = &w.billing {
            require(
                b.latest_attempt > 0
                    && b.latest_attempt <= PROXY_MAX_SAFE_INTEGER
                    && [
                        &b.latest_accepted_terms,
                        &b.request_hash,
                        &b.endpoint_contract,
                    ]
                    .iter()
                    .all(|v| hex(v))
                    && b.active_reservation_id.as_ref().is_none_or(|v| hex(v))
                    && b.spent_au
                        .checked_add(b.reserved_au)
                        .is_some_and(|n| n <= b.max_total_spend_au),
                "quote billing state is inconsistent",
            )?;
        }
        Ok(Observation { wire: w, started })
    }
}
