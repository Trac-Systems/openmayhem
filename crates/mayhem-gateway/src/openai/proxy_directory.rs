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
use mayhem_proxy::{catalog::CatalogRead, directory, Error};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

// Bound synchronous database work separately from inference. No waiting queue,
// no per-card background polling and no unbounded spawn_blocking backlog.
static READS: Semaphore = Semaphore::const_new(8);

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
    F: FnOnce(&CatalogRead) -> Result<Option<T>, Error> + Send + 'static,
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
        operation(&snapshot)
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
    read(state, headers, move |catalog| {
        catalog
            .proxy_offers(&query, params.cursor.as_deref(), limit, now_millis_u64())
            .map(Some)
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
    read(state, headers, move |catalog| {
        catalog.proxy_offer(&id, now_millis_u64())
    })
    .await
}

#[cfg(all(test, unix))]
mod tests;
