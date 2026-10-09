//! Shared read-only guidance. Canonical metadata is not upstream conformance,
//! availability, admission, or permission to publish. No full-catalog hydration.
mod money;
use super::*;
use crate::discovery::{DiscoveryClient, Page, Query, QueryBinding};
use mayhem_proto::proxy::ProxyEndpoint;
pub use money::{au_to_usd, usd_to_au, usd_to_microusd};
use std::{collections::BTreeMap, time::Duration};

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Browse {
    Families {
        cursor: Option<String>,
    },
    Markets {
        family_id: String,
        endpoint: ProxyEndpoint,
        cursor: Option<String>,
    },
}
fn query(
    kind: &str,
    lookup: Option<String>,
    filter: BTreeMap<String, String>,
    cursor: Option<String>,
) -> Query {
    Query {
        binding: QueryBinding {
            kind: kind.into(),
            lookup,
            filter,
            limit: 40,
        },
        cursor,
        since: None,
    }
}
pub struct Canonical {
    client: DiscoveryClient,
    provider: Digest,
}
impl Canonical {
    pub fn new(host: &bootstrap::Host) -> Result<Self> {
        // Fixed host-owned peer only. Browser/model output cannot choose an origin.
        let url = url::Url::parse(&host.peer_rpc).map_err(|_| Error::Invalid)?;
        require(
            url.scheme() == "https"
                || (url.scheme() == "http"
                    && url.host_str().is_some_and(|h| {
                        h == "localhost"
                            || h.trim_matches(['[', ']'])
                                .parse::<std::net::IpAddr>()
                                .is_ok_and(|ip| ip.is_loopback())
                    })),
        )?;
        Ok(Self {
            client: DiscoveryClient::with_timeout(
                &host.peer_rpc,
                host.network.clone(),
                Duration::from_secs(5),
            )
            .map_err(|_| Error::Invalid)?,
            provider: host.provider_pubkey.clone(),
        })
    }
    pub async fn browse(&self, browse: Browse) -> Result<Page> {
        let q = match browse {
            Browse::Families { cursor } => query("families", None, BTreeMap::new(), cursor),
            Browse::Markets {
                family_id,
                endpoint,
                cursor,
            } => query(
                "markets",
                None,
                BTreeMap::from([
                    ("family_id".into(), family_id),
                    (
                        "endpoint_family".into(),
                        if endpoint == ProxyEndpoint::Decisions {
                            "decisions"
                        } else {
                            "llm"
                        }
                        .into(),
                    ),
                ]),
                cursor,
            ),
        };
        self.client
            .select(&q)
            .await
            .map_err(|_| Error::Bootstrap("canonical discovery unavailable; retry this read"))
    }
    async fn exact(&self, kind: &str, key: String) -> Result<Option<serde_json::Value>> {
        let page = self
            .client
            .select(&query(kind, Some(key), BTreeMap::new(), None))
            .await
            .map_err(|_| Error::Bootstrap("canonical selection unavailable; no setup saved"))?;
        Ok(page.entries.into_iter().next().map(|e| e.value))
    }
    pub async fn next_sequence(&self) -> Result<u64> {
        match self
            .exact("providers", self.provider.as_str().into())
            .await?
        {
            None => Ok(1),
            Some(v) => v["sequence"]
                .as_u64()
                .and_then(|n| n.checked_add(1))
                .filter(|n| *n <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER)
                .ok_or(Error::Invalid),
        }
    }
    /// Re-read the exact selection immediately before saving. No arbitrary
    /// client descriptor, missing family or stale sequence becomes authorization.
    pub async fn revalidate(&self, choices: &bootstrap::Choices) -> Result<()> {
        let family = match &choices.market {
            ProfileMarket::CreateMarket { slug, model } => {
                let definition = profiles()?
                    .into_iter()
                    .find(|p| p.endpoint == choices.endpoint)
                    .ok_or(Error::Invalid)?;
                let market = profile::created_market(
                    &self.provider,
                    slug.clone(),
                    model.clone(),
                    choices.endpoint,
                    vec![ProxyEndpointContract {
                        endpoint: choices.endpoint,
                        contract_hash: mayhem_proto::endpoint_contract_canonical_fingerprint(
                            &definition.contract,
                        ),
                    }],
                );
                if self
                    .exact("markets", market.id().map_err(|_| Error::Invalid)?)
                    .await?
                    .is_some()
                {
                    return Err(Error::Bootstrap("market already exists; choose join"));
                }
                &model.family_id
            }
            ProfileMarket::JoinMarket { market } => {
                let actual = self
                    .exact("markets", market.id().map_err(|_| Error::Invalid)?)
                    .await?
                    .ok_or(Error::Invalid)?;
                let actual: ProxyMarketDescriptor =
                    serde_json::from_value(actual).map_err(|_| Error::Invalid)?;
                require(actual == *market)?;
                require(compatible(market, choices.endpoint)?)?;
                &market.model.family_id
            }
        };
        let value = self
            .exact("families", family.clone())
            .await?
            .ok_or(Error::Invalid)?;
        require(value["enabled"] == true)?;
        require(self.next_sequence().await? == choices.sequence)
    }
}
/// Exact shared standard endpoint contract and meter, never model-name matching.
pub fn compatible(market: &ProxyMarketDescriptor, endpoint: ProxyEndpoint) -> Result<bool> {
    let profile = profiles()?
        .into_iter()
        .find(|p| p.endpoint == endpoint)
        .ok_or(Error::Invalid)?;
    let hash = mayhem_proto::endpoint_contract_canonical_fingerprint(&profile.contract);
    Ok(market.family == endpoint.family()
        && market
            .endpoints
            .iter()
            .any(|e| e.endpoint == endpoint && e.contract_hash == hash)
        && market.metering == Policy::for_endpoint(endpoint).contract())
}
