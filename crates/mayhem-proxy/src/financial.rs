//! Exact-attempt canonical finance observations from the configured trusted Core
//! peer RPC. Its authenticated service verifies the indexer's signed snapshot.
//! An arbitrary upstream HTTP response is never a financial authority.
pub mod intent;
pub mod negotiation;
pub mod offer;
pub mod provider;
pub mod quote;
pub mod recovery;
use crate::{
    attempts::{Binding, Digest},
    discovery::{hex, Context, Identity, Proof},
    invalid, require, Error, Result,
};
use mayhem_proto::proxy::{
    finance::{
        ProxyReservationClosure, ProxySettlementPolicy, ProxySpendAuthorization, ProxyUsageReceipt,
    },
    PROXY_MAX_SAFE_INTEGER,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use url::Url;

const MAX_BYTES: usize = 131072;
const FRESHNESS: Duration = Duration::from_secs(15);

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Accepted {
    #[serde(rename = "type")]
    kind: String,
    pub accepted_terms: String,
    pub authorization: ProxySpendAuthorization,
    pub settlement_policy: ProxySettlementPolicy,
    pub max_checkpoints: u64,
    result: Value,
    recorded_at: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    ok: bool,
    schema_version: u32,
    lane: String,
    accepted_terms: String,
    request_nonce: String,
    requester: String,
    context: Context,
    proof: Proof,
    accepted: Accepted,
    session: Value,
    reservation: Value,
    billing: Value,
    receipt_head: Value,
    resolution: Value,
    expiry: Value,
}

/// Created only by a successful bounded, challenge-bound trusted peer read.
/// Holding this value never reserves a model slot or permits a duplicate POST.
pub struct Observation {
    wire: Wire,
    started: Instant,
}
impl Observation {
    pub fn confirms_waiver(&self, expected: &ProxyReservationClosure) -> Result<bool> {
        require(
            self.started.elapsed() <= FRESHNESS,
            "canonical financial observation expired",
        )?;
        let record = &self.wire.resolution;
        if record.is_null() {
            return Ok(false);
        }
        let actual: ProxyReservationClosure = serde_json::from_value(record["closure"].clone())?;
        let t = &self.wire.accepted.authorization.terms;
        actual
            .verify(t, crate::receipts::verify_signature)
            .map_err(|_| invalid("canonical waiver signatures rejected"))?;
        require(
            &actual == expected
                && record["type"] == "proxy_execution_resolution"
                && record["accepted_terms"] == self.wire.accepted_terms
                && record["result"]["ok"] == true
                && record["result"]["retry_safe"] == true
                && record["result"]["retained_au"] == "0"
                && record["recorded_at"]
                    == format!(
                        "proxy/close/{}",
                        actual
                            .body
                            .digest()
                            .map_err(|_| invalid("invalid waiver"))?
                    ),
            "canonical waiver conflicts with retained outcome",
        )?;
        let r = &self.wire.reservation;
        require(
            r["status"] == "closed"
                && r["reservation_id"] == t.reservation_id
                && r["user"] == t.buyer_pubkey
                && r["provider"] == t.offer.provider_pubkey
                && r["rail"] == serde_json::to_value(t.rail)?
                && !r["closed_at"].is_null(),
            "canonical waiver reservation differs",
        )?;
        Ok(true)
    }
    pub fn receipt_head(&self) -> Result<Option<ProxyUsageReceipt>> {
        require(
            self.started.elapsed() <= FRESHNESS,
            "canonical financial observation expired",
        )?;
        let head = &self.wire.receipt_head;
        if head.is_null() {
            return Ok(None);
        }
        let receipt: ProxyUsageReceipt = serde_json::from_value(head["receipt"].clone())?;
        let accepted = &self.wire.accepted;
        receipt
            .verify(
                &accepted.authorization.terms,
                &accepted.settlement_policy,
                None,
                crate::receipts::verify_signature,
            )
            .map_err(|_| invalid("canonical receipt signatures rejected"))?;
        require(
            head["type"] == "canonical_receipt_head"
                && head["lane"] == "proxy"
                && head["accepted_terms"] == self.wire.accepted_terms
                && receipt.body.accepted_terms == self.wire.accepted_terms
                && head["receipt_hash"]
                    == receipt
                        .body
                        .digest()
                        .map_err(|_| invalid("invalid canonical receipt"))?
                && head["receipt_seq"] == receipt.body.seq
                && head["settlement_ready"] == receipt.body.final_receipt,
            "canonical receipt differs",
        )?;
        Ok(Some(receipt))
    }
    /// Exact canonical finalization resolves only the financial part of recovery.
    /// It does not prove backend capacity was released or a payout delivered.
    pub fn confirms_receipt(&self, expected: &ProxyUsageReceipt) -> Result<bool> {
        let Some(actual) = self.receipt_head()? else {
            return Ok(false);
        };
        if !actual.body.final_receipt {
            return Ok(false);
        }
        require(
            &actual == expected,
            "canonical final receipt conflicts with retained outcome",
        )?;
        let t = &self.wire.accepted.authorization.terms;
        let r = &self.wire.reservation;
        require(
            r["status"] == "closed"
                && r["reservation_id"] == t.reservation_id
                && r["user"] == t.buyer_pubkey
                && r["provider"] == t.offer.provider_pubkey
                && r["rail"] == serde_json::to_value(t.rail)?
                && !r["closed_at"].is_null(),
            "canonical financial closure differs",
        )?;
        Ok(true)
    }
    pub fn accepted(&self) -> &Accepted {
        &self.wire.accepted
    }
    pub fn proof(&self) -> &Proof {
        &self.wire.proof
    }
    pub fn has_receipt(&self) -> bool {
        !self.wire.receipt_head.is_null()
    }
    pub fn is_closed(&self) -> bool {
        !self.wire.resolution.is_null() || !self.wire.expiry.is_null()
    }
    /// A fresh still-reserved financial observation is one prerequisite only.
    /// The controller must independently acquire capacity and enforce its durable
    /// dispatch fence. Recovery cannot use this to resubmit unknown prior work.
    pub fn initial_binding(&self) -> Result<Binding> {
        let w = &self.wire;
        let t = &w.accepted.authorization.terms;
        require(
            self.started.elapsed() <= FRESHNESS,
            "canonical financial observation expired",
        )?;
        require(
            !self.has_receipt() && !self.is_closed(),
            "attempt already has execution or closure evidence",
        )?;
        require(
            w.context.epoch < t.reservation_expires_after_epoch,
            "reservation no longer admits new execution",
        )?;
        self.reserved_binding()
    }
    fn reserved_binding(&self) -> Result<Binding> {
        let w = &self.wire;
        let t = &w.accepted.authorization.terms;
        let s = &w.session;
        let r = &w.reservation;
        let a = &w.billing;
        require(
            s["type"] == "targeted_spend_session"
                && s["lane"] == "proxy"
                && s["accepted_terms"] == w.accepted_terms
                && s["settlement_ready"] == false
                && s["max_spend_au"] == t.max_spend_au.to_string()
                && s["closed_at"].is_null()
                && s["authorization"] == serde_json::to_value(&w.accepted.authorization)?
                && s["settlement_policy"] == serde_json::to_value(&w.accepted.settlement_policy)?,
            "reserved session differs",
        )?;
        for v in [s, r] {
            require(
                v["billing_id"] == t.billing_id
                    && v["billing_attempt"] == t.billing_attempt
                    && v["billing_epoch"] == t.billing_epoch
                    && v["session_id"] == t.session_id
                    && v["reservation_id"] == t.reservation_id
                    && v["user"] == t.buyer_pubkey
                    && v["provider"] == t.offer.provider_pubkey
                    && v["payout_revision"] == t.payout_revision
                    && v["rail"] == serde_json::to_value(t.rail)?,
                "financial identity differs",
            )?;
        }
        require(
            r["type"] == "receipt_reservation_identity"
                && r["lane"] == "proxy"
                && r["accepted_terms"] == w.accepted_terms
                && r["status"] == "active",
            "reservation is not active",
        )?;
        require(
            a["type"] == "proxy_billing_anchor"
                && a["lane"] == "proxy"
                && a["billing_id"] == t.billing_id
                && a["user"] == t.buyer_pubkey
                && a["rail"] == serde_json::to_value(t.rail)?
                && a["request_hash"] == t.request_hash
                && a["endpoint_contract"] == t.endpoint_contract
                && a["latest_attempt"] == t.billing_attempt
                && a["latest_accepted_terms"] == w.accepted_terms
                && a["active_reservation_id"] == t.reservation_id
                && a["retry_blocked"] == false
                && a["spent_au"] == t.prior_spend_au.to_string()
                && a["reserved_au"] == t.max_spend_au.to_string()
                && a["max_total_spend_au"] == t.max_total_spend_au.to_string(),
            "billing anchor is not active for this attempt",
        )?;
        binding(&w.accepted)
    }
}

fn binding(accepted: &Accepted) -> Result<Binding> {
    terms_binding(&accepted.authorization.terms)
}
pub(crate) fn terms_binding(t: &mayhem_proto::proxy::finance::ProxySpendTerms) -> Result<Binding> {
    let d = |s: &str| Digest::new(s).map_err(|_| invalid("invalid financial digest"));
    Ok(Binding {
        request_hash: d(&t.request_hash)?,
        endpoint: t.offer.endpoint,
        contract_version: t.contract_version,
        provider_pubkey: d(&t.offer.provider_pubkey)?,
        market_id: d(&t.offer.market_id)?,
        offer_digest: d(&t.offer.digest().map_err(|_| invalid("invalid offer"))?)?,
        endpoint_contract: d(&t.endpoint_contract)?,
        metering_policy: d(&t.offer.metering_policy_hash)?,
        accepted_terms: d(&t.digest().map_err(|_| invalid("invalid financial terms"))?)?,
        reservation: d(&t.reservation_id)?,
        capacity_lease: d(&t.capacity_lease)?,
        connection_digest: d(&t.connection_digest)?,
        connection_revision: t.connection_revision,
        recipe_digest: d(&t.recipe_hash)?,
        rail: t.rail,
    })
}

/// Immutable owned evidence retained from the initial canonical observation.
/// Deserializing it never renews freshness or authorizes another execution.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Retained {
    accepted: Accepted,
    context: Context,
    proof: Proof,
}
impl std::fmt::Debug for Retained {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetainedFinancialAcceptance")
            .finish_non_exhaustive()
    }
}
impl Retained {
    pub(crate) fn capture(observation: &Observation) -> Result<Self> {
        observation.initial_binding()?;
        Ok(Self {
            accepted: observation.wire.accepted.clone(),
            context: observation.wire.context.clone(),
            proof: observation.wire.proof.clone(),
        })
    }
    pub fn accepted(&self) -> &Accepted {
        &self.accepted
    }
    pub fn context(&self) -> &Context {
        &self.context
    }
    pub fn validate_for(&self, expected: &Binding) -> Result<()> {
        let t = &self.accepted.authorization.terms;
        t.validate()
            .map_err(|_| invalid("invalid retained terms"))?;
        self.proof.validate()?;
        self.context.identity().validate()?;
        require(
            self.accepted.accepted_terms
                == t.digest().map_err(|_| invalid("invalid retained terms"))?
                && self
                    .accepted
                    .settlement_policy
                    .digest()
                    .map_err(|_| invalid("invalid retained policy"))?
                    == t.settlement_policy_hash
                && t.network_id == self.context.network_id
                && t.msb_bootstrap == self.context.msb_bootstrap
                && t.subnet_bootstrap == self.context.subnet_bootstrap
                && binding(&self.accepted)? == *expected,
            "retained financial evidence differs",
        )
    }
}

pub struct Client {
    http: reqwest::Client,
    endpoint: Url,
    publication_endpoint: Url,
    identity: Identity,
    requester: String,
    // Bounded control I/O; it neither occupies nor represents inference slots.
    slots: tokio::sync::Semaphore,
}
impl Client {
    pub fn new(
        rpc_base: &str,
        identity: Identity,
        requester: String,
        max_reads: usize,
    ) -> Result<Self> {
        identity.validate()?;
        require(
            hex(&requester) && (1..=64).contains(&max_reads),
            "invalid financial client configuration",
        )?;
        let base = Url::parse(&format!("{}/", rpc_base.trim_end_matches('/')))
            .map_err(|_| invalid("invalid peer RPC URL"))?;
        require(
            matches!(base.scheme(), "http" | "https")
                && base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none(),
            "invalid peer RPC URL",
        )?;
        require(
            base.scheme() == "https"
                || matches!(base.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
                || matches!(base.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback()),
            "financial peer RPC requires HTTPS or literal loopback HTTP",
        )?;
        let endpoint = base
            .join("proxy/financial-state")
            .map_err(|_| invalid("invalid peer RPC path"))?;
        let publication_endpoint = base
            .join("contract/feature")
            .map_err(|_| invalid("invalid peer RPC path"))?;
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(FRESHNESS)
            .build()
            .map_err(Error::Transport)?;
        Ok(Self {
            http,
            endpoint,
            publication_endpoint,
            identity,
            requester,
            slots: tokio::sync::Semaphore::new(max_reads),
        })
    }
    pub(crate) fn identity(&self) -> &Identity {
        &self.identity
    }
    pub(crate) fn requester(&self) -> &str {
        &self.requester
    }
    /// Only the buyer's durable recovery controller calls this with its saved
    /// envelope. Acknowledgment is not canonical reservation confirmation.
    pub(crate) async fn submit_reservation(
        &self,
        authorization: &ProxySpendAuthorization,
        at: u64,
    ) -> Result<()> {
        let t = &authorization.terms;
        authorization
            .verify(crate::receipts::verify_signature)
            .map_err(|_| invalid("reservation signatures rejected"))?;
        require(
            at <= PROXY_MAX_SAFE_INTEGER
                && t.network_id == self.identity.network_id
                && t.msb_bootstrap == self.identity.msb_bootstrap
                && t.subnet_bootstrap == self.identity.subnet_bootstrap
                && t.buyer_pubkey == self.requester,
            "reservation publication identity differs",
        )?;
        let digest = t
            .digest()
            .map_err(|_| invalid("invalid reservation terms"))?;
        self.submit_feature(
            format!("proxy/spend/{digest}"),
            json!({"op":"proxy_spend_reserve","authorization":authorization,"at":at}),
        )
        .await
    }
    /// Submit the same durable, dual-signed receipt through the existing canonical
    /// publication journal. HTTP success is NOT settlement confirmation. Ambiguous
    /// submission must be followed by observation of the same accepted terms.
    pub async fn submit_receipt(
        &self,
        authorization: &ProxySpendAuthorization,
        policy: &ProxySettlementPolicy,
        receipt: &ProxyUsageReceipt,
    ) -> Result<()> {
        let t = &authorization.terms;
        require(
            t.network_id == self.identity.network_id
                && t.msb_bootstrap == self.identity.msb_bootstrap
                && t.subnet_bootstrap == self.identity.subnet_bootstrap
                && t.offer.provider_pubkey == self.requester,
            "financial publication identity differs",
        )?;
        authorization
            .verify(crate::receipts::verify_signature)
            .map_err(|_| invalid("accepted signatures rejected"))?;
        receipt
            .verify(t, policy, None, crate::receipts::verify_signature)
            .map_err(|_| invalid("receipt signatures rejected"))?;
        let key = format!(
            "proxy/usage/{}",
            receipt
                .body
                .digest()
                .map_err(|_| invalid("invalid receipt"))?
        );
        self.submit_feature(
            key,
            json!({"op":"proxy_record_usage","provider":self.requester,"receipt":receipt}),
        )
        .await
    }
    pub async fn submit_waiver(
        &self,
        authorization: &ProxySpendAuthorization,
        closure: &ProxyReservationClosure,
    ) -> Result<()> {
        let t = &authorization.terms;
        require(
            t.network_id == self.identity.network_id
                && t.msb_bootstrap == self.identity.msb_bootstrap
                && t.subnet_bootstrap == self.identity.subnet_bootstrap
                && t.offer.provider_pubkey == self.requester,
            "financial publication identity differs",
        )?;
        authorization
            .verify(crate::receipts::verify_signature)
            .map_err(|_| invalid("accepted signatures rejected"))?;
        closure
            .verify(t, crate::receipts::verify_signature)
            .map_err(|_| invalid("waiver signatures rejected"))?;
        let key = format!(
            "proxy/close/{}",
            closure
                .body
                .digest()
                .map_err(|_| invalid("invalid waiver"))?
        );
        self.submit_feature(
            key,
            json!({"op":"proxy_close_reservation","provider":self.requester,"closure":closure}),
        )
        .await
    }
    /// Buyer-only release under the ORIGINAL opt-in policy and canonical epoch.
    /// This never proves execution stopped and cannot authorize redispatch.
    pub async fn submit_expiry(
        &self,
        observation: &Observation,
        expiry: &mayhem_proto::proxy::finance::ProxyReservationExpiry,
    ) -> Result<()> {
        let t = &observation.accepted().authorization.terms;
        require(
            observation.started.elapsed() <= FRESHNESS
                && t.network_id == self.identity.network_id
                && t.msb_bootstrap == self.identity.msb_bootstrap
                && t.subnet_bootstrap == self.identity.subnet_bootstrap
                && t.buyer_pubkey == self.requester,
            "buyer expiry identity or observation differs",
        )?;
        observation
            .accepted()
            .authorization
            .verify(crate::receipts::verify_signature)
            .map_err(|_| invalid("accepted signatures rejected"))?;
        expiry
            .verify(
                t,
                &observation.accepted().settlement_policy,
                observation.wire.context.epoch,
                crate::receipts::verify_signature,
            )
            .map_err(|_| invalid("expiry policy, epoch or signature rejected"))?;
        let key = format!(
            "proxy/expire/{}",
            expiry
                .body
                .digest()
                .map_err(|_| invalid("invalid expiry"))?
        );
        self.submit_feature(
            key,
            json!({"op":"proxy_expire_reservation","buyer":self.requester,"expiry":expiry}),
        )
        .await
    }
    async fn submit_feature(&self, key: String, value: Value) -> Result<()> {
        let _permit = self
            .slots
            .try_acquire()
            .map_err(|_| invalid("financial publication capacity unavailable"))?;
        let bytes = serde_json::to_vec(&json!({"feature":"mayhem","key":key,"value":value}))?;
        require(
            bytes.len() <= MAX_BYTES,
            "financial publication exceeds bound",
        )?;
        let mut response = self
            .http
            .post(self.publication_endpoint.clone())
            .header("content-type", "application/json")
            .body(bytes)
            .send()
            .await
            .map_err(Error::Transport)?;
        require(
            response.status().is_success(),
            "financial publication needs reconciliation",
        )?;
        let mut size = 0usize;
        while let Some(chunk) = response.chunk().await.map_err(Error::Transport)? {
            size = size
                .checked_add(chunk.len())
                .ok_or_else(|| invalid("financial reply exceeds bound"))?;
            require(size <= MAX_BYTES, "financial reply exceeds bound")?;
        }
        Ok(())
    }
    pub async fn observe(&self, authorization: &ProxySpendAuthorization) -> Result<Observation> {
        let _permit = self
            .slots
            .try_acquire()
            .map_err(|_| invalid("financial observation capacity unavailable"))?;
        let t = &authorization.terms;
        t.validate()
            .map_err(|_| invalid("invalid financial terms"))?;
        require(
            t.network_id == self.identity.network_id
                && t.msb_bootstrap == self.identity.msb_bootstrap
                && t.subnet_bootstrap == self.identity.subnet_bootstrap
                && [&t.buyer_pubkey, &t.offer.provider_pubkey].contains(&&self.requester),
            "financial request identity differs",
        )?;
        let digest = t.digest().map_err(|_| invalid("invalid financial terms"))?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| invalid("financial challenge unavailable"))?;
        let nonce = nonce.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let started = Instant::now();
        let mut response = self
            .http
            .post(self.endpoint.clone())
            .json(&json!({"accepted_terms":digest,"request_nonce":nonce}))
            .send()
            .await
            .map_err(Error::Transport)?;
        require(
            response.status().is_success(),
            "canonical financial observation unavailable",
        )?;
        require(
            response
                .content_length()
                .is_none_or(|n| n <= MAX_BYTES as u64),
            "financial response exceeds bound",
        )?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(Error::Transport)? {
            require(
                bytes.len().saturating_add(chunk.len()) <= MAX_BYTES,
                "financial response exceeds bound",
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
                && w.accepted_terms == digest
                && w.context.identity() == self.identity
                && w.context.epoch <= PROXY_MAX_SAFE_INTEGER,
            "financial response binding differs",
        )?;
        w.proof.validate()?;
        let a = &w.accepted;
        require(
            a.kind == "proxy_accepted_spend"
                && a.accepted_terms == digest
                && a.authorization == *authorization
                && a.max_checkpoints <= PROXY_MAX_SAFE_INTEGER
                && a.recorded_at == format!("proxy/spend/{digest}")
                && a.result["ok"] == true
                && a.result["op"] == "proxySpendReserve"
                && a.result["accepted_terms"] == digest
                && a.settlement_policy
                    .digest()
                    .map_err(|_| invalid("invalid settlement policy"))?
                    == t.settlement_policy_hash,
            "accepted financial record differs",
        )?;
        Ok(Observation { wire: w, started })
    }
}
