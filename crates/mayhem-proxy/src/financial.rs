//! Exact-attempt canonical finance observations from the configured trusted Core
//! peer RPC. Its authenticated service verifies the indexer's signed snapshot.
//! An arbitrary upstream HTTP response is never a financial authority.
use crate::{
    attempts::{Binding, Digest},
    discovery::{hex, Context, Identity, Proof},
    invalid, require, Error, Result,
};
use mayhem_proto::proxy::{
    finance::{ProxySettlementPolicy, ProxySpendAuthorization},
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
    let t = &accepted.authorization.terms;
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
        accepted_terms: d(&accepted.accepted_terms)?,
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
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(FRESHNESS)
            .build()
            .map_err(Error::Transport)?;
        Ok(Self {
            http,
            endpoint,
            identity,
            requester,
            slots: tokio::sync::Semaphore::new(max_reads),
        })
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
