//! Mutations use the existing dashboard session plus exact configured loopback
//! origin and a per-process CSRF secret. No browser-controlled paths or wallets.
use super::*;
use mayhem_proxy::setup::{Flow, FlowAction};

pub(super) struct Control {
    flow: Arc<Flow>,
    origin: String,
    authority: String,
    csrf: String,
}
impl std::fmt::Debug for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderSetup")
            .field("configured", &true)
            .finish_non_exhaustive()
    }
}
impl Control {
    pub(super) fn new(flow: Flow, origin: &str, seed: &[u8; 32]) -> Result<Self, String> {
        let url = url::Url::parse(origin).map_err(|_| "invalid setup origin")?;
        let host = url.host_str().unwrap_or("").trim_matches(['[', ']']);
        let ip: std::net::IpAddr = host
            .parse()
            .map_err(|_| "setup origin requires a literal loopback address")?;
        if !ip.is_loopback()
            || url.scheme() != "http"
            || url.origin().ascii_serialization() != origin
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("setup origin must be an exact loopback HTTP origin".into());
        }
        let key = ed25519_dalek::SigningKey::from_bytes(seed);
        if hex::encode(key.verifying_key().to_bytes()) != flow.provider().as_str() {
            return Err("setup provider must match the already loaded wallet".into());
        }
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| "setup CSRF entropy unavailable")?;
        Ok(Self {
            flow: Arc::new(flow),
            authority: origin.strip_prefix("http://").unwrap().into(),
            origin: origin.into(),
            csrf: hex::encode(nonce),
        })
    }
    pub(super) fn validate_bind(&self, bind: SocketAddr) -> Result<(), String> {
        if !bind.ip().is_loopback() || format!("http://{bind}") != self.origin {
            return Err("setup listener must match its configured loopback origin".into());
        }
        Ok(())
    }
    pub(super) fn origin(&self) -> &str {
        &self.origin
    }
    pub(super) fn host(&self, headers: &HeaderMap) -> bool {
        headers.get_all(header::HOST).iter().count() == 1
            && headers.get(header::HOST).and_then(|v| v.to_str().ok())
                == Some(self.authority.as_str())
    }
}
fn failure(status: StatusCode, code: &str) -> Response {
    dashboard_json_response(status, json!({"error":code}), None)
}
fn access<'a>(state: &'a GatewayState, headers: &HeaderMap) -> Result<&'a Control, Response> {
    let control = state
        .proxy_setup
        .as_ref()
        .as_ref()
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "setup_disabled"))?;
    if !control.host(headers) {
        return Err(failure(StatusCode::FORBIDDEN, "setup_host_mismatch"));
    }
    Ok(control)
}
pub(super) async fn page(
    State(state): State<SharedState>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<DashboardQuery>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = access(&state, &headers) {
        return response;
    }
    let Some(auth) = dashboard_request_authorized(&state, &headers, query.token.as_deref()) else {
        return dashboard_unauthorized_html_response(dashboard_locked_html(), &headers);
    };
    if let Some(response) = dashboard_bootstrap_redirect(&uri, &auth) {
        return response;
    }
    dashboard_html_response(
        StatusCode::OK,
        dashboard_html_document("Provider setup", include_str!("proxy_setup.html")),
        Some((&auth.cookie_name, &auth.browser_token)),
    )
}
pub(super) async fn script(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    if let Err(response) = access(&state, &headers) {
        return response;
    }
    if dashboard_request_authorized(&state, &headers, None).is_none() {
        return failure(StatusCode::UNAUTHORIZED, "dashboard_session_required");
    }
    let mut response = include_str!("proxy_setup.js").into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/javascript; charset=utf-8"),
    );
    with_dashboard_security_headers(response)
}
pub(super) async fn view(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    let control = match access(&state, &headers) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(auth) = dashboard_request_authorized(&state, &headers, None) else {
        return failure(StatusCode::UNAUTHORIZED, "dashboard_session_required");
    };
    let flow = control.flow.clone();
    match tokio::task::spawn_blocking(move || flow.view()).await {
        Ok(Ok(view)) => dashboard_json_response(
            StatusCode::OK,
            json!({"csrf":control.csrf,"view":view}),
            Some((&auth.cookie_name, &auth.browser_token)),
        ),
        _ => failure(StatusCode::CONFLICT, "setup_unavailable_inspect_original"),
    }
}
pub(super) async fn action(State(state): State<SharedState>, request: Request) -> Response {
    let control = match access(&state, request.headers()) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if request.uri().query().is_some()
        || dashboard_request_authorized(&state, request.headers(), None).is_none()
    {
        return failure(StatusCode::UNAUTHORIZED, "dashboard_session_required");
    }
    let headers = request.headers();
    if headers.get_all(header::ORIGIN).iter().count() != 1
        || headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
            != Some(control.origin.as_str())
        || headers.get_all("x-mayhem-setup-csrf").iter().count() != 1
        || headers
            .get("x-mayhem-setup-csrf")
            .and_then(|v| v.to_str().ok())
            != Some(control.csrf.as_str())
        || headers
            .get("sec-fetch-site")
            .is_some_and(|v| v != "same-origin")
    {
        return failure(StatusCode::FORBIDDEN, "setup_origin_or_csrf_mismatch");
    }
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap().trim())
        != Some("application/json")
        || headers.contains_key(header::CONTENT_ENCODING)
    {
        return failure(StatusCode::UNSUPPORTED_MEDIA_TYPE, "setup_json_required");
    }
    // Authorization and origin checks occur before body consumption or parsing.
    let bytes = match tokio::time::timeout(
        Duration::from_secs(5),
        axum::body::to_bytes(request.into_body(), 64 * 1024),
    )
    .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(_)) => return failure(StatusCode::PAYLOAD_TOO_LARGE, "setup_body_limit"),
        Err(_) => return failure(StatusCode::REQUEST_TIMEOUT, "setup_body_timeout"),
    };
    let action = match serde_json::from_slice::<FlowAction>(&bytes) {
        Ok(v) => v,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "setup_invalid_action"),
    };
    let key = ed25519_dalek::SigningKey::from_bytes(&state.receipt_config.user_seed);
    match control.flow.execute(action, Some(&key)).await {
        Ok(value) => dashboard_json_response(StatusCode::OK, json!(value), None),
        Err(error) => failure(StatusCode::CONFLICT, &error.to_string()),
    }
}
#[cfg(test)]
mod tests;
