//! Durable buyer acknowledgments, separate from mere provider assertions.
use super::*;
use crate::{
    buyer::Evidence,
    endpoint::ProtocolReply,
    receipts::{self, BuyerApproval},
    signing::{Authority, ProviderReceipt, ProviderWaiver},
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Intent {
    Receipt { value: ProviderReceipt },
    Waiver { value: ProviderWaiver },
}
impl Intent {
    fn bytes(&self) -> Result<Vec<u8>> {
        match self {
            Self::Receipt { value } => value.draft.body.buyer_signing_bytes(),
            Self::Waiver { value } => value.draft.body.buyer_signing_bytes(),
        }
        .map_err(|_| invalid("invalid buyer acknowledgment"))
    }
    fn validate(
        &self,
        auth: &ProxySpendAuthorization,
        policy: &ProxySettlementPolicy,
    ) -> Result<()> {
        let t = &auth.terms;
        let (sig, bytes, terms, attempt) = match self {
            Self::Receipt { value } => {
                value
                    .draft
                    .body
                    .validate_for(t, policy, value.draft.previous.as_ref())
                    .map_err(|_| invalid("saved acknowledgment violates original policy"))?;
                require(
                    value.draft.body.final_receipt,
                    "buyer acknowledgment must be terminal",
                )?;
                (
                    &value.provider_sig,
                    value.draft.body.provider_signing_bytes(),
                    &value.draft.body.accepted_terms,
                    value.draft.attempt,
                )
            }
            Self::Waiver { value } => (
                &value.provider_sig,
                value.draft.body.provider_signing_bytes(),
                &value.draft.body.accepted_terms,
                value.draft.attempt,
            ),
        };
        require(
            attempt > 0
                && terms == &t.digest().map_err(|_| invalid("invalid accepted terms"))?
                && receipts::verify_signature(
                    sig,
                    &bytes.map_err(|_| invalid("invalid provider acknowledgment"))?,
                    &t.offer.provider_pubkey,
                ),
            "saved provider signature or terms differ",
        )
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Acknowledgment {
    intent: Intent,
    pub(super) buyer_sig: Option<String>,
}
impl Acknowledgment {
    pub(super) fn validate(
        &self,
        auth: &ProxySpendAuthorization,
        policy: &ProxySettlementPolicy,
    ) -> Result<()> {
        self.intent.validate(auth, policy)?;
        if let Some(sig) = &self.buyer_sig {
            require(
                receipts::verify_signature(sig, &self.intent.bytes()?, &auth.terms.buyer_pubkey),
                "saved buyer acknowledgment signature rejected",
            )?;
        }
        Ok(())
    }
    fn signed(&self) -> Result<SignedAcknowledgment> {
        let buyer_sig = self
            .buyer_sig
            .clone()
            .ok_or_else(|| invalid("buyer signature missing"))?;
        Ok(match &self.intent {
            Intent::Receipt { value } => SignedAcknowledgment::Receipt {
                receipt: ProxyUsageReceipt {
                    body: value.draft.body.clone(),
                    provider_sig: value.provider_sig.clone(),
                    buyer_sig,
                },
            },
            Intent::Waiver { value } => SignedAcknowledgment::Waiver {
                closure: ProxyReservationClosure {
                    body: value.draft.body.clone(),
                    provider_sig: value.provider_sig.clone(),
                    buyer_sig,
                },
            },
        })
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SignedAcknowledgment {
    Receipt { receipt: ProxyUsageReceipt },
    Waiver { closure: ProxyReservationClosure },
}
impl Store {
    fn approve(
        &self,
        o: &Observation,
        intent: Intent,
        approval: &BuyerApproval,
        at_ms: u64,
    ) -> Result<()> {
        require(
            approval.identity == self.identity && intent.bytes()? == approval.signing_bytes(),
            "buyer approval does not bind this intent",
        )?;
        self.observe(o, false, at_ms)?;
        let key = &o.accepted().accepted_terms;
        let tx = self.transaction()?;
        o.buyer_binding(&self.identity)?;
        let mut table = crate::db(tx.open_table(RECORDS))?;
        let mut r = self.decode(
            key,
            crate::db(table.get(key.as_str()))?
                .ok_or_else(|| invalid("recovery record not found"))?
                .value(),
        )?;
        if let Some(saved) = &r.acknowledgment {
            return require(
                saved.intent == intent,
                "buyer already approved another outcome",
            );
        }
        require(r.confirmed.is_none(), "financial outcome already resolved")?;
        r.acknowledgment = Some(Acknowledgment {
            intent,
            buyer_sig: None,
        });
        self.save(&mut table, key, &r)?;
        drop(table);
        self.commit(tx)
    }
    fn retain_acknowledgment(
        &self,
        key: &str,
        intent: &Intent,
        signature: String,
    ) -> Result<SignedAcknowledgment> {
        let tx = self.transaction()?;
        let mut table = crate::db(tx.open_table(RECORDS))?;
        let mut r = self.decode(
            key,
            crate::db(table.get(key))?
                .ok_or_else(|| invalid("recovery record not found"))?
                .value(),
        )?;
        let a = r
            .acknowledgment
            .as_mut()
            .ok_or_else(|| invalid("buyer approval missing"))?;
        require(&a.intent == intent, "buyer outcome intent changed")?;
        if let Some(saved) = &a.buyer_sig {
            require(saved == &signature, "buyer signature changed")?;
            return a.signed();
        }
        require(r.confirmed.is_none(), "financial outcome already resolved")?;
        a.buyer_sig = Some(signature);
        let result = a.signed()?;
        self.save(&mut table, key, &r)?;
        drop(table);
        self.commit(tx)?;
        Ok(result)
    }
}
impl BuyerRecovery {
    /// Already signed evidence remains deliverable while the wallet is locked.
    /// This does not imply that the ledger has applied it.
    pub async fn acknowledgment(&self, key: Digest) -> Result<Option<SignedAcknowledgment>> {
        self.run(move |s| {
            let r = s.get(key.as_str())?;
            r.acknowledgment
                .filter(|a| a.buyer_sig.is_some())
                .map(|a| a.signed())
                .transpose()
        })
        .await
    }
    /// The parent must supply its OWN retained normalized request, model contract
    /// and actually received output. Never use evidence supplied by the provider
    /// in place of the buyer's observation. Acks cannot grant another inference.
    pub async fn approve_receipt(
        &self,
        verifier: &crate::worker::host::Pool,
        value: ProviderReceipt,
        snapshot: impl Evidence,
        own_request: Vec<u8>,
        received: ProtocolReply,
        cancelled_before_terminal: bool,
        at_ms: u64,
    ) -> Result<()> {
        let key = Digest::new(&value.draft.body.accepted_terms)
            .map_err(|_| invalid("invalid terms key"))?;
        let saved = self.run(move |s| s.get(key.as_str())).await?;
        let o = self.client.observe(&saved.authorization).await?;
        let previous = o.receipt_head()?.map(|r| r.body);
        require(
            value.draft.previous == previous,
            "provider draft differs from buyer canonical receipt head",
        )?;
        let approval = receipts::approve_terminal(
            verifier,
            &value.draft,
            &value.provider_sig,
            &saved.authorization,
            &saved.policy,
            &snapshot,
            &own_request,
            &received,
            cancelled_before_terminal,
        )
        .await?;
        self.run(move |s| s.approve(&o, Intent::Receipt { value }, &approval, at_ms))
            .await
    }
    pub async fn approve_waiver(
        &self,
        value: ProviderWaiver,
        own_request: Vec<u8>,
        received: Option<ProtocolReply>,
        cancelled_before_terminal: bool,
        at_ms: u64,
    ) -> Result<()> {
        let key = Digest::new(&value.draft.body.accepted_terms)
            .map_err(|_| invalid("invalid terms key"))?;
        let saved = self.run(move |s| s.get(key.as_str())).await?;
        let o = self.client.observe(&saved.authorization).await?;
        require(
            value.draft.body.outcome
                != mayhem_proto::proxy::finance::ProxyClosureOutcome::NotExecuted
                || o.receipt_head()?.is_none(),
            "canonical checkpoint contradicts non-execution",
        )?;
        self.run(move |s| {
            let approval = receipts::approve_waiver(
                &value.draft,
                &value.provider_sig,
                &saved.authorization,
                &own_request,
                received.as_ref(),
                cancelled_before_terminal,
            )?;
            s.approve(&o, Intent::Waiver { value }, &approval, at_ms)
        })
        .await
    }
    /// Return only after the buyer signature has been durably saved. Repeating
    /// this after restart yields the same envelope, never a fresh outcome.
    pub async fn sign_approved(
        &self,
        signer: &Authority,
        key: Digest,
    ) -> Result<SignedAcknowledgment> {
        let k = key.clone();
        let r = self.run(move |s| s.get(k.as_str())).await?;
        let saved = r
            .acknowledgment
            .ok_or_else(|| invalid("buyer approval missing"))?;
        let approval = BuyerApproval::retained(&r.authorization, saved.intent.bytes()?)?;
        let signature = signer.sign_buyer_approval(&approval)?;
        self.run(move |s| s.retain_acknowledgment(key.as_str(), &saved.intent, signature))
            .await
    }
}
