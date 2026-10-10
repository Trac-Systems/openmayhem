//! Fresh provider-owned offer and payout observation before countersigning.
//! No buyer balance access, reservation, signature or execution permission.
use super::*;
use mayhem_proto::proxy::{
    finance::ProxySpendTerms, ProxyMarketDescriptor, ProxyMembership, ProxyOffer, ProxyRail,
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub offer: ProxyOffer,
    pub rail: ProxyRail,
    pub settlement_policy_hash: String,
}
impl Query {
    fn validate(&self, provider: &str) -> Result<()> {
        self.offer.validate().map_err(invalid)?;
        require(
            self.offer.provider_pubkey == provider
                && hex(&self.settlement_policy_hash)
                && self.offer.accepted_rails.contains(&self.rail)
                && serde_json::to_vec(self)?.len() <= 32 * 1024 - 256,
            "invalid provider offer query",
        )
    }
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
    #[serde(default)]
    follow_rates: bool,
    #[serde(default)]
    current_offer: Option<ProxyOffer>,
    rail: ProxyRail,
    settlement_policy_hash: String,
    context: Context,
    proof: Proof,
    billing_epoch: u64,
    market: ProxyMarketDescriptor,
    membership: ProxyMembership,
    settlement_policy: ProxySettlementPolicy,
    payout_revision: String,
    payment_terms_hash: String,
    rules_ver: u32,
}

/// Only the bounded authenticated Core client can construct this observation.
/// Proof validation is delegated to its signed indexer transport, not performed
/// independently here. A saved proof must never be used to renew freshness.
pub struct Observation {
    wire: Wire,
    started: Instant,
}
impl Observation {
    /// Canonically observed effective offer, never a local rate suggestion.
    pub fn offer(&self) -> Result<&ProxyOffer> {
        self.fresh()?;
        Ok(&self.wire.offer)
    }
    pub(crate) fn check_descriptor(
        &self,
        context: &crate::descriptor::Context,
        approved_policy: &ProxySettlementPolicy,
    ) -> Result<()> {
        self.fresh()?;
        let w = &self.wire;
        require(
            context.network == w.context.identity()
                && context.offer == w.offer
                && context.rail == w.rail
                && context.settlement_policy_hash.as_str() == w.settlement_policy_hash
                && approved_policy == &w.settlement_policy,
            "descriptor differs from canonical offer or approved policy",
        )
    }
    fn fresh(&self) -> Result<()> {
        require(
            self.started.elapsed() <= FRESHNESS,
            "provider offer observation expired; refresh",
        )
    }
    pub fn proof(&self) -> &Proof {
        &self.wire.proof
    }
    pub fn epoch(&self) -> Result<u64> {
        self.fresh()?;
        Ok(self.wire.billing_epoch)
    }
    pub fn policy(&self) -> Result<&ProxySettlementPolicy> {
        self.fresh()?;
        Ok(&self.wire.settlement_policy)
    }
    pub fn membership(&self) -> Result<&ProxyMembership> {
        self.fresh()?;
        Ok(&self.wire.membership)
    }
    pub(crate) fn presence_binding(
        &self,
    ) -> Result<(Identity, &ProxyOffer, &ProxyMembership, u64)> {
        self.fresh()?;
        let remaining = FRESHNESS.saturating_sub(self.started.elapsed()).as_millis() as u64;
        Ok((
            self.wire.context.identity(),
            &self.wire.offer,
            &self.wire.membership,
            remaining,
        ))
    }
    pub(crate) fn check_proposal(
        &self,
        context: &crate::negotiation::Context,
        approved_policy: &ProxySettlementPolicy,
    ) -> Result<()> {
        self.fresh()?;
        let w = &self.wire;
        require(
            context.network_id == w.context.network_id
                && context.msb_bootstrap.as_str() == w.context.msb_bootstrap
                && context.subnet_bootstrap.as_str() == w.context.subnet_bootstrap
                && context.contract_version == w.context.contract_version
                && context.offer == w.offer
                && context.rail == w.rail
                && context.settlement_policy_hash.as_str() == w.settlement_policy_hash
                && approved_policy == &w.settlement_policy,
            "proposal differs from canonical offer or operator policy",
        )
    }
    /// The approved policy MUST come from this provider's trusted configuration,
    /// not the buyer or the list of globally enabled policies. This checks public
    /// offer/payment terms; the writer separately checks the buyer's funding.
    pub fn check_terms(
        &self,
        terms: &ProxySpendTerms,
        approved_policy: &ProxySettlementPolicy,
    ) -> Result<()> {
        self.fresh()?;
        let w = &self.wire;
        terms
            .validate_new_acceptance(
                &w.market,
                &w.membership,
                &w.offer,
                &w.settlement_policy,
                w.billing_epoch,
            )
            .map_err(invalid)?;
        require(
            approved_policy == &w.settlement_policy
                && terms.network_id == w.context.network_id
                && terms.msb_bootstrap == w.context.msb_bootstrap
                && terms.subnet_bootstrap == w.context.subnet_bootstrap
                && terms.contract_version == w.context.contract_version
                && terms.offer.provider_pubkey == w.requester
                && terms.rail == w.rail
                && terms.payout_revision == w.payout_revision
                && terms.payment_terms_hash == w.payment_terms_hash
                && terms.rules_ver == w.rules_ver,
            "provider offer differs from approved policy or canonical payment terms",
        )
    }
}
impl Client {
    pub async fn offer_state(&self, query: &Query) -> Result<Observation> {
        self.observe_offer(query, false).await
    }
    /// Control-plane availability only: follow signed rate publications within
    /// this exact membership/submarket. Paid negotiation uses `offer_state`.
    pub async fn current_rate_state(&self, query: &Query) -> Result<Observation> {
        self.observe_offer(query, true).await
    }
    async fn observe_offer(&self, query: &Query, follow_rates: bool) -> Result<Observation> {
        query.validate(&self.requester)?;
        let _permit = self
            .slots
            .try_acquire()
            .map_err(|_| invalid("offer observation capacity unavailable"))?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| invalid("offer challenge unavailable"))?;
        let nonce = nonce.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let mut body = serde_json::to_value(query)?;
        body["request_nonce"] = json!(nonce);
        if follow_rates {
            body["follow_rates"] = json!(true);
        }
        let started = Instant::now();
        let mut response = self
            .http
            .post(
                self.endpoint
                    .join("offer-state")
                    .map_err(|_| invalid("invalid offer endpoint"))?,
            )
            .json(&body)
            .send()
            .await
            .map_err(Error::Transport)?;
        require(
            response.status().is_success(),
            "canonical provider offer is unavailable; refresh",
        )?;
        require(
            response
                .content_length()
                .is_none_or(|n| n <= MAX_BYTES as u64),
            "offer response exceeds bound",
        )?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(Error::Transport)? {
            require(
                bytes.len().saturating_add(chunk.len()) <= MAX_BYTES,
                "offer response exceeds bound",
            )?;
            bytes.extend_from_slice(&chunk);
        }
        let mut w: Wire = serde_json::from_slice(&bytes)?;
        require(
            started.elapsed() <= FRESHNESS
                && w.ok
                && w.schema_version == 1
                && w.lane == "proxy"
                && w.requester == self.requester
                && w.request_nonce == nonce
                && w.context.identity() == self.identity
                && w.offer == query.offer
                && w.follow_rates == follow_rates
                && w.rail == query.rail
                && w.settlement_policy_hash == query.settlement_policy_hash
                && w.context.epoch < w.billing_epoch
                && w.billing_epoch <= PROXY_MAX_SAFE_INTEGER,
            "provider offer response binding differs",
        )?;
        if follow_rates {
            let current = w.current_offer.take().ok_or_else(|| invalid("current rate offer missing"))?;
            require(rate_successor(&query.offer, &current), "current offer changed outside its rates")?;
            w.offer = current;
        } else {
            require(w.current_offer.is_none(), "unexpected current rate offer")?;
        }
        w.proof.validate()?;
        w.offer
            .validate_for_membership(&w.market, &w.membership)
            .map_err(invalid)?;
        require(
            w.settlement_policy.digest().map_err(invalid)? == query.settlement_policy_hash
                && hex(&w.payout_revision)
                && hex(&w.payment_terms_hash)
                && w.rules_ver > 0,
            "provider payment state differs",
        )?;
        Ok(Observation { wire: w, started })
    }
}

/// A commercial update preserves every execution/rail binding, including the
/// registered submarket. Same-revision substitutions and rollbacks are refused.
pub(crate) fn rate_successor(original: &ProxyOffer, current: &ProxyOffer) -> bool {
    if current.validate().is_err() || current.revision < original.revision {
        return false;
    }
    let mut comparable = original.clone();
    if current.revision > original.revision {
        comparable.revision = current.revision;
        comparable.rates = current.rates.clone();
        comparable.per_request_au = current.per_request_au;
        comparable.min_session_au = current.min_session_au;
    }
    comparable == *current
}
