//! Authenticated exchange for an already negotiated proxy spend authorization.
//! Peer/session identity comes from the existing protected SC-Bridge, not a body
//! field. This is not offer negotiation or permission to skip canonical admission.
mod channel;
use crate::{
    attempts::{Digest, Identity},
    execution::{Cancellation, PaidExecutor, UnsettledReply},
    financial::recovery::SignedAcknowledgment,
    signing::{ProviderReceipt, ProviderWaiver},
};
pub use channel::{Channel, Limits};
use mayhem_proto::proxy::finance::ProxySpendAuthorization;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("proxy exchange peer, session or authorization differs")]
    Identity,
    #[error("proxy exchange message is invalid or exceeds its bound")]
    Protocol,
    #[error("proxy exchange was interrupted; reconnect to the same request")]
    Interrupted,
    #[error("proxy session transport failed; recover the same request")]
    Transport(#[source] mayhem_bridge::BridgeError),
    #[error("proxy request execution: {0}")]
    Execution(#[from] crate::execution::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Buyer,
    Provider,
}
impl Role {
    fn opposite(self) -> Self {
        match self {
            Self::Buyer => Self::Provider,
            Self::Provider => Self::Buyer,
        }
    }
}

/// Deliberately not serializable/debuggable. Contains no upstream credential.
#[derive(Clone)]
pub struct Session {
    authorization: ProxySpendAuthorization,
    accepted_terms: Digest,
    remote: Digest,
    role: Role,
    invocation: Digest,
}
impl Session {
    pub fn new(
        authorization: ProxySpendAuthorization,
        local: &Identity,
        role: Role,
    ) -> Result<Self> {
        authorization
            .verify(crate::receipts::verify_signature)
            .map_err(|_| Error::Identity)?;
        let t = &authorization.terms;
        let (own, remote) = match role {
            Role::Buyer => (&t.buyer_pubkey, &t.offer.provider_pubkey),
            Role::Provider => (&t.offer.provider_pubkey, &t.buyer_pubkey),
        };
        if local.network_id != t.network_id
            || local.msb_bootstrap.as_str() != t.msb_bootstrap
            || local.subnet_bootstrap.as_str() != t.subnet_bootstrap
            || local.controller_pubkey.as_str() != own
        {
            return Err(Error::Identity);
        }
        let remote = Digest::new(remote).map_err(|_| Error::Identity)?;
        let accepted_terms =
            Digest::new(t.digest().map_err(|_| Error::Identity)?).map_err(|_| Error::Identity)?;
        let invocation = invocation(&authorization)?;
        Ok(Self {
            authorization,
            accepted_terms,
            remote,
            role,
            invocation,
        })
    }
    pub fn invocation(&self) -> &Digest {
        &self.invocation
    }
    pub fn authorization(&self) -> &ProxySpendAuthorization {
        &self.authorization
    }

    /// Reconstruct buyer evidence solely from delivered normalized output and
    /// the buyer's original request/contract. Never forward the upstream job ID
    /// or its self-reported counters from the provider's private result journal.
    pub fn decode_result(
        &self,
        received: Received,
        snapshot: &impl crate::buyer::Evidence,
        own_request: &[u8],
    ) -> Result<crate::endpoint::ProtocolReply> {
        if self.role != Role::Buyer
            || received.recipient != self.role
            || received.accepted_terms != self.accepted_terms
        {
            return Err(Error::Identity);
        }
        let Message::Result { response } = received.message else {
            return Err(Error::Protocol);
        };
        let binding = crate::financial::terms_binding(&self.authorization.terms)
            .map_err(|_| Error::Identity)?;
        let prepared = snapshot
            .verify_request(&binding, own_request)
            .map_err(|_| Error::Identity)?;
        let id = format!("proxy_{}", self.invocation.as_str());
        if response["id"].as_str() != Some(id.as_str()) {
            return Err(Error::Identity);
        }
        let created_key = if binding.endpoint == mayhem_proto::proxy::ProxyEndpoint::Responses {
            "created_at"
        } else {
            "created"
        };
        let created = response[created_key].as_u64().ok_or(Error::Protocol)?;
        let mut reply = prepared
            .decode_json(response.clone(), &id, created)
            .map_err(|_| Error::Protocol)?;
        if reply.body != response {
            return Err(Error::Protocol);
        }
        reply.upstream_id = None;
        reply.reported_usage = None;
        Ok(reply)
    }

    fn command(&self, received: Received) -> Result<Message> {
        if self.role != Role::Provider
            || received.recipient != self.role
            || received.accepted_terms != self.accepted_terms
        {
            return Err(Error::Identity);
        }
        Ok(received.message)
    }
    fn execute(&self, received: Received, streaming: bool) -> Result<Vec<u8>> {
        match self.command(received)? {
            Message::Execute {
                request,
                streaming: value,
            } if value == streaming => serde_json::to_vec(&request).map_err(|_| Error::Protocol),
            _ => Err(Error::Protocol),
        }
    }
    async fn existing(&self, executor: &PaidExecutor) -> Result<Option<crate::attempts::Recovery>> {
        let saved = executor.recover_current(&self.invocation).await?;
        if let Some(saved) = &saved {
            let matches = if let Some(financial) = &saved.financial {
                financial.accepted().authorization == self.authorization
            } else {
                // Negotiation saves a Prepared attempt before the buyer can
                // publish its hold. Only that exact durable countersignature
                // can bridge into paid admission; no unsigned draft may do so.
                saved.record.phase == crate::attempts::Phase::Prepared
                    && saved.acceptance.is_some()
                    && saved.request.is_some()
                    && executor
                        .matches_provider_acceptance(
                            &self.invocation,
                            saved.record.attempt,
                            &self.authorization,
                        )
                        .await?
            };
            if !matches {
                return Err(Error::Identity);
            }
        }
        Ok(saved)
    }
    async fn replay(
        &self,
        executor: &PaidExecutor,
        bytes: &[u8],
        streaming: bool,
    ) -> Result<Option<UnsettledReply>> {
        let Some(saved) = self.existing(executor).await? else {
            return Ok(None);
        };
        let original = saved.acceptance.as_ref().ok_or(Error::Protocol)?;
        let adapter = crate::endpoint::Adapter::restore(original.snapshot.adapter.clone())
            .map_err(|_| Error::Protocol)?;
        let request = if streaming {
            adapter.prepare_stream(bytes)
        } else {
            adapter.prepare_json(bytes)
        }
        .map_err(|_| Error::Protocol)?;
        if !request.matches_binding(&saved.record.binding) {
            return Err(Error::Identity);
        }
        Ok(saved.result.map(|result| UnsettledReply {
            attempt: saved.record,
            reply: result.reply,
            result_digest: result.digest,
        }))
    }
    pub async fn status(
        &self,
        received: Received,
        executor: &PaidExecutor,
    ) -> Result<(PublicState, Option<Value>)> {
        if !matches!(self.command(received)?, Message::Status) {
            return Err(Error::Protocol);
        }
        let saved = self.existing(executor).await?.ok_or(Error::Protocol)?;
        use crate::attempts::Phase;
        let state = match saved.record.phase {
            Phase::Closed => PublicState::Settled,
            _ if saved.record.cancellation_requested => PublicState::CancelRequested,
            Phase::Prepared => PublicState::Prepared,
            Phase::Resolved | Phase::Dispatched if saved.result.is_some() => {
                PublicState::AwaitingReceipt
            }
            Phase::Resolved | Phase::Dispatched => PublicState::OutcomeUnknown,
        };
        Ok((state, saved.result.map(|r| r.reply.body)))
    }
    pub async fn cancel(
        &self,
        received: Received,
        executor: &PaidExecutor,
        cancel: &Cancellation,
    ) -> Result<()> {
        if !matches!(self.command(received)?, Message::Cancel) {
            return Err(Error::Protocol);
        }
        self.existing(executor).await?.ok_or(Error::Protocol)?;
        Ok(executor.request_cancel(&self.invocation, cancel).await?)
    }
    /// The paid executor still checks fresh canonical finance, original payload,
    /// active shared capacity and the durable pre-POST fence. Replays cannot infer twice.
    pub async fn execute_json(
        &self,
        received: Received,
        executor: &PaidExecutor,
        cancel: &Cancellation,
    ) -> Result<UnsettledReply> {
        let bytes = self.execute(received, false)?;
        if let Some(result) = self.replay(executor, &bytes, false).await? {
            return Ok(result);
        }
        executor
            .prepare_accepted(self.invocation.clone(), &self.authorization, &bytes, false)
            .await?;
        Ok(executor
            .execute_json(&self.invocation, &bytes, cancel)
            .await?)
    }
    pub async fn execute_stream<F, Fut>(
        &self,
        received: Received,
        executor: &PaidExecutor,
        cancel: &Cancellation,
        emit: F,
    ) -> Result<UnsettledReply>
    where
        F: FnMut(Value) -> Fut,
        Fut: Future<Output = std::result::Result<(), ()>>,
    {
        let bytes = self.execute(received, true)?;
        if self.replay(executor, &bytes, true).await?.is_some() {
            // Recover the complete original result through Status. Never splice
            // a replay onto an already partially delivered stream.
            return Err(crate::execution::Error::ExistingResult.into());
        }
        executor
            .prepare_accepted(self.invocation.clone(), &self.authorization, &bytes, true)
            .await?;
        Ok(executor
            .execute_stream(&self.invocation, &bytes, cancel, emit)
            .await?)
    }
    /// Independent verification has already produced a durable buyer signature.
    /// The provider must retain and canonically confirm this exact outcome.
    pub async fn acknowledge(
        &self,
        received: Received,
        executor: &PaidExecutor,
        attempt: u64,
    ) -> Result<bool> {
        let Message::Acknowledge { acknowledgment } = self.command(received)? else {
            return Err(Error::Protocol);
        };
        match acknowledgment {
            SignedAcknowledgment::Receipt { receipt } => {
                executor
                    .retain_terminal_receipt(&self.invocation, attempt, &receipt)
                    .await?;
                Ok(executor
                    .publish_terminal_receipt(&self.invocation, attempt)
                    .await?)
            }
            SignedAcknowledgment::Waiver { closure } => {
                executor
                    .retain_waiver(&self.invocation, attempt, &closure)
                    .await?;
                Ok(executor.publish_waiver(&self.invocation, attempt).await?)
            }
        }
    }
}

/// Deterministically scoped to buyer/network/logical attempt, never prompt text
/// or a caller-chosen provider-local ID. Changing terms cannot bypass that journal.
pub fn invocation(auth: &ProxySpendAuthorization) -> Result<Digest> {
    invocation_for_terms(&auth.terms)
}

/// Logical key before countersigning; validates shape, never authenticates a
/// buyer or proves admission. Use only after verifying the buyer's signature.
pub fn invocation_for_terms(t: &mayhem_proto::proxy::finance::ProxySpendTerms) -> Result<Digest> {
    t.validate().map_err(|_| Error::Identity)?;
    Ok(Digest::hash(
        "mayhem/proxy/buyer-invocation/v1",
        &[
            t.network_id.as_bytes(),
            t.msb_bootstrap.as_bytes(),
            t.subnet_bootstrap.as_bytes(),
            t.buyer_pubkey.as_bytes(),
            t.billing_id.as_bytes(),
            &t.billing_attempt.to_le_bytes(),
        ],
    ))
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Message {
    Execute {
        request: Value,
        streaming: bool,
    },
    Status,
    Cancel,
    Acknowledge {
        acknowledgment: SignedAcknowledgment,
    },
    Result {
        response: Value,
    },
    Stream {
        event: Value,
    },
    Receipt {
        value: ProviderReceipt,
    },
    Waiver {
        value: ProviderWaiver,
    },
    State {
        state: PublicState,
    },
    Failure {
        failure: crate::attempts::FailureSnapshot,
    },
}
impl Message {
    fn validate(&self) -> Result<()> {
        if let Self::Failure { failure } = self {
            failure.to_failure().map_err(|_| Error::Protocol)?;
        }
        Ok(())
    }
    fn sender(&self) -> Role {
        match self {
            Self::Execute { .. } | Self::Status | Self::Cancel | Self::Acknowledge { .. } => {
                Role::Buyer
            }
            _ => Role::Provider,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicState {
    Prepared,
    Running,
    OutcomeUnknown,
    AwaitingReceipt,
    Settled,
    CancelRequested,
}

/// Constructible only after the bound channel verifies the transport envelope,
/// direction, exact terms, complete bytes and message framing. No Deserialize.
pub struct Received {
    accepted_terms: Digest,
    recipient: Role,
    message: Message,
}
impl Received {
    pub fn message(&self) -> &Message {
        &self.message
    }
}
