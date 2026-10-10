//! Proxy-only public read endpoints. These neither dispatch inference nor mutate
//! payment, presence subscriptions or canonical state. Native routes are intact.

use super::{now_millis_u64, ApiError, SharedState};
use axum::{
    extract::{rejection::QueryRejection, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use mayhem_proto::proxy::{ProxyEndpoint, ProxyFamily, ProxyRail};
use mayhem_proxy::{
    attempts::Digest,
    catalog::CatalogRead,
    directory,
    presence::{gateway::Gateway, Eligibility, Observation, Registered},
    Error,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

// Bound synchronous database work separately from inference. No waiting queue,
// no per-card background polling and no unbounded spawn_blocking backlog.
static READS: Semaphore = Semaphore::const_new(8);

/// Availability is an independent observation, never part of the canonical
/// publication/digest or a promise that a slot will remain available.
#[derive(Serialize)]
struct ObservedOffer {
    #[serde(flatten)]
    publication: directory::PublishedOffer,
    availability: Observation,
}

#[derive(Serialize)]
struct ObservedPage {
    query_key: String,
    snapshot: String,
    entries: Vec<ObservedOffer>,
    previous_cursor: Option<String>,
    next_cursor: Option<String>,
    scanned_candidates: usize,
}

// One point-lookup batch for a viewport and its selection. It is deliberately
// smaller than a catalog page and shares the same bounded reader semaphore.
const MAX_BATCH_IDS: usize = 16;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BatchParams {
    ids: String,
}

#[derive(Serialize)]
struct ObservedBatchEntry {
    id: String,
    offer: Option<ObservedOffer>,
}

#[derive(Serialize)]
struct ObservedBatch {
    object: &'static str,
    observed_at_ms: u64,
    entries: Vec<ObservedBatchEntry>,
}

fn observe(
    catalog: &CatalogRead,
    presence: &Gateway,
    publication: directory::PublishedOffer,
    now: u64,
) -> Result<ObservedOffer, Error> {
    let unavailable = || Observation {
        status: Eligibility::CatalogUnavailable,
        observed_at_ms: now,
        expires_at_ms: None,
    };
    let [market, provider, slot]: [Digest; 3] = publication
        .id
        .split('/')
        .map(|id| {
            Digest::new(id).map_err(|_| Error::Invalid("invalid directory offer identity".into()))
        })
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| Error::Invalid("invalid directory offer identity".into()))?;
    let registered = Registered::read(catalog, &market, &provider, &slot, now);
    let availability = match registered {
        Ok(registered) => {
            if registered.offer() != &publication.offer
                || !publication.active
                || !publication.catalog_eligible
            {
                unavailable()
            } else {
                // None is the SAME default floor used by admission: 5 tok/s for
                // LLMs; decisions do not acquire an invented token requirement.
                presence.observe_registered(&registered, None)?
            }
        }
        Err(Error::Invalid(_) | Error::Identity) => unavailable(),
        Err(error) => return Err(error),
    };
    Ok(ObservedOffer {
        publication,
        availability,
    })
}

fn observe_page(
    catalog: &CatalogRead,
    presence: &Gateway,
    page: directory::OfferPage,
    now: u64,
) -> Result<ObservedPage, Error> {
    let directory::OfferPage {
        query_key,
        snapshot,
        entries,
        previous_cursor,
        next_cursor,
        scanned_candidates,
    } = page;
    Ok(ObservedPage {
        query_key,
        snapshot,
        entries: entries
            .into_iter()
            .map(|entry| observe(catalog, presence, entry, now))
            .collect::<Result<_, _>>()?,
        previous_cursor,
        next_cursor,
        scanned_candidates,
    })
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Params {
    kind: Option<ProxyFamily>,
    family_id: Option<String>,
    #[serde(default)]
    name_prefix: String,
    endpoint: Option<ProxyEndpoint>,
    minimum_context: Option<u32>,
    rail: Option<ProxyRail>,
    cursor: Option<String>,
    limit: Option<usize>,
}

fn failure(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    no_store(
        ApiError {
            status,
            message: message.into(),
            param: None,
            public_code: code,
            category: "proxy_discovery",
            retryable: status == StatusCode::SERVICE_UNAVAILABLE,
            safe_detail: None,
        }
        .into_response(),
    )
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
}

fn invalid_request() -> Response {
    failure(
        StatusCode::BAD_REQUEST,
        "invalid_request_error",
        "Invalid proxy directory query or offer ID.",
    )
}

async fn read<T, F>(state: SharedState, headers: HeaderMap, operation: F) -> Response
where
    T: Serialize + Send + 'static,
    F: FnOnce(&CatalogRead, &Gateway) -> Result<Option<T>, Error> + Send + 'static,
{
    if let Err(error) = state.authorize_gateway_request(&headers, None) {
        return no_store(error.into_response());
    }
    let Some(control) = state.proxy_control().cloned() else {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "proxy_directory_disabled",
            "Proxy discovery is not configured on this gateway.",
        );
    };
    let Ok(permit) = READS.try_acquire() else {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "proxy_directory_busy",
            "Proxy discovery is busy. Retry shortly.",
        );
    };
    let result = tokio::task::spawn_blocking(move || {
        // The permit lives until the database read ends, even if HTTP disconnects.
        let _permit = permit;
        let snapshot = control.catalog().read()?;
        if snapshot.status().invalidated || snapshot.status().committed.is_none() {
            return Err(Error::Invalid("proxy directory not hydrated".into()));
        }
        operation(&snapshot, control.presence())
    })
    .await;
    let response = match result {
        Ok(Ok(Some(value))) => Json(value).into_response(),
        Ok(Ok(None)) => failure(
            StatusCode::NOT_FOUND,
            "proxy_offer_not_found",
            "This proxy offer was not found.",
        ),
        Ok(Err(Error::DirectoryCursorExpired)) => failure(
            StatusCode::CONFLICT,
            "proxy_directory_cursor_expired",
            "The proxy catalog changed. Restart this query and preserve your selection.",
        ),
        // Cursor content is untrusted. Validation details and disk paths stay private.
        Ok(Err(Error::DirectoryCursorInvalid)) => invalid_request(),
        _ => failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "proxy_directory_unavailable",
            "Proxy discovery is temporarily unavailable.",
        ),
    };
    no_store(response)
}

pub(super) async fn list(
    State(state): State<SharedState>,
    headers: HeaderMap,
    params: Result<Query<Params>, QueryRejection>,
) -> Response {
    let Ok(Query(params)) = params else {
        return invalid_request();
    };
    let query = directory::Query {
        kind: params.kind,
        family_id: params.family_id,
        name_prefix: params.name_prefix,
        endpoint: params.endpoint,
        minimum_context: params.minimum_context,
        rail: params.rail,
    };
    let limit = params.limit.unwrap_or(50);
    if query.key().is_err()
        || limit == 0
        || limit > 100
        || params.cursor.as_ref().is_some_and(|c| c.len() > 8192)
    {
        return invalid_request();
    }
    read(state, headers, move |catalog, presence| {
        let now = now_millis_u64();
        let page = catalog.proxy_offers(&query, params.cursor.as_deref(), limit, now)?;
        observe_page(catalog, presence, page, now).map(Some)
    })
    .await
}

pub(super) async fn get(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((market, provider, slot)): Path<(String, String, String)>,
) -> Response {
    if [&market, &provider, &slot]
        .iter()
        .any(|s| mayhem_proxy::attempts::Digest::new(s.as_str()).is_err())
    {
        return invalid_request();
    }
    let id = format!("{market}/{provider}/{slot}");
    read(state, headers, move |catalog, presence| {
        let now = now_millis_u64();
        catalog
            .proxy_offer(&id, now)?
            .map(|entry| observe(catalog, presence, entry, now))
            .transpose()
    })
    .await
}

pub(super) async fn batch(
    State(state): State<SharedState>,
    headers: HeaderMap,
    params: Result<Query<BatchParams>, QueryRejection>,
) -> Response {
    let Ok(Query(params)) = params else {
        return invalid_request();
    };
    if params.ids.len() > MAX_BATCH_IDS * 195 - 1 {
        return invalid_request();
    }
    let ids: Vec<String> = params.ids.split(',').map(str::to_owned).collect();
    let mut unique = std::collections::HashSet::new();
    if ids.is_empty()
        || ids.len() > MAX_BATCH_IDS
        || ids.iter().any(|id| {
            id.len() != 194
                || id.split('/').count() != 3
                || id.split('/').any(|part| Digest::new(part).is_err())
                || !unique.insert(id.clone())
        })
    {
        return invalid_request();
    }
    read(state, headers, move |catalog, presence| {
        let now = now_millis_u64();
        let entries = ids
            .into_iter()
            .map(|id| {
                let offer = catalog
                    .proxy_offer(&id, now)?
                    .map(|publication| observe(catalog, presence, publication, now))
                    .transpose()?;
                Ok(ObservedBatchEntry { id, offer })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(Some(ObservedBatch {
            object: "proxy.offer_batch",
            observed_at_ms: now,
            entries,
        }))
    })
    .await
}

#[cfg(all(test, unix))]
mod tests;
