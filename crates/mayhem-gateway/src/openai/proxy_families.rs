//! Bounded canonical family facts for taxonomy publication. No policy writes,
//! admission decisions, provider evidence or whole-catalog enumeration.
use super::{now_millis_u64, ApiError, SharedState};
use axum::{
    body::to_bytes,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use mayhem_proxy::{
    catalog::CatalogRead, discovery::CATALOG_PREFIX, presence::CATALOG_AGE_MS, Error,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

static READS: Semaphore = Semaphore::const_new(8);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Lookup {
    schema_version: u32,
    family_ids: Vec<String>,
}
impl Lookup {
    fn valid(&self) -> bool {
        self.schema_version == 1
            && !self.family_ids.is_empty()
            && self.family_ids.len() <= 96
            && self.family_ids.windows(2).all(|w| w[0] < w[1])
            && self.family_ids.iter().all(|id| {
                !id.is_empty()
                    && id.len() <= 64
                    && id.as_bytes()[0].is_ascii_lowercase()
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
            })
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    enabled: bool,
    label: String,
}
#[derive(Serialize)]
struct Family {
    family_id: String,
    #[serde(flatten)]
    policy: Policy,
}
#[derive(Serialize)]
struct Families {
    schema_version: u32,
    network: mayhem_proxy::discovery::Identity,
    snapshot: String,
    proof: mayhem_proxy::discovery::Proof,
    observed_at_ms: u64,
    expires_at_ms: u64,
    families: Vec<Family>,
}
fn read(catalog: &CatalogRead, input: Lookup, now: u64) -> Result<Option<Families>, Error> {
    let status = catalog.status();
    if !status.discovery_is_fresh(now, CATALOG_AGE_MS) {
        return Err(Error::Invalid("canonical family catalog is stale".into()));
    }
    let committed = status.committed.unwrap();
    let mut families = Vec::with_capacity(input.family_ids.len());
    for family_id in input.family_ids {
        let Some(value) = catalog.get(&format!("{CATALOG_PREFIX}families/{family_id}"))? else {
            return Ok(None);
        };
        let policy: Policy = serde_json::from_value(value)?;
        if policy.label.is_empty()
            || policy.label.len() > 128
            || !policy.label.bytes().all(|b| (32..=126).contains(&b))
        {
            return Err(Error::Invalid("canonical family label is malformed".into()));
        }
        families.push(Family { family_id, policy });
    }
    let observed = committed.observed_at_ms.unwrap();
    Ok(Some(Families {
        schema_version: 1,
        network: committed.context.identity(),
        snapshot: status.content_snapshot,
        proof: committed.proof,
        observed_at_ms: observed,
        expires_at_ms: observed + CATALOG_AGE_MS,
        families,
    }))
}
fn failure(status: StatusCode, code: &'static str) -> Response {
    ApiError {
        status,
        message: "Canonical proxy family lookup could not be completed.".into(),
        param: None,
        public_code: code,
        category: "proxy_discovery",
        retryable: status == StatusCode::SERVICE_UNAVAILABLE,
        safe_detail: None,
    }
    .into_response()
}
fn private(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert("cache-control", "private, no-store".parse().unwrap());
    response
}
pub(super) async fn lookup(State(state): State<SharedState>, request: Request) -> Response {
    let auth = state
        .authorize_existing_gateway_request(request.headers(), None)
        .and_then(|auth| {
            auth.ok_or_else(|| {
                ApiError::unauthorized(
                    "canonical family metadata requires an authenticated key",
                    Some("Authorization"),
                )
            })
        });
    if let Err(error) = auth {
        return private(error.into_response());
    }
    if request.uri().query().is_some() {
        return private(failure(StatusCode::BAD_REQUEST, "invalid_request_error"));
    }
    let Ok(permit) = READS.try_acquire() else {
        return private(failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "proxy_directory_busy",
        ));
    };
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        to_bytes(request.into_body(), 8192),
    )
    .await;
    let input = match bytes {
        Ok(Ok(bytes)) => serde_json::from_slice::<Lookup>(&bytes)
            .ok()
            .filter(Lookup::valid),
        _ => None,
    };
    let Some(input) = input else {
        return private(failure(StatusCode::BAD_REQUEST, "invalid_request_error"));
    };
    let Some(control) = state.proxy_control().cloned() else {
        return private(failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "proxy_directory_disabled",
        ));
    };
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        read(&control.catalog().read()?, input, now_millis_u64())
    })
    .await;
    private(match result {
        Ok(Ok(Some(value))) => Json(value).into_response(),
        Ok(Ok(None)) => failure(StatusCode::NOT_FOUND, "proxy_family_not_found"),
        _ => failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "proxy_directory_unavailable",
        ),
    })
}

#[cfg(all(test, unix))]
mod tests;
