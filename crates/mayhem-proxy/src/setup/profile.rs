//! Offline, explicit same-protocol profiles. Templates describe what the adapter
//! accepts; they never certify that an upstream implements those capabilities.
use super::*;
use crate::endpoint::Limits;
use mayhem_proto::{proxy::*, EndpointFamilyContract};

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EndpointProfile {
    Standard {
        endpoint: ProxyEndpoint,
    },
    Custom {
        endpoint: ProxyEndpoint,
        contract: EndpointFamilyContract,
    },
}
impl EndpointProfile {
    fn resolve(&self) -> Result<(ProxyEndpoint, EndpointFamilyContract)> {
        match self {
            Self::Standard { endpoint } => Ok((*endpoint, template(*endpoint)?)),
            Self::Custom { endpoint, contract } => Ok((*endpoint, contract.clone())),
        }
    }
}
fn template(endpoint: ProxyEndpoint) -> Result<EndpointFamilyContract> {
    let family = match endpoint {
        ProxyEndpoint::Chat => mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
        ProxyEndpoint::Completions => mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS,
        ProxyEndpoint::Responses => mayhem_proto::ENDPOINT_OPENAI_RESPONSES,
        ProxyEndpoint::Decisions => mayhem_proto::ENDPOINT_MAYHEM_DECISIONS,
    };
    mayhem_proto::endpoint_family_contract_template(family).ok_or(Error::Invalid)
}

#[derive(Serialize)]
pub struct ProfileReview {
    pub schema_version: u32,
    pub kind: &'static str,
    pub endpoint: ProxyEndpoint,
    pub contract: EndpointFamilyContract,
    pub metering_policy: serde_json::Value,
    pub assurance: &'static str,
}
/// Local templates only. Nothing is fetched, inferred or published.
pub fn profiles() -> Result<Vec<ProfileReview>> {
    [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ]
    .into_iter()
    .map(|endpoint| {
        Ok(ProfileReview {
            schema_version: 1,
            kind: "same_protocol_profile",
            endpoint,
            contract: template(endpoint)?,
            metering_policy: Policy::for_endpoint(endpoint).definition(),
            assurance: "local_template_not_upstream_evidence",
        })
    })
    .collect()
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProfileMarket {
    CreateMarket {
        slug: String,
        model: ProxyModelClaim,
    },
    /// The caller supplies the exact public market it intends to join. This
    /// local preparation does not attest to its canonical publication.
    JoinMarket { market: ProxyMarketDescriptor },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipInput {
    pub revision: u64,
    pub served_context: u32,
    pub max_concurrency: u32,
    pub capacity_group: String,
    pub accepted_rails: Vec<ProxyRail>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfferInput {
    pub revision: u64,
    pub ctx_bracket: String,
    pub outcome_class: String,
    pub rates: Vec<ProxyRate>,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub per_request_au: mayhem_proto::MoneyAu,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub min_session_au: mayhem_proto::MoneyAu,
    pub accepted_rails: Vec<ProxyRail>,
}
/// Private operator input. No price, identity, resource limit or model claim is
/// inferred from a model list or filled by a vendor/model-specific default.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileInput {
    pub schema_version: u32,
    pub network: Identity,
    pub provider_pubkey: Digest,
    pub connection_file: PathBuf,
    pub profile: EndpointProfile,
    pub upstream_model: String,
    pub limits: Limits,
    pub market: ProfileMarket,
    pub membership: MembershipInput,
    pub offers: Vec<OfferInput>,
    pub sequence: u64,
    pub settlement_policy: ProxySettlementPolicy,
}
impl ProfileInput {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = private_file(path, MAX_BYTES).map_err(|_| Error::Protection)?;
        let mut value: Self = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
        if value.connection_file.is_relative() {
            let parent = std::fs::canonicalize(
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )
            .map_err(|_| Error::Protection)?;
            value.connection_file = parent.join(&value.connection_file);
        }
        Ok(value)
    }
    /// Build precisely the existing declaration, including all canonical hashes.
    /// This loads connection metadata only; it never resolves credentials or performs network I/O.
    pub fn prepare(self) -> Result<Input> {
        require(self.schema_version == 1 && self.connection_file.is_absolute())?;
        let connection =
            ConnectionConfig::load(&self.connection_file).map_err(|_| Error::Protection)?;
        let (endpoint, contract) = self.profile.resolve()?;
        let adapter = Adapter::new(endpoint, contract, self.upstream_model, self.limits)
            .map_err(|_| Error::Invalid)?;
        require(connection.paths.contains_key(&adapter.operation()))?;
        let endpoints = vec![ProxyEndpointContract {
            endpoint,
            contract_hash: adapter.contract_hash().as_str().into(),
        }];
        let (selection, market) = match self.market {
            ProfileMarket::CreateMarket { slug, model } => (
                Selection::CreateMarket,
                ProxyMarketDescriptor {
                    schema_version: 1,
                    lane: ProxyLane::Proxy,
                    creator_pubkey: self.provider_pubkey.as_str().into(),
                    slug,
                    model,
                    family: endpoint.family(),
                    endpoints: endpoints.clone(),
                    metering: Policy::for_endpoint(endpoint).contract(),
                    pricing: ProxyPricing::ProviderOffers,
                },
            ),
            ProfileMarket::JoinMarket { market } => (Selection::JoinMarket, market),
        };
        let membership = ProxyMembership {
            schema_version: 1,
            lane: ProxyLane::Proxy,
            market_id: market.id().map_err(|_| Error::Invalid)?,
            provider_pubkey: self.provider_pubkey.as_str().into(),
            revision: self.membership.revision,
            endpoints,
            served_context: self.membership.served_context,
            max_concurrency: self.membership.max_concurrency,
            recipe_hash: adapter.recipe_hash().as_str().into(),
            connection_revision: connection.revision,
            capacity_group: self.membership.capacity_group,
            accepted_rails: self.membership.accepted_rails,
        };
        let offers = self
            .offers
            .into_iter()
            .map(|offer| ProxyOffer {
                schema_version: 1,
                lane: ProxyLane::Proxy,
                market_id: membership.market_id.clone(),
                provider_pubkey: membership.provider_pubkey.clone(),
                membership_revision: membership.revision,
                revision: offer.revision,
                endpoint,
                ctx_bracket: offer.ctx_bracket,
                outcome_class: offer.outcome_class,
                metering_policy_hash: market.metering.policy_hash.clone(),
                rates: offer.rates,
                per_request_au: offer.per_request_au,
                min_session_au: offer.min_session_au,
                accepted_rails: offer.accepted_rails,
            })
            .collect();
        let input = Input {
            schema_version: 1,
            network: self.network,
            provider_pubkey: self.provider_pubkey,
            connection_file: self.connection_file,
            adapter: adapter.snapshot(),
            market,
            membership,
            offers,
            selection,
            sequence: self.sequence,
            settlement_policy: self.settlement_policy,
        };
        input.validate()?;
        input.connection()?;
        Ok(input)
    }
}
impl Store {
    /// None creates the original draft; Some updates only that exact revision.
    /// Existing probe scope, identity and recovery constraints are unchanged.
    pub fn prepare(&self, profile: ProfileInput, expected_revision: Option<u64>) -> Result<Review> {
        let input = profile.prepare()?;
        match expected_revision {
            Some(revision) => self.update(revision, input),
            None => self.create(input),
        }
    }
}
