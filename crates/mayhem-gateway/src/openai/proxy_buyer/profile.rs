//! Exact saved controls, materialized before request/financial fingerprinting.
//! Published definitions are semantics, never capability or supplier evidence.
use super::*;
use axum::{body::to_bytes, extract::Request as HttpRequest};
use mayhem_proxy::{
    endpoint::PublicAdapterSnapshot,
    registry::{self, publication::Reference},
};
use serde::Deserialize;

static READS: Semaphore = Semaphore::const_new(4);
static VALIDATIONS: Semaphore = Semaphore::const_new(4);
const DEADLINE: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    schema_version: u32,
    endpoint: ProxyEndpoint,
    request: Value,
}

pub(super) fn invalid_controls() -> ApiError {
    ApiError::bad_request(
        "Saved proxy controls do not match the explicit request or endpoint contract",
        Some("proxy.profile"),
    )
    .with_public_error("proxy_profile_controls_invalid", "proxy_profile", false)
}
fn unavailable_controls() -> ApiError {
    selection_error(proxy_request::Error::ProfileEvidence)
}
fn has_controls(request: &proxy_request::Request) -> bool {
    request.controls().profile.as_ref().is_some_and(|p| {
        !p.constraints.request_controls.is_empty()
            || !p.constraints.capabilities.is_empty()
            || !p.constraints.data_handling.is_empty()
    })
}

/// Resolves one immutable release and exact closure. CPU work retains its permit
/// after caller cancellation; no registry-wide reads or implicit defaults.
pub(super) async fn materialize(
    control: Arc<super::super::proxy_control::ProxyControl>,
    request: Arc<proxy_request::Request>,
    adapter: PublicAdapterSnapshot,
    offer: mayhem_proto::proxy::ProxyOffer,
    allow_current: bool,
) -> Result<(Value, proxy_request::RegistryRelease), ApiError> {
    let permit = VALIDATIONS
        .try_acquire()
        .map_err(|_| unavailable_controls())?;
    let policy = request
        .controls()
        .profile
        .as_ref()
        .ok_or_else(invalid_controls)?;
    if super::evidence::unsupported(policy)
        || (policy.constraints.request_controls.is_empty()
            && policy.constraints.capabilities.is_empty()
            && policy.constraints.data_handling.is_empty())
    {
        return Err(unavailable_controls());
    }
    let reader = control.registry().ok_or_else(unavailable_controls)?;
    let pin = match &request.controls().registry_release {
        Some(binding) => {
            let pin = reader
                .pin_release(&binding.release_id)
                .await
                .map_err(|_| unavailable_controls())?;
            if pin.metadata().release_hash != binding.release_hash.as_str() {
                return Err(invalid_controls());
            }
            pin
        }
        None if allow_current => reader.current().await.map_err(|_| unavailable_controls())?,
        None => return Err(invalid_controls()),
    };
    let references = policy
        .constraints
        .request_controls
        .iter()
        .map(|c| Reference {
            field_id: c.field_id.clone(),
            schema_revision: c.schema_revision,
        })
        .chain(policy.constraints.capabilities.iter().map(|p| Reference {
            field_id: p.field_id.clone(),
            schema_revision: p.schema_revision,
        }))
        .chain(policy.constraints.data_handling.iter().map(|p| Reference {
            field_id: p.field_id.clone(),
            schema_revision: p.schema_revision,
        }))
        .collect::<Vec<_>>();
    let definitions = reader
        .resolve_closure(&pin, &references)
        .await
        .map_err(|_| unavailable_controls())?;
    let binding = proxy_request::RegistryRelease {
        release_id: pin.metadata().release_id.clone(),
        release_hash: Digest::new(&pin.metadata().release_hash)
            .map_err(|_| unavailable_controls())?,
    };
    let body = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let policy = request
            .controls()
            .profile
            .as_ref()
            .ok_or_else(invalid_controls)?;
        let body = registry::apply_controls(
            request.provider_value(),
            request.endpoint(),
            &adapter.contract,
            &policy.constraints.request_controls,
            |id, revision| definitions.get(id, revision),
        )
        .map_err(|_| invalid_controls())?;
        if serde_json::to_vec(&body)
            .map_err(|_| invalid_controls())?
            .len()
            > request.policy().request_byte_limit()
        {
            return Err(invalid_controls());
        }
        // Use exactly the ordinary request/stream, local protocol limits,
        // metering allowance and full-price validation, without purchasing.
        mayhem_proxy::financial::quote::PurchaseRequest::new(
            adapter,
            serde_json::to_vec(&body).map_err(|_| invalid_controls())?,
            request.controls().prices.clone(),
            request.controls().output_units,
            request.policy().lifetimes(),
        )
        .map_err(|_| invalid_controls())?
        .maximum(&offer)
        .map_err(|_| invalid_controls())?;
        Ok(body)
    })
    .await
    .map_err(|_| unavailable_controls())??;
    Ok((body, binding))
}

/// New estimates/admissions must already contain every explicit mapped value.
/// This never changes the body whose hash the retail owner authorized.
pub(super) async fn validate(
    runtime: &Runtime,
    control: Arc<super::super::proxy_control::ProxyControl>,
    request: Arc<proxy_request::Request>,
    adapter: PublicAdapterSnapshot,
    offer: mayhem_proto::proxy::ProxyOffer,
) -> Result<(), ApiError> {
    let selected = proxy_request::resolve_estimate(control.clone(), request.clone())
        .await
        .map_err(selection_error)?;
    super::evidence::check(control.clone(), request.clone(), &selected.published).await?;
    super::data_handling::check(
        control.clone(),
        &runtime.controller,
        request.clone(),
        &selected.published,
    )
    .await
    .map_err(super::data_handling::Failure::api)?;
    if !has_controls(&request) {
        return Ok(());
    }
    let (body, _) = tokio::time::timeout(
        DEADLINE,
        materialize(control, request.clone(), adapter, offer, false),
    )
    .await
    .map_err(|_| unavailable_controls())??;
    if &body != request.provider_value() {
        return Err(invalid_controls());
    }
    Ok(())
}

/// Called only after original-job replay lookup, before any job/negotiation.
pub(super) async fn validate_admission(
    runtime: &Runtime,
    control: Arc<super::super::proxy_control::ProxyControl>,
    request: Arc<proxy_request::Request>,
    candidate: &proxy_request::Candidate,
) -> Result<(), ApiError> {
    if !has_controls(&request)
        && request
            .controls()
            .profile
            .as_ref()
            .is_none_or(|p| !super::evidence::needed(p))
    {
        return Ok(());
    }
    tokio::time::timeout(DEADLINE, async {
        let adapter = runtime
            .controller
            .describe(
                candidate.offer.clone(),
                request.controls().rail,
                request.controls().settlement_policy_hash.clone(),
                &candidate.endpoint_contract,
                &candidate.recipe_hash,
            )
            .await
            .map_err(|_| unavailable_controls())?;
        validate(
            runtime,
            control.clone(),
            request.clone(),
            adapter,
            candidate.offer.clone(),
        )
        .await?;
        let latest = proxy_request::resolve(control, request)
            .await
            .map_err(selection_error)?;
        if latest.offer != candidate.offer
            || latest.endpoint_contract != candidate.endpoint_contract
            || latest.recipe_hash != candidate.recipe_hash
        {
            return Err(unavailable_controls());
        }
        Ok(())
    })
    .await
    .map_err(|_| unavailable_controls())?
}

pub(crate) async fn handle(State(state): State<SharedState>, request: HttpRequest) -> Response {
    let result = tokio::time::timeout(DEADLINE, read(state, request))
        .await
        .unwrap_or_else(|_| Err(unavailable_controls()));
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    response
        .headers_mut()
        .insert("cache-control", "private, no-store".parse().unwrap());
    response
}
async fn read(state: SharedState, http: HttpRequest) -> Result<Response, ApiError> {
    let headers = http.headers().clone();
    if super::super::gateway_bearer_token(&headers)?.is_none() {
        return Err(ApiError::unauthorized(
            "Proxy preparation requires an authenticated key",
            Some("Authorization"),
        ));
    }
    state
        .access_control
        .preauthorize_body_headers_mode(&headers, false)?;
    let _permit = READS.try_acquire().map_err(|_| unavailable_controls())?;
    let runtime = state
        .proxy_buyer
        .as_ref()
        .ok_or_else(unavailable_controls)?;
    let control = state
        .proxy_control()
        .cloned()
        .ok_or_else(unavailable_controls)?;
    let bytes = to_bytes(
        http.into_body(),
        runtime
            .policy
            .request_byte_limit()
            .saturating_add(33 * 1024),
    )
    .await
    .map_err(|_| invalid_controls())?;
    let envelope: Envelope = serde_json::from_slice(&bytes).map_err(|_| invalid_controls())?;
    state
        .authorize_existing_gateway_request(
            &headers,
            envelope.request.get("model").and_then(Value::as_str),
        )?
        .ok_or_else(|| {
            ApiError::unauthorized(
                "Proxy preparation requires an authenticated key",
                Some("Authorization"),
            )
        })?;
    if envelope.schema_version != 1 {
        return Err(invalid_controls());
    }
    let request = Arc::new(
        proxy_request::Request::parse(envelope.endpoint, envelope.request, &runtime.policy)
            .map_err(selection_error)?
            .ok_or_else(invalid_controls)?,
    );
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
        .map_err(|_| unavailable_controls())?;
    let (mut body, registry_release) = materialize(
        control.clone(),
        request.clone(),
        adapter,
        selected.candidate.offer.clone(),
        true,
    )
    .await?;
    let latest = proxy_request::resolve_estimate(control.clone(), request.clone())
        .await
        .map_err(selection_error)?;
    let now = super::super::now_millis_u64();
    let mut expires = selected.expires_at_ms.min(latest.expires_at_ms);
    if selected.network != latest.network
        || selected.published.digest != latest.published.digest
        || selected.published.membership != latest.published.membership
        || expires <= now
    {
        return Err(unavailable_controls());
    }
    let content = retail::request_content_digest(&body).map_err(|_| invalid_controls())?;
    let mut controls = request.controls().clone();
    controls.registry_release = Some(registry_release.clone());
    let mut prepared = body.clone();
    prepared["proxy"] = serde_json::to_value(&controls).map_err(|_| invalid_controls())?;
    let prepared = Arc::new(
        proxy_request::Request::parse(request.endpoint(), prepared, request.policy())
            .map_err(selection_error)?
            .ok_or_else(invalid_controls)?,
    );
    if let Some(record) =
        super::evidence::check(control.clone(), prepared.clone(), &latest.published).await?
    {
        expires = expires.min(record.body.expires_at_ms);
        if expires <= now {
            return Err(unavailable_controls());
        }
    }
    if let Some(record) =
        super::data_handling::check(control, &runtime.controller, prepared, &latest.published)
            .await
            .map_err(super::data_handling::Failure::api)?
    {
        expires = expires.min(record.expires_at_ms);
    }
    let controls_bytes = mayhem_proto::stable_json_bytes(
        &serde_json::to_value(&controls).map_err(|_| invalid_controls())?,
    )
    .map_err(|_| invalid_controls())?;
    let mut response = serde_json::json!({"schema_version":1,"lane":"proxy","kind":"profile_preparation",
        "network":latest.network,"model":request.selector().model(),"endpoint":request.endpoint(),"rail":controls.rail,
        "profile_hash":controls.profile.as_ref().unwrap().digest().map_err(|_| invalid_controls())?,
        "registry_release":registry_release,"controls_hash":digest("mayhem/proxy/profile-controls/v1", &[&controls_bytes]),
        "offer_digest":latest.published.digest,"membership_digest":latest.published.membership.digest().map_err(|_| invalid_controls())?,
        "endpoint_contract":latest.candidate.endpoint_contract,"recipe_hash":latest.candidate.recipe_hash,
        "request_content_digest":content,"observed_at_ms":now,"expires_at_ms":expires});
    let bytes = mayhem_proto::stable_json_bytes(&response).map_err(|_| invalid_controls())?;
    response["preparation_hash"] =
        serde_json::json!(digest("mayhem/proxy/profile-preparation/v1", &[&bytes]));
    body["proxy"] = serde_json::to_value(controls).map_err(|_| invalid_controls())?;
    response["request"] = body;
    Ok(Json(response).into_response())
}
