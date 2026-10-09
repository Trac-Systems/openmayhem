//! Public normative endpoint contracts, independently of request preparation,
//! inference capacity and spending. No dummy request or price authorization.
use super::*;
use axum::{
    body::to_bytes,
    extract::{Path, Query, Request as HttpRequest},
    http::StatusCode,
};
use mayhem_proto::proxy::ProxyRail;
use mayhem_proxy::{directory::PublishedOffer, presence::Registered};
use serde::Deserialize;

static READS: Semaphore = Semaphore::const_new(mayhem_proxy::descriptor::READS);
static CATALOG_READS: Semaphore = Semaphore::const_new(mayhem_proxy::descriptor::READS);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    rail: ProxyRail,
}

struct Selected {
    offer: PublishedOffer,
    network: mayhem_proxy::discovery::Identity,
    endpoint_contract: Digest,
    recipe_hash: Digest,
    expires_at_ms: u64,
}
fn failure(status: StatusCode, code: &'static str) -> ApiError {
    ApiError {
        status,
        message: "Proxy endpoint contract is unavailable or the lookup is invalid.".into(),
        param: Some("proxy".into()),
        public_code: code,
        category: "proxy_contract",
        retryable: status == StatusCode::SERVICE_UNAVAILABLE,
        safe_detail: None,
    }
}
fn unavailable() -> ApiError {
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "proxy_contract_unavailable",
    )
}
fn invalid() -> ApiError {
    failure(StatusCode::BAD_REQUEST, "proxy_contract_invalid")
}

/// One fresh canonical snapshot. Deliberately does not consult live presence,
/// prepare an endpoint request, synthesize rate ceilings or select another offer.
async fn select(
    control: Arc<super::super::proxy_control::ProxyControl>,
    selector: proxy_request::Selector,
    rail: ProxyRail,
) -> Result<Selected, ApiError> {
    let permit = CATALOG_READS
        .try_acquire()
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE, "proxy_contract_busy"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let now = super::super::now_millis_u64();
        let catalog = control.catalog().read().map_err(|_| unavailable())?;
        let status = catalog.status();
        if !status.discovery_is_fresh(now, mayhem_proxy::presence::CATALOG_AGE_MS) {
            return Err(unavailable());
        }
        let offer = catalog
            .proxy_offer(&selector.id(), now)
            .map_err(|_| unavailable())?
            .ok_or_else(|| failure(StatusCode::NOT_FOUND, "proxy_offer_not_found"))?;
        let registered = Registered::read(
            &catalog,
            selector.market(),
            selector.provider(),
            selector.slot(),
            now,
        )
        .map_err(|_| unavailable())?;
        if offer.id != selector.id()
            || offer.lane != "proxy"
            || !offer.active
            || !offer.catalog_eligible
            || registered.offer() != &offer.offer
        {
            return Err(unavailable());
        }
        if !offer.offer.accepted_rails.contains(&rail) {
            return Err(invalid());
        }
        let contract = offer
            .membership
            .endpoints
            .iter()
            .find(|v| v.endpoint == offer.offer.endpoint)
            .ok_or_else(unavailable)?;
        let endpoint_contract =
            Digest::new(contract.contract_hash.clone()).map_err(|_| unavailable())?;
        let recipe_hash =
            Digest::new(offer.membership.recipe_hash.clone()).map_err(|_| unavailable())?;
        let committed = status.committed.ok_or_else(unavailable)?;
        let expires_at_ms = committed
            .observed_at_ms
            .ok_or_else(unavailable)?
            .saturating_add(mayhem_proxy::presence::CATALOG_AGE_MS);
        Ok(Selected {
            offer,
            network: committed.context.identity(),
            endpoint_contract,
            recipe_hash,
            expires_at_ms,
        })
    })
    .await
    .map_err(|_| unavailable())?
}

pub(crate) async fn handle(
    State(state): State<SharedState>,
    Path((market, provider, slot)): Path<(String, String, String)>,
    request: HttpRequest,
) -> Response {
    let result = tokio::time::timeout(
        mayhem_proxy::descriptor::DEADLINE,
        read(state, market, provider, slot, request),
    )
    .await
    .unwrap_or_else(|_| Err(unavailable()));
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    response
        .headers_mut()
        .insert("cache-control", "private, no-store".parse().unwrap());
    response
}
async fn read(
    state: SharedState,
    market: String,
    provider: String,
    slot: String,
    request: HttpRequest,
) -> Result<Response, ApiError> {
    let model = format!("proxy/offer/{market}/{provider}/{slot}");
    state
        .authorize_existing_gateway_request(request.headers(), Some(&model))?
        .ok_or_else(|| {
            ApiError::unauthorized(
                "proxy endpoint contracts require an authenticated key",
                Some("Authorization"),
            )
        })?;
    let selector = proxy_request::Selector::parse(&model)
        .map_err(|_| invalid())?
        .ok_or_else(invalid)?;
    if request.uri().query().is_none_or(|q| q.len() > 64) {
        return Err(invalid());
    }
    let Query(params) = Query::<Params>::try_from_uri(request.uri()).map_err(|_| invalid())?;
    let runtime = state
        .proxy_buyer
        .as_ref()
        .ok_or_else(|| failure(StatusCode::SERVICE_UNAVAILABLE, "proxy_buyer_disabled"))?;
    let control = state.proxy_control().cloned().ok_or_else(unavailable)?;
    let _permit = READS
        .try_acquire()
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE, "proxy_contract_busy"))?;
    // GET has no request body. Empty slow bodies and peer reads have finite waits.
    tokio::time::timeout(
        mayhem_proxy::descriptor::DEADLINE,
        to_bytes(request.into_body(), 0),
    )
    .await
    .map_err(|_| invalid())?
    .map_err(|_| invalid())?;
    let selected = select(control.clone(), selector.clone(), params.rail).await?;
    let wallet = runtime.controller.identity();
    if selected.network.network_id != wallet.network_id
        || selected.network.msb_bootstrap != wallet.msb_bootstrap.as_str()
        || selected.network.subnet_bootstrap != wallet.subnet_bootstrap.as_str()
        || selected.network.contract_version != mayhem_proto::CONTRACT_VERSION
    {
        return Err(unavailable());
    }
    let adapter = runtime
        .controller
        .describe(
            selected.offer.offer.clone(),
            params.rail,
            runtime.policy.settlement_policy_hash().clone(),
            &selected.endpoint_contract,
            &selected.recipe_hash,
        )
        .await
        .map_err(|_| unavailable())?;
    let latest = select(control, selector, params.rail).await?;
    let now = super::super::now_millis_u64();
    let expires = selected.expires_at_ms.min(latest.expires_at_ms);
    if latest.network != selected.network
        || latest.offer.digest != selected.offer.digest
        || latest.offer.membership != selected.offer.membership
        || latest.endpoint_contract != selected.endpoint_contract
        || latest.recipe_hash != selected.recipe_hash
        || expires <= now
    {
        return Err(unavailable());
    }
    // Project only public normative protocol data. Snapshot local limits and
    // provider private connection/upstream mapping never enter this response.
    Ok(Json(serde_json::json!({
        "schema_version":1,"lane":"proxy","kind":"endpoint_contract","network":latest.network,
        "model":model,"rail":params.rail,"offer_digest":latest.offer.digest,
        "membership_digest":latest.offer.membership.digest().map_err(|_| unavailable())?,
        "endpoint":latest.offer.offer.endpoint,"endpoint_contract":latest.endpoint_contract,
        "recipe_hash":latest.recipe_hash,"settlement_policy_hash":runtime.policy.settlement_policy_hash(),
        "contract":adapter.contract,
        "metering_policy":mayhem_proxy::metering::Policy::for_endpoint(adapter.endpoint).definition(),
        "observed_at_ms":now,"expires_at_ms":expires,
    })).into_response())
}
