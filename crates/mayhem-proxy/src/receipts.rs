//! Receipt verification at the trusted controller boundary. No signing key or
//! upstream-reported price is accepted from a connector. A buyer must supply its
//! own retained request and received result, not echo a provider's usage claim.
use crate::{
    attempts::{self, AcceptanceSnapshot, TerminalDraft},
    endpoint::ProtocolReply,
    financial, invalid, metering, require, Result,
};
use ed25519_dalek::{Signature, VerifyingKey};
use mayhem_proto::proxy::finance::{
    ProxySettlementPolicy, ProxySpendAuthorization, ProxyUsageReceipt,
};

fn decode<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2
        || !value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
    {
        return None;
    }
    let mut out = [0; N];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}
pub fn verify_signature(signature: &str, bytes: &[u8], public_key: &str) -> bool {
    let Some(key) = decode(public_key).and_then(|v| VerifyingKey::from_bytes(&v).ok()) else {
        return false;
    };
    let Some(sig) = decode(signature).map(|v| Signature::from_bytes(&v)) else {
        return false;
    };
    key.verify_strict(bytes, &sig).is_ok()
}

/// Non-deserializable result of independent verification. The owning buyer may
/// sign these exact domain-separated bytes with its existing protected wallet.
/// Signing is deliberately separate from accepting arbitrary public RPC bytes.
pub struct BuyerApproval {
    signing_bytes: Vec<u8>,
    pub(crate) identity: attempts::Identity,
}
fn buyer_identity(t: &mayhem_proto::proxy::finance::ProxySpendTerms) -> Result<attempts::Identity> {
    let digest =
        |v: &str| attempts::Digest::new(v).map_err(|_| invalid("invalid signing identity"));
    Ok(attempts::Identity {
        network_id: t.network_id.clone(),
        msb_bootstrap: digest(&t.msb_bootstrap)?,
        subnet_bootstrap: digest(&t.subnet_bootstrap)?,
        controller_pubkey: digest(&t.buyer_pubkey)?,
    })
}
/// Explicit buyer consent to a zero-charge closure. For terminal output, verify
/// the buyer's own received evidence. For unsent cancellation, verify the signed
/// provider assertion and absence of locally received output. That assertion is
/// not independent proof of remote non-execution; capacity/retry authorities still
/// require their own journal/upstream evidence and must not trust this object alone.
pub fn approve_waiver(
    draft: &attempts::WaiverDraft,
    provider_signature: &str,
    authorization: &ProxySpendAuthorization,
    own_request: &[u8],
    received: Option<&ProtocolReply>,
    cancellation_accepted_before_terminal: bool,
) -> Result<BuyerApproval> {
    use mayhem_proto::proxy::finance::ProxyClosureOutcome as Outcome;
    authorization
        .verify(verify_signature)
        .map_err(|_| invalid("accepted signatures rejected"))?;
    let t = &authorization.terms;
    let body = &draft.body;
    body.validate().map_err(|_| invalid("invalid waiver"))?;
    require(
        draft.attempt > 0
            && body.accepted_terms == t.digest().map_err(|_| invalid("invalid accepted terms"))?
            && verify_signature(
                provider_signature,
                &body
                    .provider_signing_bytes()
                    .map_err(|_| invalid("invalid waiver"))?,
                &t.offer.provider_pubkey,
            ),
        "provider waiver differs",
    )?;
    let request = serde_json::from_slice(own_request)?;
    require(
        mayhem_proto::endpoint_request_fingerprint(&request) == t.request_hash,
        "buyer request differs",
    )?;
    let evidence = if body.outcome == Outcome::NotExecuted {
        require(
            received.is_none() && cancellation_accepted_before_terminal,
            "unsent waiver contradicts buyer evidence",
        )?;
        attempts::unsent_commitment(&draft.invocation, draft.attempt)
    } else {
        let received = received.ok_or_else(|| invalid("buyer terminal result is missing"))?;
        let disposition = received
            .observed_usage
            .as_ref()
            .ok_or_else(|| invalid("buyer result is unverified"))?
            .disposition;
        let outcome = if cancellation_accepted_before_terminal {
            Outcome::Cancelled
        } else if disposition == metering::Disposition::Incomplete {
            Outcome::Failed
        } else {
            Outcome::CompletedUnbilled
        };
        require(body.outcome == outcome, "buyer terminal outcome differs")?;
        draft
            .result_commitment
            .compute(
                &draft.invocation,
                draft.attempt,
                &financial::terms_binding(t)?,
                received,
            )
            .map_err(|_| invalid("buyer result commitment differs"))?
    };
    require(
        body.evidence_hash == evidence.as_str(),
        "waiver evidence differs",
    )?;
    Ok(BuyerApproval {
        identity: buyer_identity(t)?,
        signing_bytes: body
            .buyer_signing_bytes()
            .map_err(|_| invalid("invalid waiver"))?,
    })
}
impl BuyerApproval {
    /// Only trusted private recovery code may reconstruct an approval whose
    /// exact typed intent was already independently verified and durably saved.
    pub(crate) fn retained(
        authorization: &ProxySpendAuthorization,
        bytes: Vec<u8>,
    ) -> Result<Self> {
        authorization
            .verify(verify_signature)
            .map_err(|_| invalid("saved authorization rejected"))?;
        Ok(Self {
            identity: buyer_identity(&authorization.terms)?,
            signing_bytes: bytes,
        })
    }
    pub fn signing_bytes(&self) -> &[u8] {
        &self.signing_bytes
    }
}

pub async fn approve_terminal(
    verifier: &crate::worker::host::Pool,
    draft: &TerminalDraft,
    provider_signature: &str,
    authorization: &ProxySpendAuthorization,
    policy: &ProxySettlementPolicy,
    snapshot: &AcceptanceSnapshot,
    own_request: &[u8],
    received: &ProtocolReply,
    cancellation_accepted_before_terminal: bool,
) -> Result<BuyerApproval> {
    let t = &authorization.terms;
    authorization
        .verify(verify_signature)
        .map_err(|_| invalid("accepted signatures rejected"))?;
    draft
        .body
        .validate_for(t, policy, draft.previous.as_ref())
        .map_err(|_| invalid("receipt terms differ"))?;
    require(
        draft.attempt > 0
            && draft.body.final_receipt
            && draft.body.outcome != mayhem_proto::proxy::finance::ProxyReceiptOutcome::Running,
        "receipt is not a terminal result",
    )?;
    require(
        verify_signature(
            provider_signature,
            &draft
                .body
                .provider_signing_bytes()
                .map_err(|_| invalid("invalid receipt"))?,
            &t.offer.provider_pubkey,
        ),
        "provider receipt signature rejected",
    )?;
    let binding = financial::terms_binding(t)?;
    snapshot
        .validate_for(&binding)
        .map_err(|_| invalid("original execution snapshot differs"))?;
    let value = serde_json::from_slice(own_request)?;
    require(
        mayhem_proto::endpoint_request_fingerprint(&value) == t.request_hash,
        "buyer request differs",
    )?;
    // Independently enforce the buyer's original endpoint shape and binding.
    // Full schema/regex execution below stays in the isolated verifier worker.
    let adapter = crate::endpoint::Adapter::restore(snapshot.adapter.clone())
        .map_err(|_| invalid("buyer endpoint snapshot is invalid"))?;
    let prepared = if value.get("stream") == Some(&serde_json::Value::Bool(true)) {
        adapter.prepare_stream(own_request)
    } else {
        adapter.prepare_json(own_request)
    }
    .map_err(|_| invalid("buyer request violates its original contract"))?;
    require(
        prepared.matches_binding(&binding),
        "buyer endpoint binding differs",
    )?;
    let id = received.body["id"]
        .as_str()
        .ok_or_else(|| invalid("buyer result identity missing"))?;
    let created_key = if binding.endpoint == mayhem_proto::proxy::ProxyEndpoint::Responses {
        "created_at"
    } else {
        "created"
    };
    let created = received.body[created_key]
        .as_u64()
        .ok_or_else(|| invalid("buyer result time missing"))?;
    let checked = prepared
        .decode_json(received.body.clone(), id, created)
        .map_err(|_| invalid("buyer result violates its original endpoint contract"))?;
    require(
        checked.body == received.body,
        "buyer result is not the normalized output",
    )?;
    let observation = checked
        .observed_usage
        .ok_or_else(|| invalid("buyer result cannot be metered"))?;
    require(
        draft.body.outcome
            == metering::terminal_outcome(
                observation.disposition,
                cancellation_accepted_before_terminal,
            )
            && received.observed_usage.as_ref() == Some(&observation),
        "buyer result outcome differs",
    )?;
    let result_digest = draft
        .result_commitment
        .compute(&draft.invocation, draft.attempt, &binding, received)
        .map_err(|_| invalid("buyer result commitment failed"))?;
    let amount = t
        .offer
        .cost(&observation.units)
        .map_err(|_| invalid("buyer subtotal overflow"))?;
    let subtotal = metering::VerifiedSubtotal {
        invocation: draft.invocation.clone(),
        attempt: draft.attempt,
        binding,
        result_digest: result_digest.clone(),
        observation,
        subtotal_au: amount,
    };
    require(
        draft.body.result_hash == result_digest.as_str()
            && draft.body.observation_hash
                == subtotal
                    .digest()
                    .map_err(|_| invalid("buyer observation differs"))?
                    .as_str()
            && draft.body.usage == subtotal.observation.units
            && draft.body.au_owed_cum == amount
            && draft.body.billing_au_owed_cum
                == t.prior_spend_au
                    .checked_add(amount)
                    .ok_or_else(|| invalid("buyer total overflow"))?,
        "receipt differs from buyer-observed result",
    )?;
    verifier
        .verify_received(
            &draft.invocation,
            draft.attempt,
            &subtotal.binding,
            prepared.semantic_policy(),
            &received.body,
            adapter.limits().response_bytes,
        )
        .await
        .map_err(|_| invalid("buyer result failed isolated endpoint contract verification"))?;
    Ok(BuyerApproval {
        identity: buyer_identity(t)?,
        signing_bytes: draft
            .body
            .buyer_signing_bytes()
            .map_err(|_| invalid("invalid receipt"))?,
    })
}

pub fn verify_terminal(
    receipt: &ProxyUsageReceipt,
    draft: &TerminalDraft,
    authorization: &ProxySpendAuthorization,
    policy: &ProxySettlementPolicy,
) -> Result<()> {
    require(
        receipt.body == draft.body,
        "signed receipt differs from durable draft",
    )?;
    authorization
        .verify(verify_signature)
        .map_err(|_| invalid("accepted signatures rejected"))?;
    receipt
        .verify(
            &authorization.terms,
            policy,
            draft.previous.as_ref(),
            verify_signature,
        )
        .map_err(|_| invalid("receipt signatures rejected"))
}
