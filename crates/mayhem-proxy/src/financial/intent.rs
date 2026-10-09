//! Canonical observation of the original signed intention, even after its offer
//! was withdrawn. A missing HTTP result is never evidence of non-admission.
use super::*;
use negotiation::BuyerOffer;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Open,
    Admitted,
    Expired,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    ok: bool,
    schema_version: u32,
    lane: String,
    requester: String,
    request_nonce: String,
    intent: BuyerOffer,
    accepted_terms: String,
    status: Status,
    authorization: Option<ProxySpendAuthorization>,
    context: Context,
    proof: Proof,
}

/// Constructible only through the trusted, bounded, challenge-bound Core read.
/// Does not independently verify Merkle proofs; the signed indexer service does.
pub struct Observation {
    wire: Wire,
    started: Instant,
}
impl Observation {
    pub(crate) fn fresh(&self) -> Result<()> {
        require(
            self.started.elapsed() <= FRESHNESS,
            "intent observation expired",
        )
    }
    pub fn status(&self) -> Result<Status> {
        self.fresh()?;
        Ok(self.wire.status)
    }
    pub fn authorization(&self) -> Result<Option<&ProxySpendAuthorization>> {
        self.fresh()?;
        Ok(self.wire.authorization.as_ref())
    }
    pub fn proof(&self) -> &Proof {
        &self.wire.proof
    }
    pub(crate) fn absent(&self, offer: &BuyerOffer, requester: &str) -> Result<RetainedAbsence> {
        self.fresh()?;
        require(
            self.wire.status == Status::Expired
                && self.wire.intent == *offer
                && self.wire.requester == requester
                && self.wire.authorization.is_none(),
            "intent is not canonically expired without admission",
        )?;
        let proof = RetainedAbsence {
            accepted_terms: self.wire.accepted_terms.clone(),
            requester: requester.into(),
            context: self.wire.context.clone(),
            proof: self.wire.proof.clone(),
        };
        proof.validate_for(&offer.terms, requester)?;
        Ok(proof)
    }
}

/// Private historical evidence. Deserialization cannot refresh an observation
/// or release another capacity lease/obligation. It binds the exact terms only.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetainedAbsence {
    accepted_terms: String,
    requester: String,
    context: Context,
    proof: Proof,
}
impl RetainedAbsence {
    pub(crate) fn validate_for(
        &self,
        t: &mayhem_proto::proxy::finance::ProxySpendTerms,
        requester: &str,
    ) -> Result<()> {
        self.proof.validate()?;
        self.context.identity().validate()?;
        require(
            self.accepted_terms == t.digest().map_err(invalid)?
                && self.requester == requester
                && [t.buyer_pubkey.as_str(), t.offer.provider_pubkey.as_str()].contains(&requester)
                && self.context.network_id == t.network_id
                && self.context.msb_bootstrap == t.msb_bootstrap
                && self.context.subnet_bootstrap == t.subnet_bootstrap
                && self.context.epoch >= t.billing_epoch
                && self.context.epoch <= PROXY_MAX_SAFE_INTEGER,
            "retained non-admission evidence differs",
        )
    }
}

impl Client {
    pub async fn intent_state(&self, intent: &BuyerOffer) -> Result<Observation> {
        intent.verify()?;
        let t = &intent.terms;
        require(
            [t.buyer_pubkey.as_str(), t.offer.provider_pubkey.as_str()]
                .contains(&self.requester.as_str())
                && t.network_id == self.identity.network_id
                && t.msb_bootstrap == self.identity.msb_bootstrap
                && t.subnet_bootstrap == self.identity.subnet_bootstrap,
            "intent request identity differs",
        )?;
        let body_size = serde_json::to_vec(intent)?.len();
        require(body_size <= 32768 - 256, "intent query exceeds bound")?;
        let _permit = self
            .slots
            .try_acquire()
            .map_err(|_| invalid("intent observation capacity unavailable"))?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| invalid("intent challenge unavailable"))?;
        let nonce = nonce.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let started = Instant::now();
        let mut response = self
            .http
            .post(
                self.endpoint
                    .join("intent-state")
                    .map_err(|_| invalid("invalid intent endpoint"))?,
            )
            .json(&json!({"intent":intent,"request_nonce":nonce}))
            .send()
            .await
            .map_err(Error::Transport)?;
        require(
            response.status().is_success(),
            "canonical intent observation unavailable",
        )?;
        require(
            response
                .content_length()
                .is_none_or(|n| n <= MAX_BYTES as u64),
            "intent response exceeds bound",
        )?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(Error::Transport)? {
            require(
                bytes.len().saturating_add(chunk.len()) <= MAX_BYTES,
                "intent response exceeds bound",
            )?;
            bytes.extend_from_slice(&chunk);
        }
        let wire: Wire = serde_json::from_slice(&bytes)?;
        require(
            started.elapsed() <= FRESHNESS
                && wire.ok
                && wire.schema_version == 1
                && wire.lane == "proxy"
                && wire.requester == self.requester
                && wire.request_nonce == nonce
                && wire.intent == *intent
                && wire.accepted_terms == t.digest().map_err(invalid)?
                && wire.context.identity() == self.identity
                && wire.context.epoch <= PROXY_MAX_SAFE_INTEGER,
            "intent response binding differs",
        )?;
        wire.proof.validate()?;
        match (&wire.status, &wire.authorization) {
            (Status::Admitted, Some(auth)) => {
                auth.verify(crate::receipts::verify_signature)
                    .map_err(invalid)?;
                require(
                    auth.terms == *t && auth.buyer_sig == intent.buyer_sig,
                    "admitted intention differs",
                )?;
            }
            (Status::Expired, None) => require(
                wire.context.epoch >= t.billing_epoch,
                "intention epoch remains open",
            )?,
            (Status::Open, None) => require(
                wire.context.epoch < t.billing_epoch,
                "intention epoch is closed",
            )?,
            _ => return Err(invalid("intent admission state is inconsistent")),
        }
        Ok(Observation { wire, started })
    }
}
