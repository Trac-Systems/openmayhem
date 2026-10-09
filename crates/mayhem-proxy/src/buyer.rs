//! Public contract verification and backwards-compatible private buyer recovery.
//! None of these types authorizes provider dispatch or trusts reported usage.
use crate::{
    attempts::{AcceptanceSnapshot, Binding},
    endpoint::{Adapter, PublicAdapter, PublicAdapterSnapshot, PublicRequest},
    invalid, require, Result,
};
use mayhem_proto::proxy::ProxyOffer;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicAcceptanceSnapshot {
    pub version: u32,
    pub adapter: PublicAdapterSnapshot,
    pub offer: ProxyOffer,
}
impl PublicAcceptanceSnapshot {
    pub fn validate_for(&self, binding: &Binding) -> Result<()> {
        require(self.version == 1, "invalid buyer snapshot version")?;
        let adapter = PublicAdapter::restore(self.adapter.clone())
            .map_err(|_| invalid("invalid buyer adapter"))?;
        require(
            adapter.endpoint() == binding.endpoint
                && adapter.contract_hash() == &binding.endpoint_contract
                && adapter.recipe_hash() == &binding.recipe_digest,
            "buyer adapter binding differs",
        )?;
        crate::attempts::validate_offer_binding(&self.offer, binding)
            .map_err(|_| invalid("buyer offer binding differs"))
    }
}

/// Untagged only to preserve the exact JSON and commitment of legacy private
/// intents. New purchases always use Public. Strict fields distinguish variants;
/// recovery never rewrites an old signed intent merely to change its format.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Snapshot {
    Public(PublicAcceptanceSnapshot),
    Legacy(AcceptanceSnapshot),
}
impl Snapshot {
    pub fn validate_for(&self, binding: &Binding) -> Result<()> {
        self.public_snapshot()?.validate_for(binding)
    }
}
mod sealed {
    pub trait Sealed {}
    impl Sealed for super::PublicAcceptanceSnapshot {}
    impl Sealed for super::AcceptanceSnapshot {}
    impl Sealed for super::Snapshot {}
}
/// Sealed: a caller cannot substitute its own verifier or arbitrary decoder.
pub trait Evidence: sealed::Sealed {
    fn public_snapshot(&self) -> Result<PublicAcceptanceSnapshot>;
    fn verify_request(&self, binding: &Binding, request: &[u8]) -> Result<PublicRequest> {
        let snapshot = self.public_snapshot()?;
        snapshot.validate_for(binding)?;
        let adapter = PublicAdapter::restore(snapshot.adapter)
            .map_err(|_| invalid("invalid buyer adapter"))?;
        require(
            request.len() <= adapter.limits().request_bytes,
            "buyer request exceeds bound",
        )?;
        let value: serde_json::Value = serde_json::from_slice(request)?;
        let prepared = if value.get("stream") == Some(&serde_json::Value::Bool(true)) {
            adapter.prepare_stream(request)
        } else {
            adapter.prepare_json(request)
        }
        .map_err(|_| invalid("buyer request violates its original contract"))?;
        require(
            prepared.matches_binding(binding),
            "buyer request binding differs",
        )?;
        Ok(prepared)
    }
}
impl Evidence for PublicAcceptanceSnapshot {
    fn public_snapshot(&self) -> Result<PublicAcceptanceSnapshot> {
        Ok(self.clone())
    }
}
impl Evidence for AcceptanceSnapshot {
    fn public_snapshot(&self) -> Result<PublicAcceptanceSnapshot> {
        let adapter = Adapter::restore(self.adapter.clone())
            .map_err(|_| invalid("invalid legacy buyer adapter"))?;
        Ok(PublicAcceptanceSnapshot {
            version: 1,
            adapter: adapter.public_snapshot(),
            offer: self.offer.clone(),
        })
    }
}
impl Evidence for Snapshot {
    fn public_snapshot(&self) -> Result<PublicAcceptanceSnapshot> {
        match self {
            Self::Public(v) => v.public_snapshot(),
            Self::Legacy(v) => v.public_snapshot(),
        }
    }
}
