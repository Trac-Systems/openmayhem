//! Authenticated pre-acceptance exchange. Canonical quote/offer checks, durable
//! signing, capacity and financial publication remain mandatory outside transport.
//! Context is supplied by the trusted session dispatcher, not authenticated merely
//! because a caller can deserialize it. No raw signer or public RPC lives here.
use crate::{
    attempts::{Digest, FailureSnapshot, Identity, SignedProviderAcceptance},
    endpoint::{PublicAdapter, PublicAdapterSnapshot},
    exchange::{
        self,
        channel::{Link, Purpose, Wire, WireMessage},
        Error, Result, Role,
    },
    financial::{negotiation::BuyerOffer, quote::SessionBinding},
};
use mayhem_bridge::ScBridgeConfig;
use mayhem_proto::proxy::{
    finance::ProxySpendTerms, ProxyOffer, ProxyRail, PROXY_MAX_SAFE_INTEGER,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// Public immutable identity of one negotiation, including its exact request and
/// offer. It contains no prompt, connection address, credential or payout target.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub schema_version: u32,
    pub network_id: String,
    pub msb_bootstrap: Digest,
    pub subnet_bootstrap: Digest,
    pub contract_version: u32,
    pub session_id: Digest,
    pub buyer: Digest,
    pub billing_id: Digest,
    pub billing_attempt: u64,
    pub request_hash: Digest,
    pub offer: ProxyOffer,
    pub rail: ProxyRail,
    pub settlement_policy_hash: Digest,
}
impl Context {
    pub fn validate(&self) -> Result<()> {
        self.offer.validate().map_err(|_| Error::Protocol)?;
        let identity = Identity {
            network_id: self.network_id.clone(),
            msb_bootstrap: self.msb_bootstrap.clone(),
            subnet_bootstrap: self.subnet_bootstrap.clone(),
            controller_pubkey: self.buyer.clone(),
        };
        identity.validate().map_err(|_| Error::Identity)?;
        if self.schema_version != 1
            || self.contract_version == 0
            || self.billing_attempt == 0
            || self.billing_attempt > PROXY_MAX_SAFE_INTEGER
            || !self.offer.accepted_rails.contains(&self.rail)
            || serde_json::to_vec(self).map_err(|_| Error::Protocol)?.len() > 32 * 1024
        {
            return Err(Error::Protocol);
        }
        Ok(())
    }
    pub fn digest(&self) -> Result<Digest> {
        self.validate()?;
        let bytes = mayhem_proto::stable_json_bytes(
            &serde_json::to_value(self).map_err(|_| Error::Protocol)?,
        )
        .map_err(|_| Error::Protocol)?;
        Ok(Digest::hash(
            "mayhem/proxy/negotiation-context/v1",
            &[&bytes],
        ))
    }
    pub fn invocation(&self) -> Result<Digest> {
        self.validate()?;
        Ok(Digest::hash(
            "mayhem/proxy/buyer-invocation/v1",
            &[
                self.network_id.as_bytes(),
                self.msb_bootstrap.as_str().as_bytes(),
                self.subnet_bootstrap.as_str().as_bytes(),
                self.buyer.as_str().as_bytes(),
                self.billing_id.as_str().as_bytes(),
                &self.billing_attempt.to_le_bytes(),
            ],
        ))
    }
    fn bind_terms(&self, t: &ProxySpendTerms) -> Result<()> {
        t.validate().map_err(|_| Error::Protocol)?;
        if t.network_id != self.network_id
            || t.msb_bootstrap != self.msb_bootstrap.as_str()
            || t.subnet_bootstrap != self.subnet_bootstrap.as_str()
            || t.contract_version != self.contract_version
            || t.session_id != self.session_id.as_str()
            || t.buyer_pubkey != self.buyer.as_str()
            || t.billing_id != self.billing_id.as_str()
            || t.billing_attempt != self.billing_attempt
            || t.request_hash != self.request_hash.as_str()
            || t.offer != self.offer
            || t.rail != self.rail
            || t.settlement_policy_hash != self.settlement_policy_hash.as_str()
        {
            return Err(Error::Identity);
        }
        Ok(())
    }
    fn link(&self, local: &Identity, role: Role) -> Result<Link> {
        self.validate()?;
        local.validate().map_err(|_| Error::Identity)?;
        let (own, remote) = match role {
            Role::Buyer => (self.buyer.as_str(), self.offer.provider_pubkey.as_str()),
            Role::Provider => (self.offer.provider_pubkey.as_str(), self.buyer.as_str()),
        };
        if local.network_id != self.network_id
            || local.msb_bootstrap != self.msb_bootstrap
            || local.subnet_bootstrap != self.subnet_bootstrap
            || local.controller_pubkey.as_str() != own
        {
            return Err(Error::Identity);
        }
        Ok(Link {
            session_id: self.session_id.as_str().into(),
            remote: Digest::new(remote).map_err(|_| Error::Identity)?,
            role,
            purpose: Purpose::Negotiation(self.digest()?),
        })
    }
}

/// Claims to validate against the buyer's own canonical quote, never a substitute
/// for a provider-held capacity Reservation or canonical financial observation.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub adapter: PublicAdapterSnapshot,
    pub reservation_id: Digest,
    pub connection_digest: Digest,
    pub connection_revision: u64,
    pub capacity_lease: Digest,
}
impl Proposal {
    fn validate(&self, context: &Context) -> Result<()> {
        let adapter = PublicAdapter::restore(self.adapter.clone()).map_err(|_| Error::Protocol)?;
        if adapter.endpoint() != context.offer.endpoint
            || self.connection_revision == 0
            || self.connection_revision > PROXY_MAX_SAFE_INTEGER
        {
            return Err(Error::Protocol);
        }
        Ok(())
    }
    pub fn session_binding(&self, context: &Context) -> Result<SessionBinding> {
        self.validate(context)?;
        Ok(SessionBinding {
            session_id: context.session_id.clone(),
            reservation_id: self.reservation_id.clone(),
            connection_digest: self.connection_digest.clone(),
            capacity_lease: self.capacity_lease.clone(),
        })
    }
    fn bind_terms(&self, t: &ProxySpendTerms) -> Result<()> {
        let adapter = PublicAdapter::restore(self.adapter.clone()).map_err(|_| Error::Protocol)?;
        if t.endpoint_contract != adapter.contract_hash().as_str()
            || t.recipe_hash != adapter.recipe_hash().as_str()
            || t.reservation_id != self.reservation_id.as_str()
            || t.connection_digest != self.connection_digest.as_str()
            || t.connection_revision != self.connection_revision
            || t.capacity_lease != self.capacity_lease.as_str()
        {
            return Err(Error::Identity);
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Message {
    Request { request: Value },
    Proposal { proposal: Proposal },
    Offer { offer: BuyerOffer },
    Accepted { value: SignedProviderAcceptance },
    Recover,
    Refused { failure: FailureSnapshot },
}
impl WireMessage for Message {
    fn sender(&self) -> Role {
        match self {
            Self::Request { .. } | Self::Offer { .. } | Self::Recover => Role::Buyer,
            _ => Role::Provider,
        }
    }
    fn validate(&self) -> Result<()> {
        match self {
            Self::Offer { offer } => offer.verify().map_err(|_| Error::Identity)?,
            Self::Accepted { value } => {
                value
                    .authorization
                    .verify(crate::receipts::verify_signature)
                    .map_err(|_| Error::Identity)?;
                if value.policy.digest().map_err(|_| Error::Protocol)?
                    != value.authorization.terms.settlement_policy_hash
                {
                    return Err(Error::Identity);
                }
            }
            Self::Refused { failure } => {
                failure.to_failure().map_err(|_| Error::Protocol)?;
            }
            _ => (),
        }
        Ok(())
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Start,
    Requested,
    Proposed,
    Offered,
    Recovering,
    Finished,
}
pub struct Channel {
    wire: Wire,
    context: Context,
    role: Role,
    phase: Phase,
    proposal: Option<Proposal>,
    offered: Option<BuyerOffer>,
    accepted: Option<SignedProviderAcceptance>,
    control_deadline: Duration,
}
impl Channel {
    pub async fn connect(
        config: ScBridgeConfig,
        context: Context,
        local: &Identity,
        role: Role,
        limits: exchange::Limits,
    ) -> Result<Self> {
        let control_deadline = config
            .operation_deadline
            .filter(|d| !d.is_zero())
            .ok_or(Error::Protocol)?;
        let wire = Wire::connect(config, context.link(local, role)?, limits).await?;
        Ok(Self {
            wire,
            context,
            role,
            phase: Phase::Start,
            proposal: None,
            offered: None,
            accepted: None,
            control_deadline,
        })
    }
    pub fn context(&self) -> &Context {
        &self.context
    }
    fn next(&self, message: &Message) -> Result<Phase> {
        message.validate()?;
        match (self.phase, message) {
            (Phase::Start, Message::Request { request }) => {
                if self.context.contract_version != mayhem_proto::CONTRACT_VERSION
                    || !request.is_object()
                    || mayhem_proto::endpoint_request_fingerprint(request)
                        != self.context.request_hash.as_str()
                {
                    return Err(Error::Identity);
                }
                Ok(Phase::Requested)
            }
            (Phase::Requested, Message::Proposal { proposal }) => {
                proposal.validate(&self.context)?;
                Ok(Phase::Proposed)
            }
            (Phase::Proposed, Message::Offer { offer }) => {
                self.context.bind_terms(&offer.terms)?;
                self.proposal
                    .as_ref()
                    .ok_or(Error::Protocol)?
                    .bind_terms(&offer.terms)?;
                Ok(Phase::Offered)
            }
            (Phase::Offered | Phase::Recovering, Message::Accepted { value }) => {
                self.context.bind_terms(&value.authorization.terms)?;
                if self.phase == Phase::Offered
                    && self.offered.as_ref().is_none_or(|o| {
                        o.terms != value.authorization.terms
                            || o.buyer_sig != value.authorization.buyer_sig
                    })
                {
                    return Err(Error::Identity);
                }
                Ok(Phase::Finished)
            }
            (Phase::Start, Message::Recover) => Ok(Phase::Recovering),
            (
                Phase::Requested | Phase::Proposed | Phase::Offered | Phase::Recovering,
                Message::Refused { .. },
            ) => Ok(Phase::Finished),
            _ => Err(Error::Protocol),
        }
    }
    fn record(&mut self, phase: Phase, message: &Message) {
        match message {
            Message::Proposal { proposal } => self.proposal = Some(proposal.clone()),
            Message::Offer { offer } => self.offered = Some(offer.clone()),
            Message::Accepted { value } => self.accepted = Some(value.clone()),
            _ => (),
        }
        self.phase = phase;
    }
    pub async fn send(&mut self, message: &Message) -> Result<()> {
        let next = self.next(message)?;
        match tokio::time::timeout(self.control_deadline, self.wire.send(message)).await {
            Ok(result) => result?,
            Err(_) => {
                self.wire.poison();
                return Err(Error::Interrupted);
            }
        }
        self.record(next, message);
        Ok(())
    }
    /// Explicit finite control wait; this is never a model generation deadline.
    pub async fn receive(&mut self, wait: Duration) -> Result<Received> {
        let message: Message = self.wire.receive(Some(wait)).await?;
        let next = match self.next(&message) {
            Ok(p) => p,
            Err(e) => {
                self.wire.poison();
                return Err(e);
            }
        };
        self.record(next, &message);
        Ok(Received {
            context: self.context.digest()?,
            recipient: self.role,
            message,
        })
    }
    /// Promotion proves negotiated identity, not financial admission. The paid
    /// Session still checks canonical funding and capacity before dispatch.
    pub fn into_paid(self, local: &Identity) -> Result<exchange::Channel> {
        if self.phase != Phase::Finished {
            return Err(Error::Protocol);
        }
        let accepted = self.accepted.ok_or(Error::Protocol)?;
        let session = exchange::Session::new(accepted.authorization, local, self.role)?;
        exchange::Channel::from_negotiation(self.wire, session)
    }
    pub async fn close(self) -> Result<()> {
        self.wire.close().await
    }
}

/// Constructible only after transport peer, context, order and message checks.
pub struct Received {
    context: Digest,
    recipient: Role,
    message: Message,
}
impl Received {
    pub fn message(&self) -> &Message {
        &self.message
    }
    pub fn into_message(self, context: &Context, recipient: Role) -> Result<Message> {
        if self.context != context.digest()? || self.recipient != recipient {
            return Err(Error::Identity);
        }
        Ok(self.message)
    }
}
