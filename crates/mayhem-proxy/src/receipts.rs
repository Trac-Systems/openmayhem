//! Receipt verification at the trusted controller boundary. No signing key or
//! upstream-reported price is accepted from a connector. A buyer must supply its
//! own retained request and received result, not echo a provider's usage claim.
use crate::{
    attempts::{self, AcceptanceSnapshot, CompletedDraft},
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
}
impl BuyerApproval {
    pub fn signing_bytes(&self) -> &[u8] {
        &self.signing_bytes
    }
}

pub fn approve_completed(
    draft: &CompletedDraft,
    provider_signature: &str,
    authorization: &ProxySpendAuthorization,
    policy: &ProxySettlementPolicy,
    snapshot: &AcceptanceSnapshot,
    own_request: &[u8],
    received: &ProtocolReply,
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
            && draft.body.outcome == mayhem_proto::proxy::finance::ProxyReceiptOutcome::Complete,
        "receipt is not a completed result",
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
    let prepared = metering::Policy::resolve(binding.endpoint, &binding.metering_policy)
        .and_then(|p| p.prepare(binding.endpoint, &value))
        .map_err(|_| invalid("buyer input cannot be metered"))?;
    let observation = prepared
        .observe(&received.body)
        .map_err(|_| invalid("buyer result cannot be metered"))?;
    require(
        observation.disposition == metering::Disposition::Complete
            && received.observed_usage.as_ref() == Some(&observation),
        "buyer result is not complete",
    )?;
    let result_digest = attempts::result_commitment(
        &draft.invocation,
        draft.attempt,
        &binding,
        &serde_json::to_vec(received)?,
    )
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
    Ok(BuyerApproval {
        signing_bytes: draft
            .body
            .buyer_signing_bytes()
            .map_err(|_| invalid("invalid receipt"))?,
    })
}

pub fn verify_completed(
    receipt: &ProxyUsageReceipt,
    draft: &CompletedDraft,
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
