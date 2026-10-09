//! Read-only estimates. No HTTP job, owner gate, capacity lease, signing intent,
//! budget hold or model request is constructed by this handler.
use super::*;
use axum::{body::to_bytes, extract::Request as HttpRequest};
use mayhem_proxy::financial::quote::PurchaseRequest;
use serde::Deserialize;

static READS: Semaphore = Semaphore::const_new(mayhem_proxy::descriptor::READS);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Envelope {
    schema_version: u32,
    endpoint: ProxyEndpoint,
    request: Value,
}
fn failure(code: &'static str, unavailable: bool) -> ApiError {
    let error = if unavailable {
        ApiError::service_unavailable("proxy estimate is temporarily unavailable", Some("proxy"))
    } else {
        ApiError::bad_request(
            "invalid proxy estimate request or constraints",
            Some("proxy"),
        )
    };
    error.with_public_error(code, "proxy_estimate", unavailable)
}
fn selection_error(error: proxy_request::Error) -> ApiError {
    use proxy_request::Error::*;
    match error {
        SettlementPolicyMismatch => failure("proxy_settlement_policy_mismatch", false),
        ProfileEvidence => failure("proxy_profile_evidence_unavailable", true),
        Busy => failure("proxy_estimate_busy", true),
        Catalog | Availability(_) => failure("proxy_estimate_unavailable", true),
        _ => failure("proxy_estimate_invalid", false),
    }
}
pub(crate) async fn handle(State(state): State<SharedState>, request: HttpRequest) -> Response {
    let result = read(state, request).await;
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    response
        .headers_mut()
        .insert("cache-control", "private, no-store".parse().unwrap());
    response
}
async fn read(state: SharedState, request: HttpRequest) -> Result<Response, ApiError> {
    let headers = request.headers().clone();
    if super::super::gateway_bearer_token(&headers)?.is_none() {
        return Err(ApiError::unauthorized(
            "proxy estimates require an authenticated key",
            Some("Authorization"),
        ));
    }
    state
        .access_control
        .preauthorize_body_headers_mode(&headers, false)?;
    let permit = READS
        .try_acquire()
        .map_err(|_| failure("proxy_estimate_busy", true))?;
    let runtime = state
        .proxy_buyer
        .as_ref()
        .ok_or_else(|| failure("proxy_buyer_disabled", true))?;
    // Charge no inference slots; bound raw input before allocating/parsing JSON.
    let limit = runtime
        .policy
        .request_byte_limit()
        .saturating_add(33 * 1024);
    let bytes = tokio::time::timeout(
        mayhem_proxy::descriptor::DEADLINE,
        to_bytes(request.into_body(), limit),
    )
    .await
    .map_err(|_| failure("proxy_estimate_invalid", false))?
    .map_err(|_| failure("proxy_estimate_invalid", false))?;
    let envelope =
        serde_json::from_slice(&bytes).map_err(|_| failure("proxy_estimate_invalid", false))?;
    estimate(state, headers, envelope, permit).await
}
async fn estimate(
    state: SharedState,
    headers: HeaderMap,
    envelope: Envelope,
    permit: tokio::sync::SemaphorePermit<'static>,
) -> Result<Response, ApiError> {
    let raw = envelope.request.get("model").and_then(Value::as_str);
    state
        .authorize_existing_gateway_request(&headers, raw)?
        .ok_or_else(|| {
            ApiError::unauthorized(
                "proxy estimates require an authenticated key",
                Some("Authorization"),
            )
        })?;
    let runtime = state
        .proxy_buyer
        .as_ref()
        .ok_or_else(|| failure("proxy_buyer_disabled", true))?;
    let control = state
        .proxy_control()
        .cloned()
        .ok_or_else(|| failure("proxy_estimate_unavailable", true))?;
    if envelope.schema_version != 1 {
        return Err(failure("proxy_estimate_invalid", false));
    }
    let request = Arc::new(
        proxy_request::Request::parse(envelope.endpoint, envelope.request, &runtime.policy)
            .map_err(selection_error)?
            .ok_or_else(|| failure("proxy_estimate_invalid", false))?,
    );
    request.check_settlement_policy().map_err(selection_error)?;
    let selected = proxy_request::resolve_estimate(control.clone(), request.clone())
        .await
        .map_err(selection_error)?;
    let adapter = runtime
        .controller
        .describe(
            selected.candidate.offer.clone(),
            request.controls().rail,
            request.controls().settlement_policy_hash.clone(),
            &selected.candidate.endpoint_contract,
            &selected.candidate.recipe_hash,
        )
        .await
        .map_err(|_| failure("proxy_estimate_unavailable", true))?;
    super::profile::validate(
        control.clone(),
        request.clone(),
        adapter.clone(),
        selected.candidate.offer.clone(),
    )
    .await?;
    let prepare_request = request.clone();
    let offer = selected.candidate.offer.clone();
    // The read permit survives a disconnected HTTP caller until CPU work ends.
    let (maximum, content_digest, _permit) = tokio::task::spawn_blocking(move || {
        let result = (|| {
            let intent = PurchaseRequest::new(
                adapter,
                prepare_request.provider_request().to_vec(),
                prepare_request.controls().prices.clone(),
                prepare_request.controls().output_units,
                prepare_request.policy().lifetimes(),
            )
            .map_err(|_| failure("proxy_estimate_invalid", false))?;
            let maximum = intent
                .maximum(&offer)
                .map_err(|_| failure("proxy_estimate_invalid", false))?;
            let body: Value = serde_json::from_slice(prepare_request.provider_request())
                .map_err(|_| failure("proxy_estimate_invalid", false))?;
            let content = retail::request_content_digest(&body)
                .map_err(|_| failure("proxy_estimate_invalid", false))?;
            Ok::<_, ApiError>((maximum, content))
        })();
        result.map(|(maximum, content)| (maximum, content, permit))
    })
    .await
    .map_err(|_| failure("proxy_estimate_unavailable", true))??;
    // Descriptor/network awaits cannot renew catalog freshness or substitute a
    // newer offer, membership, recipe or price into the estimate.
    let latest = proxy_request::resolve_estimate(control.clone(), request.clone())
        .await
        .map_err(selection_error)?;
    if latest.network != selected.network
        || latest.published.digest != selected.published.digest
        || latest.published.membership != selected.published.membership
    {
        return Err(failure("proxy_estimate_unavailable", true));
    }
    let now = super::super::now_millis_u64();
    let mut expires = selected.expires_at_ms.min(latest.expires_at_ms);
    if let Some(record) =
        super::evidence::check(control, request.clone(), &latest.published).await?
    {
        expires = expires.min(record.body.expires_at_ms);
    }
    if latest.availability.status == mayhem_proxy::presence::Eligibility::Available {
        expires = expires.min(
            latest
                .availability
                .expires_at_ms
                .ok_or_else(|| failure("proxy_estimate_unavailable", true))?,
        );
    }
    if expires <= now {
        return Err(failure("proxy_estimate_unavailable", true));
    }
    let mut value = serde_json::json!({
        "schema_version":1,"lane":"proxy","kind":"maximum_estimate",
        "network":latest.network,"model":request.selector().model(),"endpoint":request.endpoint(),
        "rail":request.controls().rail,"request_hash":maximum.request_hash,
        "request_content_digest":content_digest,"controls":request.controls(),
        "offer":latest.published.offer,"offer_digest":latest.published.digest,
        "membership_digest":latest.published.membership.digest().map_err(|_| failure("proxy_estimate_unavailable", true))?,
        "endpoint_contract":latest.candidate.endpoint_contract,"recipe_hash":latest.candidate.recipe_hash,
        "settlement_policy_hash":request.controls().settlement_policy_hash,
        "metering_policy_hash":maximum.metering_policy_hash,"max_usage":maximum.max_usage,
        "max_spend_au":maximum.max_spend_au.to_string(),"observed_at_ms":now,"expires_at_ms":expires,
        "availability":latest.availability,
    });
    let bytes = mayhem_proto::stable_json_bytes(&value)
        .map_err(|_| failure("proxy_estimate_invalid", false))?;
    value["estimate_hash"] =
        serde_json::json!(digest("mayhem/proxy/maximum-estimate/v1", &[&bytes]));
    Ok(Json(value).into_response())
}
