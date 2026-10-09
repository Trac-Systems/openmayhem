//! Build terms from the buyer's owned request, explicit budget and fresh quote.
//! This is a pre-signing object, not consent inferred from an account balance.
//! Persistence, authenticated negotiation and dual signatures remain mandatory.
use super::*;
use crate::{
    attempts::Digest,
    buyer::{Evidence, PublicAcceptanceSnapshot, Snapshot},
    endpoint::{PublicAdapter, PublicAdapterSnapshot},
};
use mayhem_proto::proxy::ProxyLane;
use std::collections::BTreeMap;

/// Explicit epoch lifetimes resolved from the buyer's policy. No default can
/// silently enlarge an authorization or turn an API timeout into a hold expiry.
#[derive(Clone, Copy)]
pub struct Lifetimes {
    pub acceptance_epochs: u64,
    pub reservation_epochs: u64,
    pub receipt_grace_epochs: u64,
}
impl Lifetimes {
    fn validate(&self) -> Result<()> {
        require(
            self.acceptance_epochs < self.reservation_epochs
                && self.reservation_epochs <= PROXY_MAX_SAFE_INTEGER
                && self.receipt_grace_epochs <= PROXY_MAX_SAFE_INTEGER,
            "invalid explicit purchase lifetimes",
        )
    }
}

/// Identifiers proposed by the trusted provider controller. These bind terms;
/// possessing them does not prove that a model slot was reserved. Paid dispatch
/// independently requires the durable capacity authority and canonical hold.
pub struct SessionBinding {
    pub session_id: Digest,
    pub reservation_id: Digest,
    pub connection_digest: Digest,
    pub capacity_lease: Digest,
}

/// Constructed by the buyer's trusted parent from the actual outgoing request.
/// Not deserializable/debuggable: callers cannot substitute provider-reported
/// usage or accidentally dump prompts into ordinary diagnostics.
pub struct PurchaseRequest {
    request: Vec<u8>,
    adapter: PublicAdapterSnapshot,
    prices: PriceLimits,
    usage: BTreeMap<String, u64>,
    lifetimes: Lifetimes,
}
/// Exact request-derived quantities and their offer maximum, never an admission.
#[derive(Clone, Serialize)]
pub struct Maximum {
    pub request_hash: Digest,
    pub metering_policy_hash: Digest,
    pub max_usage: BTreeMap<String, u64>,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub max_spend_au: MoneyAu,
}
impl PurchaseRequest {
    pub fn new(
        adapter: PublicAdapterSnapshot,
        request: Vec<u8>,
        prices: PriceLimits,
        output_units: Option<u64>,
        lifetimes: Lifetimes,
    ) -> Result<Self> {
        lifetimes.validate()?;
        let restored = PublicAdapter::restore(adapter.clone())
            .map_err(|_| invalid("invalid purchase adapter"))?;
        let parsed = prepare(&restored, &request)?;
        let usage = parsed
            .maximum_usage(output_units)
            .map_err(|_| invalid("invalid explicit purchase usage allowance"))?;
        Ok(Self {
            request,
            adapter,
            prices,
            usage,
            lifetimes,
        })
    }
    /// Shared by estimates and actual purchase preparation. No financial state,
    /// capacity authority, signer, provider request or persistence is available.
    pub fn maximum(&self, offer: &ProxyOffer) -> Result<Maximum> {
        let adapter = PublicAdapter::restore(self.adapter.clone())
            .map_err(|_| invalid("invalid purchase adapter"))?;
        let request = prepare(&adapter, &self.request)?;
        require(
            request.endpoint() == offer.endpoint
                && request.metering_policy_hash().as_str() == offer.metering_policy_hash,
            "purchase request differs from quoted endpoint or metering",
        )?;
        self.prices.permits(offer)?;
        let maximum = offer.cost(&self.usage).map_err(invalid)?;
        require(
            maximum > 0 && maximum <= self.prices.max_total_spend_au,
            "purchase maximum exceeds explicit total limit",
        )?;
        Ok(Maximum {
            request_hash: request.request_hash().clone(),
            metering_policy_hash: request.metering_policy_hash(),
            max_usage: self.usage.clone(),
            max_spend_au: maximum,
        })
    }
}
fn prepare(adapter: &PublicAdapter, request: &[u8]) -> Result<crate::endpoint::PublicRequest> {
    require(
        request.len() <= adapter.limits().request_bytes,
        "purchase request exceeds bound",
    )?;
    let value: serde_json::Value = serde_json::from_slice(request)?;
    if value.get("stream") == Some(&Value::Bool(true)) {
        adapter.prepare_stream(request)
    } else {
        adapter.prepare_json(request)
    }
    .map_err(|_| invalid("purchase request is incompatible with endpoint"))
}

/// Exact unsent purchase ready for durable negotiation storage. No signature,
/// inference POST, model-capacity claim or funds mutation is performed here.
pub struct PreparedPurchase {
    terms: ProxySpendTerms,
    policy: ProxySettlementPolicy,
    prices: PriceLimits,
    snapshot: Snapshot,
    request: Vec<u8>,
}
/// Private trusted-parent persistence, not an input accepted from connectors.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetainedPurchase {
    pub terms: ProxySpendTerms,
    pub policy: ProxySettlementPolicy,
    pub prices: PriceLimits,
    pub snapshot: Snapshot,
    pub request: String,
}
impl RetainedPurchase {
    pub(crate) fn validate(&self) -> Result<()> {
        self.terms.validate().map_err(invalid)?;
        self.prices.permits(&self.terms.offer)?;
        require(
            self.terms.max_total_spend_au == self.prices.max_total_spend_au
                && self.policy.digest().map_err(invalid)? == self.terms.settlement_policy_hash,
            "retained purchase policy differs",
        )?;
        let binding = super::super::terms_binding(&self.terms)?;
        self.snapshot
            .validate_for(&binding)
            .map_err(|_| invalid("retained purchase snapshot differs"))?;
        let prepared = self
            .snapshot
            .verify_request(&binding, self.request.as_bytes())?;
        let output = self.terms.max_usage.get("output_token").copied();
        require(
            prepared.matches_binding(&binding)
                && prepared
                    .maximum_usage(output)
                    .map_err(|_| invalid("invalid retained usage"))?
                    == self.terms.max_usage,
            "retained purchase request differs",
        )
    }
    pub(crate) fn commitment(&self) -> Result<Digest> {
        self.validate()?;
        let bytes = mayhem_proto::stable_json_bytes(&serde_json::to_value(self)?)?;
        Ok(Digest::hash("mayhem/proxy/purchase-intent/v1", &[&bytes]))
    }
}
impl PreparedPurchase {
    pub(crate) fn retained(&self) -> Result<RetainedPurchase> {
        let retained = RetainedPurchase {
            terms: self.terms.clone(),
            policy: self.policy.clone(),
            prices: self.prices.clone(),
            snapshot: self.snapshot.clone(),
            request: String::from_utf8(self.request.clone())
                .map_err(|_| invalid("invalid purchase UTF-8"))?,
        };
        retained.validate()?;
        Ok(retained)
    }
    pub fn terms(&self) -> &ProxySpendTerms {
        &self.terms
    }
    pub fn policy(&self) -> &ProxySettlementPolicy {
        &self.policy
    }
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    pub fn request(&self) -> &[u8] {
        &self.request
    }
}
impl Observation {
    /// Revalidate the exact original proposal after asynchronous negotiation or
    /// storage work. This never rebuilds terms at a new price or extends a hold.
    pub fn recheck_purchase(&self, purchase: &PreparedPurchase) -> Result<()> {
        self.check_terms(&purchase.terms, &purchase.prices)
    }
    pub fn prepare_purchase(
        &self,
        intent: &PurchaseRequest,
        session: &SessionBinding,
    ) -> Result<PreparedPurchase> {
        self.fresh()?;
        let w = &self.wire;
        let maximum = intent.maximum(&w.offer)?;
        let adapter = PublicAdapter::restore(intent.adapter.clone())
            .map_err(|_| invalid("invalid purchase adapter"))?;
        let request = prepare(&adapter, &intent.request)?;
        require(
            request.endpoint() == w.offer.endpoint
                && request.metering_policy_hash().as_str() == w.offer.metering_policy_hash,
            "purchase request differs from quoted endpoint or metering",
        )?;
        let add_epoch = |delta: u64| {
            w.billing_epoch
                .checked_add(delta)
                .filter(|v| *v <= PROXY_MAX_SAFE_INTEGER)
                .ok_or_else(|| invalid("purchase lifetime overflow"))
        };
        let (billing_attempt, prior_spend_au) = match &w.billing {
            Some(b) => (
                b.latest_attempt
                    .checked_add(1)
                    .ok_or_else(|| invalid("purchase attempt overflow"))?,
                b.spent_au,
            ),
            None => (1, 0),
        };
        let terms = ProxySpendTerms {
            schema_version: 1,
            lane: ProxyLane::Proxy,
            network_id: w.context.network_id.clone(),
            msb_bootstrap: w.context.msb_bootstrap.clone(),
            subnet_bootstrap: w.context.subnet_bootstrap.clone(),
            contract_version: w.context.contract_version,
            buyer_pubkey: w.requester.clone(),
            billing_id: w.billing_id.clone(),
            billing_attempt,
            session_id: session.session_id.as_str().into(),
            reservation_id: session.reservation_id.as_str().into(),
            billing_epoch: w.billing_epoch,
            acceptance_expires_after_epoch: add_epoch(intent.lifetimes.acceptance_epochs)?,
            reservation_expires_after_epoch: add_epoch(intent.lifetimes.reservation_epochs)?,
            reservation_receipt_grace_epochs: intent.lifetimes.receipt_grace_epochs,
            payout_revision: w.payout_revision.clone(),
            request_hash: request.request_hash().as_str().into(),
            endpoint_contract: adapter.contract_hash().as_str().into(),
            recipe_hash: adapter.recipe_hash().as_str().into(),
            connection_digest: session.connection_digest.as_str().into(),
            connection_revision: w.membership.connection_revision,
            capacity_lease: session.capacity_lease.as_str().into(),
            offer: w.offer.clone(),
            rail: w.rail,
            served_context: w.membership.served_context,
            settlement_policy_hash: w.settlement_policy_hash.clone(),
            payment_terms_hash: w.payment_terms_hash.clone(),
            rules_ver: w.rules_ver,
            max_usage: maximum.max_usage,
            max_spend_au: maximum.max_spend_au,
            prior_spend_au,
            prior_reserved_au: 0,
            max_total_spend_au: intent.prices.max_total_spend_au,
        };
        self.check_terms(&terms, &intent.prices)?;
        let snapshot = Snapshot::Public(PublicAcceptanceSnapshot {
            version: 1,
            adapter: intent.adapter.clone(),
            offer: w.offer.clone(),
        });
        let binding = super::super::terms_binding(&terms)?;
        snapshot
            .validate_for(&binding)
            .map_err(|_| invalid("purchase execution snapshot differs"))?;
        require(
            request.matches_binding(&binding),
            "purchase request differs from proposal",
        )?;
        Ok(PreparedPurchase {
            terms,
            policy: w.settlement_policy.clone(),
            prices: intent.prices.clone(),
            snapshot,
            request: intent.request.clone(),
        })
    }
}
