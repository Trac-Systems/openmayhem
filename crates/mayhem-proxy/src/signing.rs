//! Role- and network-bound signatures in the trusted Core parent. Construct from
//! the existing protected wallet's native Ed25519 key after unlock. No RPC, bytes
//! signing command, key export, environment lookup or subprocess exists here.
use crate::{
    attempts::{Identity, TerminalDraft, WaiverDraft},
    financial::Accepted,
    invalid,
    receipts::BuyerApproval,
    require, Result,
};
use ed25519_dalek::{Signer, SigningKey};
use mayhem_proto::proxy::finance::{
    ProxyExpiryBody, ProxyReservationExpiry, ProxySettlementPolicy, ProxySpendAuthorization,
    ProxySpendTerms,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderReceipt {
    pub draft: TerminalDraft,
    pub provider_sig: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderWaiver {
    pub draft: WaiverDraft,
    pub provider_sig: String,
}

/// Deliberately not Clone, Debug or serializable. Only the parent owns this key;
/// connector workers receive neither it nor access to these approval objects.
pub struct Authority {
    key: SigningKey,
    identity: Identity,
}
impl Authority {
    pub fn from_unlocked_wallet(key: SigningKey, identity: Identity) -> Result<Self> {
        identity
            .validate()
            .map_err(|_| invalid("invalid signing identity"))?;
        require(
            hex(&key.verifying_key().to_bytes()) == identity.controller_pubkey.as_str(),
            "unlocked wallet does not match proxy signing identity",
        )?;
        Ok(Self { key, identity })
    }
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    /// Trusted negotiation store persists this exact signature before exposure.
    /// There is intentionally no public arbitrary spend-signing API.
    pub(crate) fn buyer_spend(
        &self,
        purchase: &crate::financial::quote::PreparedPurchase,
    ) -> Result<String> {
        self.party(purchase.terms(), true)?;
        let bytes = purchase
            .terms()
            .buyer_signing_bytes()
            .map_err(|_| invalid("invalid buyer purchase"))?;
        Ok(hex(&self.key.sign(&bytes).to_bytes()))
    }
    fn terms(&self, authorization: &ProxySpendAuthorization, buyer: bool) -> Result<()> {
        authorization
            .verify(crate::receipts::verify_signature)
            .map_err(|_| invalid("accepted signatures rejected"))?;
        self.party(&authorization.terms, buyer)
    }
    fn party(&self, t: &ProxySpendTerms, buyer: bool) -> Result<()> {
        require(
            self.identity.network_id == t.network_id
                && self.identity.msb_bootstrap.as_str() == t.msb_bootstrap
                && self.identity.subnet_bootstrap.as_str() == t.subnet_bootstrap
                && self.identity.controller_pubkey.as_str()
                    == if buyer {
                        &t.buyer_pubkey
                    } else {
                        &t.offer.provider_pubkey
                    },
            "proxy signature network or role differs",
        )
    }
    /// Accepts ONLY a non-deserializable approval produced by independent buyer
    /// verification. An arbitrary tool, upstream response or HTTP body is not one.
    pub(crate) fn sign_buyer_approval(&self, approval: &BuyerApproval) -> Result<String> {
        require(
            self.identity == approval.identity,
            "buyer approval belongs to another wallet or network",
        )?;
        Ok(hex(&self.key.sign(approval.signing_bytes()).to_bytes()))
    }
    pub(crate) fn provider_receipt(
        &self,
        draft: TerminalDraft,
        accepted: &Accepted,
    ) -> Result<ProviderReceipt> {
        self.terms(&accepted.authorization, false)?;
        draft
            .body
            .validate_for(
                &accepted.authorization.terms,
                &accepted.settlement_policy,
                draft.previous.as_ref(),
            )
            .map_err(|_| invalid("provider receipt draft differs from original policy"))?;
        require(
            draft.body.final_receipt,
            "provider terminal receipt must be final",
        )?;
        let bytes = draft
            .body
            .provider_signing_bytes()
            .map_err(|_| invalid("invalid provider receipt"))?;
        Ok(ProviderReceipt {
            draft,
            provider_sig: hex(&self.key.sign(&bytes).to_bytes()),
        })
    }
    pub(crate) fn provider_waiver(
        &self,
        draft: WaiverDraft,
        accepted: &Accepted,
    ) -> Result<ProviderWaiver> {
        self.terms(&accepted.authorization, false)?;
        draft
            .body
            .validate()
            .map_err(|_| invalid("invalid provider waiver"))?;
        require(
            draft.body.accepted_terms == accepted.accepted_terms,
            "provider waiver terms differ",
        )?;
        let bytes = draft
            .body
            .provider_signing_bytes()
            .map_err(|_| invalid("invalid provider waiver"))?;
        Ok(ProviderWaiver {
            draft,
            provider_sig: hex(&self.key.sign(&bytes).to_bytes()),
        })
    }
    pub(crate) fn buyer_expiry(
        &self,
        authorization: &ProxySpendAuthorization,
        policy: &ProxySettlementPolicy,
        body: ProxyExpiryBody,
    ) -> Result<ProxyReservationExpiry> {
        self.terms(authorization, true)?;
        let bytes = body
            .buyer_signing_bytes()
            .map_err(|_| invalid("invalid buyer expiry"))?;
        let expiry = ProxyReservationExpiry {
            body,
            buyer_sig: hex(&self.key.sign(&bytes).to_bytes()),
        };
        expiry
            .verify(
                &authorization.terms,
                policy,
                expiry.body.observed_epoch,
                crate::receipts::verify_signature,
            )
            .map_err(|_| invalid("saved expiry is not authorized by original policy"))?;
        Ok(expiry)
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attempts::Digest;
    #[test]
    fn wrong_wallet_cannot_create_an_authority_and_identity_has_no_release_pin() {
        let key = SigningKey::from_bytes(&[1; 32]);
        let mut identity = Identity {
            network_id: "918".into(),
            msb_bootstrap: Digest::new("0".repeat(64)).unwrap(),
            subnet_bootstrap: Digest::new("1".repeat(64)).unwrap(),
            controller_pubkey: Digest::new(hex(&key.verifying_key().to_bytes())).unwrap(),
        };
        assert!(Authority::from_unlocked_wallet(
            SigningKey::from_bytes(&[2; 32]),
            identity.clone()
        )
        .is_err());
        let authority = Authority::from_unlocked_wallet(key, identity.clone()).unwrap();
        assert_eq!(authority.identity(), &identity);
        identity.network_id.clear();
        assert!(
            Authority::from_unlocked_wallet(SigningKey::from_bytes(&[1; 32]), identity).is_err()
        );
    }
}
