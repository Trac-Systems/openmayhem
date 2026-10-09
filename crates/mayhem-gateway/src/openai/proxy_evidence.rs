//! Financial evidence is available only through an authenticated gateway-owned
//! route, never through keys found inside an upstream model response.
use super::*;

pub(super) async fn retrieve(
    State(state): State<SharedState>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    // Signature verification and serialization run outside the HTTP executor.
    // Hold a bounded permit even if the HTTP waiter is dropped.
    let permit = match VERIFY.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return ApiError::service_unavailable("proxy evidence reader is busy", None)
                .into_response()
        }
    };
    let read_id = job_id.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<Value, ApiError> {
        let _permit = permit;
        let (model, owner) = {
            let jobs = state.jobs.lock_recover("gateway job vault");
            let job = jobs.proxy_evidence_record(&read_id, now_secs()).ok_or_else(missing)?;
            (job.model.clone(), job.owner_token_id.clone())
        };
        // Same current-key scope, revocation, expiry/rate and owner checks as
        // ordinary job reads; already paid results do not require spare budget.
        let token = state.authorize_existing_gateway_request(&headers, Some(&model))?;
        if token.as_ref().map(|token| token.token_id.as_str()) != owner.as_deref() || owner.is_none() {
            return Err(missing());
        }
        let jobs = state.jobs.lock_recover("gateway job vault");
        let job = jobs.proxy_evidence_record(&read_id, now_secs()).ok_or_else(missing)?;
        if job.model != model || job.owner_token_id != owner {
            return Err(missing());
        }
        crate::job_store::proxy::evidence::project(job)
            .and_then(|evidence| serde_json::to_value(evidence).map_err(|e| e.to_string()))
            .map_err(|_| ApiError::service_unavailable("proxy evidence is unavailable", None))
    })
    .await;
    let value = match result {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => return error.into_response(),
        // Never return private job content or detailed signature failures.
        _ => {
            return ApiError::service_unavailable("proxy evidence is unavailable", None)
                .into_response()
        }
    };
    let mut response = Json(value).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    attach_gateway_job_headers(&mut response, &job_id);
    response
}

static VERIFY: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(8)));

fn missing() -> ApiError {
    ApiError::not_found("proxy job evidence was not found", Some("job_id"))
}
