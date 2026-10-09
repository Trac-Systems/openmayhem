//! Shared local provider setup. Structural validation is not conformance,
//! admission, publication, or serving authority. Only an explicit probe invokes
//! the existing bounded upstream controller; setup never opens a wallet.
mod admission;
mod connection;
mod probe;
mod profile;
mod review;
mod store;
pub use admission::{AdmissionEvidence, AdmissionReport, AdmissionState, CanonicalProvider};
pub use connection::{DiscoveryState, InventoryReview};
pub use probe::{ProbeGroup, ProbePlan, ProbeReport, ProbeScope, ProbeState};
pub use profile::{
    profiles, EndpointProfile, MembershipInput, OfferInput, ProfileInput, ProfileMarket,
    ProfileReview,
};
pub use review::{AdmissionHandoff, Review, State};
pub use store::Store;

use crate::{
    attempts::Digest,
    connector::config::{private_file, ConnectionConfig},
    discovery::Identity,
    endpoint::{Adapter, AdapterSnapshot},
    metering::Policy,
};
use mayhem_proto::proxy::{
    finance::ProxySettlementPolicy, ProxyAction, ProxyEndpointContract, ProxyLane,
    ProxyMarketDescriptor, ProxyMembership, ProxyOffer, ProxyOperation,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub const MAX_BYTES: usize = 512 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid provider setup declaration or binding")]
    Invalid,
    #[error("provider setup file protection or access rejected")]
    Protection,
    #[error("provider setup draft is missing")]
    Missing,
    #[error("provider setup revision changed or the draft already exists")]
    Conflict,
    #[error("provider setup is busy; retry the original operation")]
    Busy,
    #[error("provider setup storage is unavailable")]
    Storage,
    #[error("provider setup write outcome is uncertain; inspect the original draft")]
    CommitUnknown,
    #[error("provider setup connection changed; update and recheck the draft")]
    ConnectionChanged,
    #[error("provider setup probe requires recovery of the original retained attempt")]
    ProbeRecovery,
    #[error("provider setup probe capacity is unavailable or its allowance is exhausted")]
    ProbeCapacity,
    #[error("provider connection discovery state is missing")]
    DiscoveryMissing,
}
pub type Result<T> = std::result::Result<T, Error>;
fn require(ok: bool) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid)
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    CreateMarket,
    JoinMarket,
}

/// Private input, never returned by the public review. One connection and one
/// selected membership per draft; multiple explicit offer slots may share it.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub schema_version: u32,
    pub network: Identity,
    pub provider_pubkey: Digest,
    pub connection_file: PathBuf,
    pub adapter: AdapterSnapshot,
    pub market: ProxyMarketDescriptor,
    pub membership: ProxyMembership,
    pub offers: Vec<ProxyOffer>,
    pub selection: Selection,
    /// Declared next operation sequence; canonical publication must recheck it.
    pub sequence: u64,
    pub settlement_policy: ProxySettlementPolicy,
}
impl Input {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = private_file(path, MAX_BYTES).map_err(|_| Error::Protection)?;
        let mut input: Self = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
        if input.connection_file.is_relative() {
            let parent = std::fs::canonicalize(
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )
            .map_err(|_| Error::Protection)?;
            input.connection_file = parent.join(&input.connection_file);
        }
        input.validate()?;
        Ok(input)
    }
    pub fn validate(&self) -> Result<()> {
        require(
            self.schema_version == 1
                && self.connection_file.is_absolute()
                && !self.connection_file.as_os_str().is_empty()
                && (1..=16).contains(&self.offers.len())
                && serde_json::to_vec(self).map_err(|_| Error::Invalid)?.len() <= MAX_BYTES - 4096,
        )?;
        self.network.validate().map_err(|_| Error::Invalid)?;
        require(self.network.contract_version == mayhem_proto::CONTRACT_VERSION)?;
        let adapter = Adapter::restore(self.adapter.clone()).map_err(|_| Error::Invalid)?;
        self.market.validate().map_err(|_| Error::Invalid)?;
        self.membership
            .validate_for_market(&self.market)
            .map_err(|_| Error::Invalid)?;
        self.settlement_policy
            .validate()
            .map_err(|_| Error::Invalid)?;
        let expected = ProxyEndpointContract {
            endpoint: adapter.endpoint(),
            contract_hash: adapter.contract_hash().as_str().into(),
        };
        require(
            self.membership.provider_pubkey == self.provider_pubkey.as_str()
                && self.membership.endpoints == vec![expected]
                && self.membership.recipe_hash == adapter.recipe_hash().as_str()
                && self.market.metering == Policy::for_endpoint(adapter.endpoint()).contract(),
        )?;
        let mut slots = BTreeSet::new();
        for offer in &self.offers {
            offer
                .validate_for_membership(&self.market, &self.membership)
                .map_err(|_| Error::Invalid)?;
            require(
                offer.endpoint == adapter.endpoint()
                    && slots.insert(offer.slot_id().map_err(|_| Error::Invalid)?),
            )?;
        }
        self.operation().validate().map_err(|_| Error::Invalid)
    }
    fn operation(&self) -> ProxyOperation {
        ProxyOperation {
            schema_version: 1,
            lane: ProxyLane::Proxy,
            network_id: self.network.network_id.clone(),
            msb_bootstrap: self.network.msb_bootstrap.clone(),
            subnet_bootstrap: self.network.subnet_bootstrap.clone(),
            contract_version: self.network.contract_version,
            provider_pubkey: self.provider_pubkey.as_str().into(),
            sequence: self.sequence,
            action: match self.selection {
                Selection::CreateMarket => ProxyAction::CreateMarket {
                    market: self.market.clone(),
                    membership: self.membership.clone(),
                },
                Selection::JoinMarket => ProxyAction::JoinMarket {
                    membership: self.membership.clone(),
                },
            },
        }
    }
    fn connection(&self) -> Result<Digest> {
        let connection =
            ConnectionConfig::load(&self.connection_file).map_err(|_| Error::Protection)?;
        let adapter = Adapter::restore(self.adapter.clone()).map_err(|_| Error::Invalid)?;
        require(
            connection.revision == self.membership.connection_revision
                && connection.paths.contains_key(&adapter.operation()),
        )?;
        // fingerprint validates configuration but never resolves/reads secrets.
        connection.fingerprint().map_err(|_| Error::Invalid)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema_version: u32,
    id: Digest,
    revision: u64,
    input: Input,
    connection: Digest,
    checked: Option<Digest>,
    #[serde(default)]
    probe_scope: Option<ProbeScope>,
    #[serde(default)]
    probe: Option<probe::Attempt>,
    #[serde(default)]
    admission: Option<admission::Attempt>,
}
impl Record {
    fn binding(&self) -> Result<Digest> {
        let bytes = mayhem_proto::stable_json_bytes(
            &serde_json::json!({"input":self.input,"connection":self.connection}),
        )
        .map_err(|_| Error::Invalid)?;
        Ok(Digest::hash(
            "mayhem/proxy/setup-local-binding/v1",
            &[&bytes],
        ))
    }
    fn validate(&self) -> Result<()> {
        require(
            self.schema_version == 1
                && self.revision > 0
                && self.revision <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER,
        )?;
        self.input.validate()?;
        if let Some(admission) = &self.admission {
            admission.validate()?;
        }
        if let Some(scope) = &self.probe_scope {
            scope.validate()?;
        }
        if let Some(probe) = &self.probe {
            require(self.probe_scope.is_some())?;
            probe.validate()?;
        }
        require(
            self.checked
                .as_ref()
                .is_none_or(|v| self.binding().is_ok_and(|b| &b == v)),
        )
    }
    fn next(&mut self, expected: u64) -> Result<()> {
        if self.revision != expected {
            return Err(Error::Conflict);
        }
        self.revision = self
            .revision
            .checked_add(1)
            .filter(|v| *v <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER)
            .ok_or(Error::Invalid)?;
        Ok(())
    }
}
