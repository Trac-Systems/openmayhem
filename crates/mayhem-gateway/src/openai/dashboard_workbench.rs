use super::*;
use crate::HeartbeatModalityCapacity;
use std::path::Path as FsPath;
use std::sync::atomic::{AtomicU64, Ordering};

const WORKBENCH_SCENARIO_COOKIE: &str = "mayhem_dashboard_workbench_scenario";
const WORKBENCH_HEARTBEAT_TTL_MILLIS: u64 = 7 * 24 * 60 * 60 * 1_000;
const WORKBENCH_SCALE_MODEL_COUNT: usize = 96;
const WORKBENCH_SCALE_RECEIPT_COUNT: usize = 96;

const WORKBENCH_CSS: &str = r#"
.workbench-dock{position:relative;z-index:20;width:auto;margin:16px clamp(18px,3.1vw,52px) 0;padding:10px 12px;border:1px solid var(--app-border-strong);border-radius:12px;background:rgba(16,18,23,.96);box-shadow:0 8px 28px rgba(0,0,0,.28);font-size:12px}
.workbench-dock-head{display:flex;align-items:center;justify-content:space-between;gap:16px;margin-bottom:8px}.workbench-dock strong{color:var(--app-text)}.workbench-dock a{color:var(--app-text);text-decoration:none}.workbench-dock .workbench-home{color:var(--app-accent-strong)}.workbench-scenarios{display:flex;gap:6px;overflow-x:auto;padding-bottom:2px}.workbench-scenario{flex:0 0 auto;padding:6px 9px;border:1px solid var(--app-border);border-radius:8px}.workbench-scenario.active{border-color:var(--app-accent);color:var(--app-accent-strong)}
.workbench-index{width:100%;margin:0;padding:48px clamp(18px,3.1vw,52px) 96px}.workbench-index-header{max-width:760px;margin-bottom:32px}.workbench-index-header h1{font-size:42px;margin:8px 0 12px}.workbench-index-header p{color:var(--app-text-muted);font-size:16px}.workbench-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,360px),1fr));gap:18px}.workbench-card{border:1px solid var(--app-border);border-radius:16px;background:var(--app-panel);padding:20px}.workbench-card h2{margin:4px 0 8px;font-size:20px}.workbench-card p{min-height:0;margin:0 0 18px;color:var(--app-text-muted)}.workbench-links{display:flex;gap:8px;flex-wrap:wrap}.workbench-links a{min-height:44px;display:inline-flex;align-items:center;padding:8px 10px;border:1px solid var(--app-border);border-radius:10px;color:var(--app-text);text-decoration:none}.workbench-links a:hover{border-color:var(--app-accent);color:var(--app-accent-strong)}.workbench-note{margin-top:24px;padding:16px;border-left:3px solid var(--app-accent);background:rgba(255,107,122,.08);color:var(--app-text-muted)}
@media(max-width:760px){.workbench-grid{grid-template-columns:1fr}.workbench-index{padding:32px 18px 112px}.workbench-index-header h1{font-size:34px}.workbench-dock{margin:10px 18px 0}.workbench-dock-head{align-items:center}}
"#;

const WORKBENCH_RELOAD_JS: &str = r#"
(() => {
  let currentVersion = null;
  async function checkVersion() {
    try {
      const response = await fetch('/__workbench/version', { cache: 'no-store' });
      if (!response.ok) return;
      const nextVersion = await response.text();
      if (currentVersion !== null && currentVersion !== nextVersion) {
        window.location.reload();
        return;
      }
      currentVersion = nextVersion;
    } catch (_) {
      // The watcher briefly takes the server down while swapping binaries.
    }
  }
  checkVersion();
  window.setInterval(checkVersion, 750);
})();
"#;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum WorkbenchScenario {
    Showcase,
    AuthRequired,
    Empty,
    Loading,
    Failure,
    Offline,
    SourceUpdate,
    SignedUpdate,
    UpdateRequired,
    Scale,
}

impl WorkbenchScenario {
    const ALL: [Self; 10] = [
        Self::Showcase,
        Self::AuthRequired,
        Self::Empty,
        Self::Loading,
        Self::Failure,
        Self::Offline,
        Self::SourceUpdate,
        Self::SignedUpdate,
        Self::UpdateRequired,
        Self::Scale,
    ];

    fn id(self) -> &'static str {
        match self {
            Self::Showcase => "showcase",
            Self::AuthRequired => "auth-required",
            Self::Empty => "empty",
            Self::Loading => "loading",
            Self::Failure => "failure",
            Self::Offline => "offline",
            Self::SourceUpdate => "source-update",
            Self::SignedUpdate => "signed-update",
            Self::UpdateRequired => "update-required",
            Self::Scale => "scale",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Showcase => "Showcase",
            Self::AuthRequired => "Credential required",
            Self::Empty => "Empty state",
            Self::Loading => "Provider loading",
            Self::Failure => "Provider failure",
            Self::Offline => "Routes offline",
            Self::SourceUpdate => "Source update",
            Self::SignedUpdate => "Signed update",
            Self::UpdateRequired => "Update required",
            Self::Scale => "Scale and overflow",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Showcase => "Populated balances, sessions, providers, earnings, capabilities, and price history.",
            Self::AuthRequired => "Authentication is required and no active access token exists yet.",
            Self::Empty => "No catalog, routes, sessions, earnings, or access tokens.",
            Self::Loading => "A provider downloading and verifying a model before its route exists.",
            Self::Failure => "A provider load that failed during artifact verification.",
            Self::Offline => "Canonical routes exist, but no provider heartbeat is currently live.",
            Self::SourceUpdate => "Newer GitHub source is available without a signed executable for this system.",
            Self::SignedUpdate => "A newer GitHub release includes the complete signed executable asset set.",
            Self::UpdateRequired => "A catalog compatibility gate hides a model until the app is updated.",
            Self::Scale => "Ninety-six models, more than one hundred provider-scoped routes, ninety-six receipts, sixty-four access tokens, and sixty-four probes for testing reachability, renderer bounds, density, and mobile overflow.",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|scenario| scenario.id().eq_ignore_ascii_case(value.trim()))
    }
}

#[derive(Clone)]
struct WorkbenchState {
    scenarios: Arc<BTreeMap<WorkbenchScenario, GatewayState>>,
    version: Arc<str>,
    request_sequence: Arc<AtomicU64>,
}

impl WorkbenchState {
    fn new() -> io::Result<Self> {
        let mut scenarios = BTreeMap::new();
        scenarios.insert(WorkbenchScenario::Showcase, showcase_state(4));
        scenarios.insert(WorkbenchScenario::AuthRequired, auth_required_state());
        scenarios.insert(
            WorkbenchScenario::Empty,
            GatewayState::from_models(Vec::new()),
        );
        scenarios.insert(
            WorkbenchScenario::Loading,
            progress_state("loading", "download", "running", 68)?,
        );
        scenarios.insert(
            WorkbenchScenario::Failure,
            progress_state("failure", "verify", "error", 43)?,
        );
        scenarios.insert(
            WorkbenchScenario::Offline,
            base_state(workbench_models(4), false),
        );
        scenarios.insert(WorkbenchScenario::SourceUpdate, github_update_state(false));
        scenarios.insert(WorkbenchScenario::SignedUpdate, github_update_state(true));
        scenarios.insert(
            WorkbenchScenario::UpdateRequired,
            showcase_state(3).with_hidden_update_models(vec![GatewayUpdateModelNotice {
                model_id: "workbench/catalog-next".to_owned(),
                min_app_version: "9999.0.0".to_owned(),
                installed_app_version: installed_app_version().to_owned(),
                message: "Workbench fixture for a catalog compatibility gate".to_owned(),
            }]),
        );
        scenarios.insert(WorkbenchScenario::Scale, scale_state());

        let version = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_string();
        Ok(Self {
            scenarios: Arc::new(scenarios),
            version: Arc::from(version),
            request_sequence: Arc::new(AtomicU64::new(1)),
        })
    }

    fn gateway(&self, scenario: WorkbenchScenario) -> GatewayState {
        self.scenarios
            .get(&scenario)
            .cloned()
            .unwrap_or_else(GatewayState::fixture)
    }
}

fn github_update_state(installable: bool) -> GatewayState {
    let update = if installable {
        GatewayGithubUpdate {
            kind: "release".to_owned(),
            installed_version: installed_app_version().to_owned(),
            available_version: Some("0.3.0".to_owned()),
            installed_revision: Some("1".repeat(40)),
            available_revision: None,
            release_url: Some(
                "https://github.com/Trac-Systems/openmayhem/releases/tag/0.3.0".to_owned(),
            ),
            compare_url: None,
            published_at: Some("2026-07-18T12:00:00Z".to_owned()),
            installable: true,
            message: "Mayhem 0.3.0 is available as a signed update for this system.".to_owned(),
        }
    } else {
        GatewayGithubUpdate {
            kind: "source".to_owned(),
            installed_version: installed_app_version().to_owned(),
            available_version: None,
            installed_revision: Some("1".repeat(40)),
            available_revision: Some("2".repeat(40)),
            release_url: None,
            compare_url: Some(
                "https://github.com/Trac-Systems/openmayhem/compare/source...main".to_owned(),
            ),
            published_at: None,
            installable: false,
            message: "3 newer source changes are available on GitHub.".to_owned(),
        }
    };
    showcase_state(4).with_github_update_status(GatewayGithubUpdateStatus {
        state: "available".to_owned(),
        installed_version: installed_app_version().to_owned(),
        installed_revision: Some("1".repeat(40)),
        checked_at_seconds: Some(now_secs()),
        message: update.message.clone(),
        update: Some(update),
    })
}

#[derive(Debug, Default, Deserialize)]
struct WorkbenchQuery {
    scenario: Option<String>,
    page: Option<String>,
    probe_page: Option<String>,
    kind: Option<String>,
    id: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    enclave: Option<String>,
    room: Option<String>,
    rail: Option<String>,
    fresh_evidence: Option<bool>,
}

impl WorkbenchQuery {
    fn dashboard_query(&self) -> DashboardQuery {
        DashboardQuery {
            token: None,
            page: self.page.clone(),
            probe_page: self.probe_page.clone(),
            kind: self.kind.clone(),
            id: self.id.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            enclave: self.enclave.clone(),
            room: self.room.clone(),
            rail: self.rail.clone(),
        }
    }
}

fn gateway_for_query(
    state: &WorkbenchState,
    scenario: WorkbenchScenario,
    query: &WorkbenchQuery,
) -> GatewayState {
    let mut gateway = state.gateway(scenario);
    if query.fresh_evidence != Some(true) {
        return gateway;
    }
    if let Some(mut payment_directory) = gateway.payment_directory() {
        if let Some(object) = payment_directory.as_object_mut() {
            object.insert("observed_at".to_owned(), json!(now_secs()));
        }
        gateway = gateway.with_payment_directory(payment_directory);
    }
    let earnings = gateway.provider_earnings_snapshot().entries;
    gateway.with_provider_earnings(earnings)
}

pub fn dashboard_workbench_router() -> io::Result<Router> {
    let state = WorkbenchState::new()?;
    Ok(Router::new()
        .route("/", get(workbench_index))
        .route("/mayhem/dashboard", get(workbench_user))
        .route("/mayhem/dashboard/", get(workbench_dashboard_redirect))
        .route("/mayhem/dashboard/provider", get(workbench_provider))
        .route("/mayhem/dashboard/network", get(workbench_network))
        .route("/mayhem/dashboard/evidence", get(workbench_evidence))
        .route(
            "/mayhem/dashboard/assets/exo-latin.woff2",
            get(workbench_font),
        )
        .route("/mayhem/dashboard/assets/app.css", get(workbench_app_css))
        .route("/mayhem/dashboard/assets/app.js", get(workbench_app_js))
        .route(
            "/mayhem/dashboard/assets/brand/{asset}",
            get(workbench_brand_asset),
        )
        .route("/mayhem/dashboard/{*page}", get(workbench_product_page))
        .route("/__workbench/version", get(workbench_version))
        .route("/__workbench/reload.js", get(workbench_reload_script))
        .route("/__workbench/health", get(workbench_health))
        .route("/v1/chat/completions", post(workbench_chat_completions))
        .route("/v1/images/generations", post(workbench_image_generation))
        .route("/v1/audio/speech", post(workbench_audio_speech))
        .with_state(state))
}

async fn workbench_index(State(state): State<WorkbenchState>) -> Response {
    let cards = WorkbenchScenario::ALL
        .into_iter()
        .map(|scenario| {
            format!(
                r#"<article class="workbench-card"><span class="label">Scenario</span><h2>{}</h2><p>{}</p><div class="workbench-links"><a href="/mayhem/dashboard?scenario={}">User</a><a href="/mayhem/dashboard/provider?scenario={}">Provider</a><a href="/mayhem/dashboard/network?scenario={}">Network</a></div></article>"#,
                html_escape(scenario.title()),
                html_escape(scenario.description()),
                scenario.id(),
                scenario.id(),
                scenario.id(),
            )
        })
        .collect::<String>();
    let body = format!(
        r#"<main class="workbench-index"><header class="workbench-index-header"><span class="label">Local development</span><h1>Dashboard workbench</h1><p>Real Mayhem dashboard rendering with deterministic fixture states. No peer, ledger, payments, model downloads, or inference workers are running.</p></header><section class="workbench-grid">{cards}</section><aside class="workbench-note">Choose a scenario and surface. When the watcher swaps in a successful rebuild, open previews reload automatically.</aside><p class="privacy-note mono">preview version {}</p></main>"#,
        html_escape(&state.version),
    );
    let html = enhance_workbench_html(dashboard_html_document("Workbench", &body), None, None);
    dashboard_html_response(StatusCode::OK, html, None)
}

async fn workbench_user(
    State(state): State<WorkbenchState>,
    Query(query): Query<WorkbenchQuery>,
    headers: HeaderMap,
) -> Response {
    let scenario = selected_scenario(&query, &headers);
    let gateway = gateway_for_query(&state, scenario, &query);
    let dashboard_query = query.dashboard_query();
    let origin = dashboard_origin_from_headers(&headers);
    let html = render_dashboard_product_page(
        &gateway,
        &origin,
        &dashboard_query,
        DashboardProductPage::Home,
    );
    workbench_product_response(html, scenario, "/mayhem/dashboard")
}

async fn workbench_dashboard_redirect() -> Response {
    let mut response = StatusCode::PERMANENT_REDIRECT.into_response();
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_static("/mayhem/dashboard"),
    );
    with_dashboard_security_headers(response)
}

async fn workbench_evidence(
    State(state): State<WorkbenchState>,
    Query(query): Query<WorkbenchQuery>,
    headers: HeaderMap,
) -> Response {
    let scenario = selected_scenario(&query, &headers);
    let gateway = state.gateway(scenario);
    let dashboard_query = query.dashboard_query();
    let Some(payload) = dashboard_evidence_payload(&gateway, &dashboard_query) else {
        let response = if dashboard_wants_json(&headers) {
            dashboard_json_response(
                StatusCode::NOT_FOUND,
                json!({"ok": false, "error": "dashboard_evidence_not_found"}),
                None,
            )
        } else {
            dashboard_html_response(
                StatusCode::NOT_FOUND,
                dashboard_html_document(
                    "Evidence not found",
                    r#"<main class="evidence-standalone"><section class="panel"><div class="empty-block"><div class="empty-block-inner"><h1>Evidence not found</h1><p>The fixture does not contain the requested record.</p><a class="primary-button" href="/mayhem/dashboard">Return to dashboard</a></div></div></section></main>"#,
                ),
                None,
            )
        };
        return with_workbench_scenario_cookie(response, scenario);
    };
    let response = if dashboard_wants_json(&headers) {
        dashboard_json_response(StatusCode::OK, payload, None)
    } else {
        let html = enhance_workbench_html(
            render_dashboard_evidence_page(&payload),
            Some(scenario),
            Some("/mayhem/dashboard"),
        );
        dashboard_html_response(StatusCode::OK, html, None)
    };
    with_workbench_scenario_cookie(response, scenario)
}

async fn workbench_provider(
    State(state): State<WorkbenchState>,
    Query(query): Query<WorkbenchQuery>,
    headers: HeaderMap,
) -> Response {
    let scenario = selected_scenario(&query, &headers);
    let gateway = gateway_for_query(&state, scenario, &query);
    let dashboard_query = query.dashboard_query();
    let origin = dashboard_origin_from_headers(&headers);
    let html = render_dashboard_product_page(
        &gateway,
        &origin,
        &dashboard_query,
        DashboardProductPage::Earn,
    );
    workbench_product_response(html, scenario, "/mayhem/dashboard/provider")
}

async fn workbench_network(
    State(state): State<WorkbenchState>,
    Query(query): Query<WorkbenchQuery>,
    headers: HeaderMap,
) -> Response {
    let scenario = selected_scenario(&query, &headers);
    let gateway = gateway_for_query(&state, scenario, &query);
    let dashboard_query = query.dashboard_query();
    let origin = dashboard_origin_from_headers(&headers);
    let html = render_dashboard_product_page(
        &gateway,
        &origin,
        &dashboard_query,
        DashboardProductPage::Network,
    );
    workbench_product_response(html, scenario, "/mayhem/dashboard/network")
}

async fn workbench_product_page(
    State(state): State<WorkbenchState>,
    Path(page): Path<String>,
    Query(query): Query<WorkbenchQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(product_page) = DashboardProductPage::from_path(&page) else {
        return dashboard_html_response(
            StatusCode::NOT_FOUND,
            dashboard_html_document(
                "Workbench page not found",
                r#"<main class="evidence-standalone"><section class="panel"><div class="empty-block"><div class="empty-block-inner"><div class="empty-symbol" aria-hidden="true">&mdash;</div><h1>Workbench page not found</h1><p>The requested preview route does not exist.</p><a class="primary-button" href="/">Return to previews</a></div></div></section></main>"#,
            ),
            None,
        );
    };
    let scenario = selected_scenario(&query, &headers);
    let gateway = gateway_for_query(&state, scenario, &query);
    let dashboard_query = query.dashboard_query();
    let origin = dashboard_origin_from_headers(&headers);
    let html = render_dashboard_product_page(&gateway, &origin, &dashboard_query, product_page);
    let base_path = format!("/mayhem/dashboard/{}", page.trim_matches('/'));
    workbench_product_response(html, scenario, &base_path)
}

fn selected_scenario(query: &WorkbenchQuery, headers: &HeaderMap) -> WorkbenchScenario {
    query
        .scenario
        .as_deref()
        .and_then(WorkbenchScenario::parse)
        .or_else(|| {
            dashboard_cookie_named(headers, WORKBENCH_SCENARIO_COOKIE)
                .and_then(WorkbenchScenario::parse)
        })
        .unwrap_or(WorkbenchScenario::Showcase)
}

fn with_workbench_scenario_cookie(mut response: Response, scenario: WorkbenchScenario) -> Response {
    let scenario_cookie = format!(
        "{WORKBENCH_SCENARIO_COOKIE}={}; Path=/; Max-Age=604800; SameSite=Lax",
        scenario.id()
    );
    if let Ok(value) = HeaderValue::from_str(&scenario_cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

fn workbench_product_response(
    html: String,
    scenario: WorkbenchScenario,
    base_path: &str,
) -> Response {
    let html = enhance_workbench_html(html, Some(scenario), Some(base_path));
    let response = dashboard_html_response(StatusCode::OK, html, None);
    with_workbench_scenario_cookie(response, scenario)
}

fn enhance_workbench_html(
    mut html: String,
    scenario: Option<WorkbenchScenario>,
    base_path: Option<&str>,
) -> String {
    html = html.replacen(
        "</head>",
        &format!("<style>{WORKBENCH_CSS}</style></head>"),
        1,
    );
    if let (Some(scenario), Some(base_path)) = (scenario, base_path) {
        let links = WorkbenchScenario::ALL
            .into_iter()
            .map(|candidate| {
                let active = if candidate == scenario { " active" } else { "" };
                format!(
                    r#"<a class="workbench-scenario{active}" href="{}?scenario={}">{}</a>"#,
                    base_path,
                    candidate.id(),
                    html_escape(candidate.title()),
                )
            })
            .collect::<String>();
        let dock = format!(
            r#"<aside class="workbench-dock" data-workbench-chrome><div class="workbench-dock-head"><strong>Fixture: {}</strong><a class="workbench-home" href="/">All previews</a></div><div class="workbench-scenarios">{links}</div></aside>"#,
            html_escape(scenario.title()),
        );
        html = html.replacen(
            "<body>",
            &format!(r#"<body class="has-workbench">{dock}"#),
            1,
        );
    }
    html.replacen(
        "</body>",
        r#"<script src="/__workbench/reload.js"></script></body>"#,
        1,
    )
}

async fn workbench_font() -> Response {
    let mut response = Response::new(Body::from(DASHBOARD_EXO_LATIN_WOFF2.to_vec()));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("font/woff2"));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    with_dashboard_security_headers(response)
}

async fn workbench_app_css() -> Response {
    dashboard_asset_response("text/css; charset=utf-8", DASHBOARD_APP_CSS)
}

async fn workbench_app_js() -> Response {
    dashboard_asset_response("text/javascript; charset=utf-8", DASHBOARD_APP_JS)
}

async fn workbench_brand_asset(Path(asset): Path<String>) -> Response {
    let Some((content_type, bytes)) = dashboard_brand_asset(&asset) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response = Response::new(Body::from(bytes));
    if let Ok(value) = HeaderValue::from_str(content_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    with_dashboard_security_headers(response)
}

async fn workbench_version(State(state): State<WorkbenchState>) -> Response {
    let mut response = Response::new(Body::from(state.version.to_string()));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn workbench_reload_script() -> Response {
    let mut response = Response::new(Body::from(WORKBENCH_RELOAD_JS));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/javascript; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    with_dashboard_security_headers(response)
}

async fn workbench_health(State(state): State<WorkbenchState>) -> Response {
    Json(json!({
        "ok": true,
        "fixture_only": true,
        "scenarios": WorkbenchScenario::ALL.map(WorkbenchScenario::id),
        "version": state.version.as_ref(),
    }))
    .into_response()
}

async fn workbench_chat_completions(
    State(state): State<WorkbenchState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    let scenario = selected_scenario(&WorkbenchQuery::default(), &headers);
    let gateway = state.gateway(scenario);
    let Some(requested_model) = payload
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return workbench_chat_error(
            scenario,
            StatusCode::BAD_REQUEST,
            "fixture_model_required",
            "Choose a fixture model before sending.",
        );
    };

    if let Some((status, code, message)) = workbench_scenario_blocker(scenario) {
        return workbench_chat_error(scenario, status, code, message);
    }

    let include_usage = match workbench_include_usage(&payload) {
        Ok(include_usage) => include_usage,
        Err((code, message)) => {
            return workbench_chat_error(scenario, StatusCode::BAD_REQUEST, code, message);
        }
    };

    if gateway.update_notice().is_some_and(|notice| {
        notice
            .models
            .iter()
            .any(|model| model.model_id == requested_model)
    }) {
        return workbench_chat_error(
            scenario,
            StatusCode::UPGRADE_REQUIRED,
            "fixture_update_required",
            format!(
                "The fixture model {requested_model} is hidden until the required app update is installed."
            ),
        );
    }

    let models = gateway.models_snapshot();
    let Some(model) = models.iter().find(|model| model.id == requested_model) else {
        return workbench_chat_error(
            scenario,
            StatusCode::NOT_FOUND,
            "fixture_model_not_found",
            format!("The fixture catalog does not contain model {requested_model}."),
        );
    };
    let Some(candidate) = workbench_serving_candidate(&gateway, model) else {
        return workbench_chat_error(
            scenario,
            StatusCode::SERVICE_UNAVAILABLE,
            "fixture_no_eligible_route",
            format!(
                "No fresh fixture route is accepting requests for model {}.",
                model.id
            ),
        );
    };

    let prompt = payload
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.last())
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("your request");
    let length_limited = payload.get("max_tokens").and_then(Value::as_u64) == Some(64);
    let excerpt = short_text(prompt, 80);
    let content = if length_limited {
        let tokens = (1..=64)
            .map(|index| format!("preview-{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!("Workbench output-limit fixture from {}: {tokens}", model.id)
    } else {
        format!(
            "Workbench response from {}. This deterministic preview received: {excerpt}",
            model.id
        )
    };
    let sequence = state.request_sequence.fetch_add(1, Ordering::Relaxed);
    let session_id = format!("workbench-live-{sequence:016x}");
    let created = now_secs();
    let mut receipt = live_fixture_receipt(model, &candidate, sequence, &session_id, prompt);
    if length_limited {
        let input_tokens = receipt.receipt.body.usage.input_tokens();
        let usage = ReceiptUsage::text(input_tokens, 64);
        receipt.receipt.body.au_owed_cum = calculate_au_owed(&model.mayhem.price_ref_au, &usage);
        receipt.receipt.body.usage = usage;
    }
    let prompt_tokens = receipt.receipt.body.usage.input_tokens();
    let completion_tokens = receipt.receipt.body.usage.output_tokens();
    let usage = Usage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens.saturating_add(completion_tokens),
    };
    let usage_value = chat_usage_value(
        &usage,
        (scenario == WorkbenchScenario::Showcase).then_some(&receipt),
    );
    let receipt_value =
        (scenario == WorkbenchScenario::Showcase).then(|| receipt_summary(&receipt));

    if scenario == WorkbenchScenario::Showcase && gateway.record_workbench_receipt(receipt).is_err()
    {
        return workbench_chat_error(
            scenario,
            StatusCode::INTERNAL_SERVER_ERROR,
            "fixture_receipt_record_failed",
            "The deterministic response was not returned because its fixture receipt could not be recorded.",
        );
    }

    let event = json!({
        "id": session_id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model.id,
        "choices": [{"index": 0, "delta": {"content": content}, "finish_reason": Value::Null}],
    });
    let mut body = format!("data: {event}\n\n");
    let finish_event = json!({
        "id": session_id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model.id,
        "choices": [{"index": 0, "delta": {}, "finish_reason": if length_limited { "length" } else { "stop" }}],
    });
    body.push_str(&format!("data: {finish_event}\n\n"));
    if include_usage {
        let usage_event = json!({
            "id": session_id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model.id,
            "choices": [],
            "usage": usage_value,
            "mayhem": {"receipt": receipt_value},
        });
        body.push_str(&format!("data: {usage_event}\n\n"));
    }
    body.push_str("data: [DONE]\n\n");
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    with_workbench_scenario_cookie(with_dashboard_security_headers(response), scenario)
}

async fn workbench_image_generation(
    State(state): State<WorkbenchState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    let scenario = selected_scenario(&WorkbenchQuery::default(), &headers);
    if let Some((status, code, message)) = workbench_scenario_blocker(scenario) {
        return workbench_chat_error(scenario, status, code, message);
    }
    let requested_model = payload
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let gateway = state.gateway(scenario);
    let Some(model) = gateway
        .models_snapshot()
        .iter()
        .find(|model| model.id == requested_model && model.mayhem.model_class == "image-generation")
        .cloned()
    else {
        return workbench_chat_error(
            scenario,
            StatusCode::BAD_REQUEST,
            "fixture_image_model_required",
            "Choose an image-generation fixture model.",
        );
    };
    if workbench_serving_candidate(&gateway, &model).is_none() {
        return workbench_chat_error(
            scenario,
            StatusCode::SERVICE_UNAVAILABLE,
            "fixture_no_eligible_route",
            "No fresh fixture route is accepting this image request.",
        );
    }
    let sequence = state.request_sequence.fetch_add(1, Ordering::Relaxed);
    let id = format!("workbench-image-{sequence:016x}");
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="768" height="768" viewBox="0 0 768 768"><defs><radialGradient id="g" cx="70%" cy="22%" r="85%"><stop stop-color="#ff8a96"/><stop offset=".42" stop-color="#7c456e"/><stop offset="1" stop-color="#11131a"/></radialGradient><filter id="n"><feTurbulence baseFrequency=".7" numOctaves="2" seed="7"/><feColorMatrix values="1 0 0 0 0 0 1 0 0 0 0 0 1 0 0 0 0 0 .09 0"/></filter></defs><rect width="768" height="768" rx="44" fill="url(#g)"/><rect width="768" height="768" rx="44" filter="url(#n)" opacity=".5"/><circle cx="590" cy="170" r="92" fill="#ffd7c7" opacity=".8"/><path d="M0 575 170 385l104 92 105-145 108 130 97-78 184 191v193H0Z" fill="#11131a" opacity=".82"/><path d="m0 632 208-145 126 94 98-61 148 112 188-96v232H0Z" fill="#171b24" opacity=".9"/><text x="48" y="80" fill="#fff" font-family="system-ui,sans-serif" font-size="25" font-weight="700">OpenMayhem workbench</text><text x="48" y="113" fill="#f7c7cb" font-family="system-ui,sans-serif" font-size="17">Deterministic image-generation fixture</text></svg>"##;
    let encoded = BASE64_STANDARD.encode(svg.as_bytes());
    let payload = json!({
        "id": id,
        "object": "images.response",
        "created": now_secs(),
        "model": model.id,
        "data": [{
            "b64_json": encoded,
            "revised_prompt": Value::Null,
            "mayhem": {
                "artifact_id": format!("workbench-artifact-{sequence:016x}"),
                "content_type": "image/svg+xml",
                "blake3": hex_fill(0xd1),
            }
        }],
        "usage": {"image": 1, "step": 1},
        "mayhem": {
            "backend": "workbench-image-fixture",
            "direct_session": false,
            "billable": false,
            "dev_session": true,
            "receipt": Value::Null,
        }
    });
    let response = Json(payload).into_response();
    with_workbench_scenario_cookie(with_dashboard_security_headers(response), scenario)
}

fn workbench_silence_wav() -> Vec<u8> {
    const SAMPLE_RATE: u32 = 8_000;
    const SAMPLE_COUNT: u32 = 2_000;
    let data_len = SAMPLE_COUNT * 2;
    let mut wav = Vec::with_capacity((44 + data_len) as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.resize((44 + data_len) as usize, 0);
    wav
}

async fn workbench_audio_speech(
    State(state): State<WorkbenchState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    let scenario = selected_scenario(&WorkbenchQuery::default(), &headers);
    if let Some((status, code, message)) = workbench_scenario_blocker(scenario) {
        return workbench_chat_error(scenario, status, code, message);
    }
    let requested_model = payload
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let gateway = state.gateway(scenario);
    let Some(model) = gateway
        .models_snapshot()
        .iter()
        .find(|model| model.id == requested_model && model.mayhem.model_class == "tts")
        .cloned()
    else {
        return workbench_chat_error(
            scenario,
            StatusCode::BAD_REQUEST,
            "fixture_speech_model_required",
            "Choose a speech fixture model.",
        );
    };
    if workbench_serving_candidate(&gateway, &model).is_none() {
        return workbench_chat_error(
            scenario,
            StatusCode::SERVICE_UNAVAILABLE,
            "fixture_no_eligible_route",
            "No fresh fixture route is accepting this speech request.",
        );
    }
    let mut response = Response::new(Body::from(workbench_silence_wav()));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"));
    response.headers_mut().insert(
        "x-mayhem-backend",
        HeaderValue::from_static("workbench-speech-fixture"),
    );
    response
        .headers_mut()
        .insert("x-mayhem-direct-session", HeaderValue::from_static("false"));
    with_workbench_scenario_cookie(with_dashboard_security_headers(response), scenario)
}

fn workbench_include_usage(payload: &Value) -> Result<bool, (&'static str, &'static str)> {
    let Some(stream_options) = payload.get("stream_options") else {
        return Ok(false);
    };
    if stream_options.is_null() {
        return Ok(false);
    }
    let Some(stream_options) = stream_options.as_object() else {
        return Err((
            "fixture_invalid_stream_options",
            "stream_options must be an object when provided.",
        ));
    };
    if stream_options
        .keys()
        .any(|option| option != "include_usage")
    {
        return Err((
            "fixture_unsupported_stream_option",
            "The workbench accepts only stream_options.include_usage, matching the gateway contract.",
        ));
    }
    match stream_options.get("include_usage") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(include_usage)) => Ok(*include_usage),
        Some(_) => Err((
            "fixture_invalid_include_usage",
            "stream_options.include_usage must be a boolean when provided.",
        )),
    }
}

fn workbench_scenario_blocker(
    scenario: WorkbenchScenario,
) -> Option<(StatusCode, &'static str, &'static str)> {
    match scenario {
        WorkbenchScenario::AuthRequired => Some((
            StatusCode::UNAUTHORIZED,
            "fixture_credential_required",
            "Create a fixture gateway credential before sending a request in this scenario.",
        )),
        WorkbenchScenario::Empty => Some((
            StatusCode::SERVICE_UNAVAILABLE,
            "fixture_catalog_unavailable",
            "The empty fixture has no catalog model or provider route to serve this request.",
        )),
        WorkbenchScenario::Loading => Some((
            StatusCode::SERVICE_UNAVAILABLE,
            "fixture_route_preparing",
            "The fixture provider is still preparing the model, so no route can serve this request yet.",
        )),
        WorkbenchScenario::Failure => Some((
            StatusCode::SERVICE_UNAVAILABLE,
            "fixture_provider_failure",
            "The fixture provider failed while preparing the model, so no route can serve this request.",
        )),
        WorkbenchScenario::Offline => Some((
            StatusCode::SERVICE_UNAVAILABLE,
            "fixture_no_fresh_route",
            "The fixture catalog has routes, but none has a fresh provider heartbeat.",
        )),
        WorkbenchScenario::Showcase
        | WorkbenchScenario::SourceUpdate
        | WorkbenchScenario::SignedUpdate
        | WorkbenchScenario::UpdateRequired
        | WorkbenchScenario::Scale => None,
    }
}

fn workbench_serving_candidate(
    gateway: &GatewayState,
    model: &GatewayModel,
) -> Option<GatewayRouteCandidate> {
    let entries = gateway
        .provider_table
        .lock_recover("provider table")
        .entries(now_millis_u64());
    model
        .mayhem
        .route_candidates
        .iter()
        .find(|candidate| {
            dashboard_entry_for_route(&entries, candidate)
                .and_then(|entry| entry.heartbeat.as_ref())
                .is_some_and(|heartbeat| {
                    heartbeat.accepting_new
                        && heartbeat.slots.active < heartbeat.slots.max
                        && heartbeat.q.free_slots > 0
                })
        })
        .cloned()
}

fn workbench_chat_error(
    scenario: WorkbenchScenario,
    status: StatusCode,
    code: &'static str,
    message: impl Into<String>,
) -> Response {
    let response = dashboard_json_response(
        status,
        json!({
            "error": {
                "message": message.into(),
                "type": "workbench_scenario_error",
                "code": code,
                "scenario": scenario.id(),
            }
        }),
        None,
    );
    with_workbench_scenario_cookie(response, scenario)
}

fn showcase_state(model_count: usize) -> GatewayState {
    showcase_state_with_receipts(model_count, model_count.min(8))
}

fn scale_state() -> GatewayState {
    let mut models = workbench_models(WORKBENCH_SCALE_MODEL_COUNT);
    let scale_provider = models
        .first()
        .and_then(|model| model.mayhem.route_candidates.first())
        .map(|candidate| candidate.provider.clone())
        .expect("scale workbench fixture has a provider route");
    for model in &mut models {
        for candidate in &mut model.mayhem.route_candidates {
            candidate.provider.clone_from(&scale_provider);
            if let Some(kyb) = candidate.kyb.as_mut() {
                kyb.provider.clone_from(&scale_provider);
            }
        }
    }
    let state = showcase_state_from_models_with_receipts(models, WORKBENCH_SCALE_RECEIPT_COUNT)
        .with_access_control(workbench_scale_access_control());
    let models = state.models_snapshot();
    for index in 1..64 {
        let model = &models[index % models.len()];
        let candidate = model
            .mayhem
            .route_candidates
            .first()
            .expect("scale workbench model has a provider route");
        state.record_probe(workbench_probe(
            model,
            candidate,
            format!("workbench-scale-probe-{:02}", index + 1),
        ));
    }
    state
}

fn showcase_state_with_receipts(model_count: usize, receipt_count: usize) -> GatewayState {
    let models = if model_count == 4 {
        workbench_playground_models(model_count)
    } else {
        workbench_models(model_count)
    };
    showcase_state_from_models_with_receipts(models, receipt_count)
}

fn workbench_playground_models(count: usize) -> Vec<GatewayModel> {
    let mut models = workbench_models(count);
    if let Some(model) = models.get_mut(2) {
        model.id = "tongyi/z-image-turbo".to_owned();
        model.owned_by = "Tongyi-MAI".to_owned();
        model.mayhem.family = "z-image".to_owned();
        model.mayhem.model_class = "image-generation".to_owned();
        model.mayhem.caps.ctx = 1_200;
        model.mayhem.caps.image = true;
        model.mayhem.caps.output_modality = Some("image".to_owned());
        model.mayhem.caps.output_modalities = vec!["image".to_owned()];
        model.mayhem.adapter.modality_set = vec!["text".to_owned(), "image".to_owned()];
        for candidate in &mut model.mayhem.route_candidates {
            candidate.served_modalities = vec!["image".to_owned()];
        }
    }
    if let Some(model) = models.get_mut(3) {
        model.id = "hexgrad/kokoro-82m".to_owned();
        model.owned_by = "Hexgrad".to_owned();
        model.mayhem.family = "kokoro".to_owned();
        model.mayhem.model_class = "tts".to_owned();
        model.mayhem.caps.ctx = 800;
        model.mayhem.caps.audio = true;
        model.mayhem.caps.output_modality = Some("audio".to_owned());
        model.mayhem.caps.output_modalities = vec!["audio".to_owned()];
        model.mayhem.adapter.modality_set = vec!["text".to_owned(), "audio".to_owned()];
        for candidate in &mut model.mayhem.route_candidates {
            candidate.served_modalities = vec!["audio".to_owned()];
        }
    }
    models
}

fn showcase_state_from_models_with_receipts(
    models: Vec<GatewayModel>,
    receipt_count: usize,
) -> GatewayState {
    let state = base_state(models.clone(), true)
        .with_receipt_balance_au(184_720_000_000_000_000_000)
        .with_receipt_rail("fiat")
        .with_payment_directory(json!({
            "ok": true,
            "observed_at": now_secs(),
            "payments": {
                "rails": ["fiat", "tap", "tnk"],
                "fiat": {
                    "processor": "stripe",
                    "integration_currency": "usd",
                    "adaptive_pricing": true,
                    "payout_currencies": ["eur", "gbp", "usd"]
                },
                "tap": { "network": "ethereum" },
                "tnk": { "network": "trac" }
            },
            "rates": {
                "fiat": { "usd": "1.00", "source": "workbench" },
                "tap": { "usd": "0.42", "source": "workbench", "fresh": true },
                "tnk": { "usd": "0.18", "source": "workbench", "fresh": true }
            }
        }))
        .with_access_control(workbench_access_control());

    let mut providers = BTreeSet::new();
    for model in &models {
        for candidate in &model.mayhem.route_candidates {
            providers.insert(candidate.provider.clone());
        }
    }
    let earnings = providers
        .iter()
        .enumerate()
        .map(|(index, provider)| {
            let total = 18_420_000_000_000_000_000_u128
                .saturating_add((index as u128) * 3_100_000_000_000_000_000);
            let held = total / 5;
            let paid = total / 4;
            json!({
                "provider": provider,
                "rail": "fiat",
                "denom": "au_usd",
                "total_au": total.to_string(),
                "held_au": held.to_string(),
                "paid_cum_au": paid.to_string(),
                "claimable_au": total.saturating_sub(held).saturating_sub(paid).to_string(),
                "holdbacks": [{"epoch": 41 + index, "au": held.to_string()}],
                "updated_epoch": 52,
            })
        })
        .collect::<Vec<_>>();
    let state = state.with_provider_earnings(earnings);

    for (index, model) in models.iter().take(receipt_count).enumerate() {
        if let Some(candidate) = model.mayhem.route_candidates.first() {
            let final_receipt = index % 3 != 0;
            let receipt = fixture_receipt(model, candidate, index, final_receipt);
            state
                .record_workbench_receipt(receipt)
                .expect("workbench receipts are internally consistent");
        }
    }
    if let Some((model, candidate)) = models.first().and_then(|model| {
        model
            .mayhem
            .route_candidates
            .first()
            .map(|candidate| (model, candidate))
    }) {
        state.record_probe(workbench_probe(
            model,
            candidate,
            "workbench-probe-latest".to_owned(),
        ));
    }
    state
}

fn workbench_probe(
    model: &GatewayModel,
    candidate: &GatewayRouteCandidate,
    probe_id: String,
) -> StoredProbeEvent {
    StoredProbeEvent {
        probe_id,
        model_id: model.id.clone(),
        provider: candidate.provider.clone(),
        enclave_id: candidate.enclave_id.clone(),
        binary_hash: candidate.binary_hash.clone(),
        canary_set: "workbench-canary-v1".to_owned(),
        verification_method: "token_fingerprint".to_owned(),
        expected_fingerprint: "aa".repeat(32),
        observed_fingerprint: "aa".repeat(32),
        match_bps: 9_980,
        pass: true,
        reputation_event_kind: ReputationEventKind::ProbeOk,
        session_receipt_hash: "bb".repeat(32),
        evidence_hash: "cc".repeat(32),
        evidence: json!({"at": now_secs(), "source": "dashboard_workbench"}),
        probe_command: json!({"fixture": true}),
    }
}

fn auth_required_state() -> GatewayState {
    showcase_state_with_receipts(4, 0).with_access_control(GatewayAccessControl::new(
        true,
        GatewayTokenStore {
            version: 1,
            tokens: Vec::new(),
        },
        None,
    ))
}

fn base_state(models: Vec<GatewayModel>, live: bool) -> GatewayState {
    let local_provider = models
        .iter()
        .flat_map(|model| model.mayhem.route_candidates.iter())
        .next()
        .map(|candidate| candidate.provider.clone());
    let heartbeats = if live {
        models
            .iter()
            .flat_map(|model| {
                model
                    .mayhem
                    .route_candidates
                    .iter()
                    .enumerate()
                    .map(|(index, candidate)| fixture_heartbeat(model, candidate, index))
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut state = GatewayState::from_models(models)
        .with_provider_heartbeat_ttl_millis(WORKBENCH_HEARTBEAT_TTL_MILLIS);
    if let Some(provider) = local_provider {
        state = state.with_local_provider_id(provider);
    }
    if live {
        state.with_provider_heartbeats(heartbeats)
    } else {
        state
    }
}

fn progress_state(name: &str, phase: &str, status: &str, percent: u64) -> io::Result<GatewayState> {
    let mut models = workbench_models(1);
    let model = models
        .first_mut()
        .expect("embedded workbench catalog has at least one model");
    let candidate = model
        .mayhem
        .route_candidates
        .first()
        .cloned()
        .expect("workbench model has a route candidate");
    let local_provider = candidate.provider.clone();
    model.mayhem.route_candidates.clear();
    model.mayhem.providers_online = 0;
    model.mayhem.rooms = 0;

    let dir = workbench_fixture_dir(name);
    fs::create_dir_all(&dir)?;
    fs::write(
        dir.join("provider-progress.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": 1,
            "provider": candidate.provider,
            "model_id": model.id,
            "enclave_id": candidate.enclave_id,
            "artifact": "gguf-q4_k_m",
            "label": "verify catalog artifact",
            "phase": phase,
            "status": status,
            "error": if status == "error" { Some("artifact signature mismatch") } else { None },
            "position": percent,
            "total": 100,
            "percent": percent,
            "updated_at_ms": now_millis_u64(),
        }))?,
    )?;
    Ok(GatewayState::from_models(models)
        .with_local_provider_id(local_provider)
        .with_provider_load_progress_dir(dir))
}

fn workbench_fixture_dir(name: &str) -> PathBuf {
    FsPath::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/dashboard-workbench/fixtures")
        .join(std::process::id().to_string())
        .join(name)
}

fn workbench_models(count: usize) -> Vec<GatewayModel> {
    let embedded = GatewayState::from_embedded_catalog().models_snapshot();
    let seeds = if embedded.is_empty() {
        GatewayState::fixture().models_snapshot()
    } else {
        embedded
    };
    (0..count)
        .map(|index| {
            let mut model = seeds[index % seeds.len()].clone();
            if index >= seeds.len() {
                model.id = format!("workbench/{:02}-{}", index + 1, short_slug(&model.id));
            }
            model.created = now_secs().saturating_sub((index as u64) * 86_400);
            model.owned_by = "mayhem-workbench".to_owned();
            model.mayhem.source = "workbench fixture based on signed catalog shape".to_owned();
            model.mayhem.price_ref_au = fixture_price(&model.mayhem.price_ref_au, index);
            let provider_count = if index % 3 == 0 { 2 } else { 1 };
            model.mayhem.route_candidates = (0..provider_count)
                .map(|route_index| fixture_candidate(&model, index, route_index))
                .collect();
            model.mayhem.markets = model
                .mayhem
                .route_candidates
                .first()
                .map(|candidate| GatewayMarketInfo {
                    enclave_id: candidate.enclave_id.clone(),
                    att_tier: candidate.att_tier,
                    quant: candidate.quant.clone(),
                    ctx_bracket: Some("base".to_owned()),
                    room_ids: model
                        .mayhem
                        .route_candidates
                        .iter()
                        .map(|route| route.room_id.clone())
                        .collect(),
                    providers_online: provider_count as u32,
                    route_count: provider_count as u32,
                    availability: "routable".to_owned(),
                    price_ref_au: model.mayhem.price_ref_au.clone(),
                })
                .into_iter()
                .collect();
            model.mayhem.providers_online = provider_count as u32;
            model.mayhem.rooms = provider_count as u32;
            model.mayhem.attestation_tiers = model.mayhem.route_candidates.iter().fold(
                BTreeMap::new(),
                |mut tiers, candidate| {
                    *tiers.entry(format!("T{}", candidate.att_tier)).or_insert(0) += 1;
                    tiers
                },
            );
            model.mayhem.attestation_tier_labels =
                attestation_tier_labels_for_counts(&model.mayhem.attestation_tiers);
            model.mayhem.quant_buckets = BTreeMap::from([
                ("int4".to_owned(), provider_count as u32),
                ("int8".to_owned(), 1),
            ]);
            model.mayhem.kyb_identities = model
                .mayhem
                .route_candidates
                .iter()
                .filter_map(|candidate| candidate.kyb.clone())
                .collect();
            model
        })
        .collect()
}

fn fixture_candidate(
    model: &GatewayModel,
    model_index: usize,
    route_index: usize,
) -> GatewayRouteCandidate {
    let prior_multi_route_models = model_index.div_ceil(3);
    let seed = u8::try_from(model_index + prior_multi_route_models + route_index)
        .expect("workbench route fixture seed fits in one byte");
    let provider = hex_fill(0x40_u8.wrapping_add(seed));
    let tier = match (model_index + route_index) % 4 {
        1 => 2,
        2 => 3,
        3 => 4,
        _ => 1,
    };
    let served_specialities = model
        .mayhem
        .adapter
        .specialities
        .iter()
        .map(|speciality| {
            (
                speciality.name.clone(),
                speciality
                    .levels
                    .iter()
                    .map(|level| level.name.clone())
                    .collect(),
            )
        })
        .collect();
    let kyb = (tier == 4).then(|| ProviderKybInfo {
        provider: provider.clone(),
        legal_name: "Mayhem Workbench Compute GmbH".to_owned(),
        jurisdiction: "DE".to_owned(),
        proof_hash: hex_fill(0xc1),
        kyb_ref: "workbench/kyb/demo".to_owned(),
    });
    GatewayRouteCandidate {
        provider,
        accepted_rails: vec!["fiat".to_owned(), "tap".to_owned(), "tnk".to_owned()],
        served_modalities: model.mayhem.caps.output_modalities.clone(),
        served_specialities,
        enclave_id: hex_fill(0x70_u8.wrapping_add(seed)),
        room_id: format!("{:02x}", 0xa0_u8.wrapping_add(seed)).repeat(16),
        price_ver: model.mayhem.price_ref_au.ver,
        price_ref_au: Some(model.mayhem.price_ref_au.clone()),
        min_ask_au: 0,
        att_tier: tier,
        quant: if route_index % 2 == 0 { "int4" } else { "int8" }.to_owned(),
        served_ctx: Some(model.mayhem.caps.ctx),
        hardware_fingerprint: Some(hex_fill(0x90_u8.wrapping_add(seed))),
        device_key: Some(format!("workbench-device-{model_index}-{route_index}")),
        admin_pubkey: hex_fill(0x21),
        artifact_root: hex_fill(0x30_u8.wrapping_add(seed)),
        artifact_sidecar_roots: BTreeMap::new(),
        manifest_hash: hex_fill(0x31_u8.wrapping_add(seed)),
        binary_hash: hex_fill(0x32_u8.wrapping_add(seed)),
        approved_binary_hashes: BTreeSet::new(),
        launch_measurements: json!({"workbench": true}),
        kyb,
        reputation_bps: 9_650_u32.saturating_sub((seed as u32 % 4) * 175),
        probation: (model_index == 0 && route_index == 0).then(|| ProviderProbation {
            active: true,
            since_seconds: 1_725_000_000,
            successful_sessions: 7,
            required_successful_sessions: 25,
            required_seconds: 7 * 24 * 60 * 60,
            caps: crate::ProbationCaps::default(),
        }),
        caps: json!({
            "engine": if model_index % 2 == 0 { "llama.cpp" } else { "vllm" },
            "tools": model.mayhem.caps.tools,
            "json": model.mayhem.caps.json,
            "vision": model.mayhem.caps.vision,
            "ctx": model.mayhem.caps.ctx,
        }),
        local_run: Some(GatewayLocalRunBadge {
            marker: "●".to_owned(),
            status: "runs_full".to_owned(),
            label: "runs full".to_owned(),
            reason: "workbench capacity fixture".to_owned(),
            requested_ctx: u64::from(model.mayhem.caps.ctx),
            served_ctx: u64::from(model.mayhem.caps.ctx),
            estimated_tok_s: Some(format!("{:.1}", 34.0 + model_index as f64 * 2.7)),
            memory_required_human: "8.40 GiB".to_owned(),
            memory_budget_human: "12.00 GiB".to_owned(),
            download_human: "5.80 GiB".to_owned(),
            eta: "cached".to_owned(),
        }),
    }
}

fn fixture_price(base: &PriceRefAu, model_index: usize) -> PriceRefAu {
    let mut price = base.clone();
    if model_index == 1 {
        price.rate_map.clear();
        price.per_req_au = 150_000_000_000_000_000;
        price.min_session_au = 250_000_000_000_000_000;
    } else if price.rate_map.is_empty() {
        price.rate_map = text_generation_rate_map(15_000_000_000_000, 45_000_000_000_000);
    }
    price.ver = 52;
    price.history = (40_u64..=52)
        .map(|epoch| {
            let wave = 92_u128 + u128::from((epoch + model_index as u64 * 3) % 17);
            let historical_rates = price
                .rate_map
                .iter()
                .map(|rate| {
                    json!({
                        "unit": rate.unit,
                        "per_unit_au": rate.per_unit_au.saturating_mul(wave).div_ceil(100).to_string(),
                        "granularity": rate.granularity,
                    })
                })
                .collect::<Vec<_>>();
            json!({
                "epoch": epoch,
                "price_source": "market_float",
                "ctx_bracket": if epoch % 4 == 0 { "32k" } else { "base" },
                "usage": {
                    "active_demand_au": (650_000_000_000_000_000_u128 + u128::from(epoch - 40) * 95_000_000_000_000_000).to_string(),
                    "settled_work_au": (400_000_000_000_000_000_u128 + u128::from(epoch - 40) * 70_000_000_000_000_000).to_string(),
                    "session_count": 7 + epoch - 40,
                },
                "controller": {
                    "source": "market_float",
                    "active_supply": 3 + model_index,
                    "utilization_bps": 6_900 + (epoch - 40) * 155,
                    "frozen": false,
                },
                "seed_price": {"ver": 1, "rate_map": historical_rates},
                "result_price": {"ver": epoch, "rate_map": historical_rates},
                "price_root": hex_fill((epoch as u8).wrapping_add(1)),
                "derivation_hash": hex_fill((epoch as u8).wrapping_add(2)),
            })
        })
        .collect();
    price.derivation = price.history.last().cloned();
    price
}

fn fixture_heartbeat(
    model: &GatewayModel,
    candidate: &GatewayRouteCandidate,
    route_index: usize,
) -> ProviderHeartbeat {
    let active = (route_index % 3) as u32;
    let max = 6;
    ProviderHeartbeat {
        t: "hb".to_owned(),
        v: crate::HEARTBEAT_SCHEMA_VERSION,
        contract_version: CONTRACT_VERSION,
        provider: candidate.provider.clone(),
        enclave_id: candidate.enclave_id.clone(),
        model_id: model.id.clone(),
        room_id: candidate.room_id.clone(),
        sat: f64::from(active) / f64::from(max),
        slots: HeartbeatSlots {
            active,
            active_requests: active,
            max,
        },
        q: HeartbeatQueue {
            free_slots: max - active,
            engine_backlog: route_index as u32,
            est_wait_ms: 120 + route_index as u64 * 80,
        },
        perf: HeartbeatPerf {
            tok_s: Some(48.5 + route_index as f64 * 7.25),
            ttft_ms: 180 + route_index as u64 * 45,
        },
        price_ver: candidate.price_ver,
        min_ask_au: candidate.min_ask_au,
        transport_peer: Some(candidate.provider.clone()),
        identity_anchor: Some(format!("provider:{}", candidate.provider)),
        accepting_new: route_index % 5 != 4,
        caps: HeartbeatCaps {
            tools: model.mayhem.caps.tools,
            json: model.mayhem.caps.json,
            ctx: model.mayhem.caps.ctx,
            vision: model.mayhem.caps.vision,
            served_modalities: candidate.served_modalities.clone(),
            modality_capacity: candidate
                .served_modalities
                .iter()
                .filter(|modality| modality.as_str() != "text")
                .map(|modality| {
                    (
                        modality.clone(),
                        HeartbeatModalityCapacity {
                            unit: "unit".to_owned(),
                            max_inflight_items: 4,
                            active_items: 1,
                            max_items_per_request: 2,
                            max_item_bytes: 64 * 1024 * 1024,
                            max_item_units: 1_000_000,
                            working_set_bytes_per_item: 512 * 1024 * 1024,
                        },
                    )
                })
                .collect(),
            served_specialities: candidate.served_specialities.clone(),
        },
        att: HeartbeatAttestation {
            epoch: 52,
            head: candidate.binary_hash.clone(),
        },
        ts: now_millis_u64(),
        nonce: format!("workbench-{}", candidate.room_id),
        sig: "11".repeat(64),
    }
}

fn fixture_receipt(
    model: &GatewayModel,
    candidate: &GatewayRouteCandidate,
    index: usize,
    final_receipt: bool,
) -> StoredReceipt {
    let session_id = format!("workbench-session-{:02}", index + 1);
    let usage = ReceiptUsage::text(640 + index as u64 * 175, 210 + index as u64 * 95);
    let cost = 120_000_000_000_000_000_u128.saturating_add(index as u128 * 47_000_000_000_000_000);
    let voucher_body = SpendVoucherBody {
        schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
        session_id: session_id.clone(),
        billing_id: session_id.clone(),
        billing_attempt: 0,
        billing_prior_usage: ReceiptUsage::default(),
        billing_prior_au_owed_cum: 0,
        billing_epoch: 7,
        reservation_id: hex_fill(0x71),
        user: hex_fill(0x12),
        provider: candidate.provider.clone(),
        payout_revision: "payout-workbench".to_owned(),
        model_id: model.id.clone(),
        rules_ver: 1,
        rail: "fiat".to_owned(),
        enclave_id: candidate.enclave_id.clone(),
        price_ver: candidate.price_ver,
        locked_rate_map: model.mayhem.price_ref_au.rate_map.clone(),
        locked_per_req_au: model.mayhem.price_ref_au.per_req_au,
        locked_min_session_au: model.mayhem.price_ref_au.min_session_au,
        served_ctx: model.mayhem.caps.ctx,
        required_modalities: Vec::new(),
        required_specialities: BTreeMap::new(),
        ctx_bracket: Some("base".to_owned()),
        ctx_bracket_table_ver: Some(1),
        max_spend_au: 5 * AU_PER_USD,
        checkpoint_every: CheckpointPolicy {
            tokens: 256,
            ms: 750,
        },
    };
    let receipt_body = ReceiptBody {
        schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
        session_id: session_id.clone(),
        billing_id: session_id.clone(),
        billing_attempt: 0,
        billing_prior_usage: ReceiptUsage::default(),
        billing_prior_au_owed_cum: 0,
        billing_epoch: voucher_body.billing_epoch,
        reservation_id: voucher_body.reservation_id.clone(),
        payout_revision: voucher_body.payout_revision.clone(),
        seq: if final_receipt { 2 } else { 1 },
        final_receipt,
        rail: "fiat".to_owned(),
        user: hex_fill(0x12),
        provider: candidate.provider.clone(),
        enclave_id: candidate.enclave_id.clone(),
        model_id: model.id.clone(),
        price_ver: candidate.price_ver,
        locked_rate_map: model.mayhem.price_ref_au.rate_map.clone(),
        locked_per_req_au: model.mayhem.price_ref_au.per_req_au,
        locked_min_session_au: model.mayhem.price_ref_au.min_session_au,
        served_ctx: model.mayhem.caps.ctx,
        ctx_bracket: Some("base".to_owned()),
        ctx_bracket_table_ver: Some(1),
        rules_ver: 1,
        usage,
        usage_attribution: BTreeMap::from([(
            "reasoning_tokens".to_owned(),
            72 + index as u64 * 11,
        )]),
        au_owed_cum: cost,
        prompt_hash: hex_fill(0xd0_u8.wrapping_add(index as u8)),
        ts: now_secs().saturating_sub(index as u64 * 460 + if final_receipt { 90 } else { 12 }),
    };
    StoredReceipt {
        rail: "fiat".to_owned(),
        voucher: SpendVoucher {
            body: voucher_body,
            user_sig: "22".repeat(64),
        },
        receipt: SessionReceipt {
            body: receipt_body,
            enclave_sig: "33".repeat(64),
            enclave_pubkey: hex_fill(0xe1),
            user_sig: "44".repeat(64),
        },
        receipt_ack: ReceiptAck {
            session_id,
            seq: if final_receipt { 2 } else { 1 },
            user_sig: "55".repeat(64),
        },
        access_token: Some(GatewayTokenAttribution {
            name: "Workbench agent".to_owned(),
            token_id: "tok_workbench_agent".to_owned(),
        }),
    }
}

fn live_fixture_receipt(
    model: &GatewayModel,
    candidate: &GatewayRouteCandidate,
    sequence: u64,
    session_id: &str,
    prompt: &str,
) -> StoredReceipt {
    let fixture_index = usize::try_from(sequence % 10_000).unwrap_or_default();
    let mut receipt = fixture_receipt(model, candidate, fixture_index, true);
    receipt.voucher.body.session_id = session_id.to_owned();
    receipt.receipt.body.session_id = session_id.to_owned();
    receipt.receipt.body.seq = 1;
    receipt.receipt.body.final_receipt = true;
    receipt.receipt.body.prompt_hash = blake3_hex(prompt.as_bytes());
    receipt.receipt.body.ts = now_secs();
    receipt.receipt_ack.session_id = session_id.to_owned();
    receipt.receipt_ack.seq = 1;
    receipt
}

fn workbench_access_control() -> GatewayAccessControl {
    let now = now_secs();
    GatewayAccessControl::new(
        false,
        GatewayTokenStore {
            version: 1,
            tokens: vec![
                GatewayTokenRecord {
                    name: "Workbench agent".to_owned(),
                    token_hash: gateway_token_hash("workbench-agent"),
                    token_id: "tok_workbench_agent".to_owned(),
                    created_at: now.saturating_sub(14 * 86_400),
                    expires_at: None,
                    budget_au: Some(50 * AU_PER_USD),
                    budget_period: Some(GatewayTokenBudgetPeriod::Month),
                    spent_total_au: 3_240_000_000_000_000_000,
                    spent_period_au: 3_240_000_000_000_000_000,
                    period_started_at: Some(now.saturating_sub(14 * 86_400)),
                    max_rate_per_minute: Some(120),
                    models: Vec::new(),
                    last_used_at: Some(now.saturating_sub(47)),
                    revoked_at: None,
                },
                GatewayTokenRecord {
                    name: "Old integration".to_owned(),
                    token_hash: gateway_token_hash("workbench-old"),
                    token_id: "tok_workbench_old".to_owned(),
                    created_at: now.saturating_sub(90 * 86_400),
                    expires_at: Some(now.saturating_sub(30 * 86_400)),
                    budget_au: None,
                    budget_period: None,
                    spent_total_au: 870_000_000_000_000_000,
                    spent_period_au: 0,
                    period_started_at: None,
                    max_rate_per_minute: None,
                    models: Vec::new(),
                    last_used_at: Some(now.saturating_sub(35 * 86_400)),
                    revoked_at: Some(now.saturating_sub(30 * 86_400)),
                },
            ],
        },
        None,
    )
}

fn workbench_scale_access_control() -> GatewayAccessControl {
    let now = now_secs();
    let tokens = (0_u64..64)
        .map(|index| {
            let active = index >= 56;
            GatewayTokenRecord {
                name: if active {
                    format!("Scale active {:02}", index + 1)
                } else {
                    format!("Scale inactive {:02}", index + 1)
                },
                token_hash: gateway_token_hash(&format!("workbench-scale-{index}")),
                token_id: format!("tok_workbench_scale_{index:02}"),
                created_at: now.saturating_sub((index + 1) * 86_400),
                expires_at: None,
                budget_au: Some(100 * AU_PER_USD),
                budget_period: Some(match index % 3 {
                    0 => GatewayTokenBudgetPeriod::Day,
                    1 => GatewayTokenBudgetPeriod::Month,
                    _ => GatewayTokenBudgetPeriod::Total,
                }),
                spent_total_au: MoneyAu::from(index + 1) * AU_PER_USD,
                spent_period_au: MoneyAu::from(index + 1) * AU_PER_CENT,
                period_started_at: Some(now.saturating_sub(3_600)),
                max_rate_per_minute: Some(120),
                models: Vec::new(),
                last_used_at: Some(now.saturating_sub(index * 60)),
                revoked_at: (!active).then_some(now.saturating_sub(300)),
            }
        })
        .collect();
    GatewayAccessControl::new(false, GatewayTokenStore { version: 1, tokens }, None)
}

fn hex_fill(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
}

fn short_slug(value: &str) -> String {
    let slug = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    slug.trim_matches('-').chars().take(34).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    async fn rendered_workbench_page(app: &Router, uri: &str) -> String {
        rendered_workbench_page_with_cookie(app, uri, None).await
    }

    async fn rendered_workbench_page_with_cookie(
        app: &Router,
        uri: &str,
        cookie: Option<&str>,
    ) -> String {
        let mut request = axum::http::Request::builder()
            .uri(uri)
            .header(header::HOST, "127.0.0.1:11436");
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "unexpected status for {uri}"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    async fn workbench_chat_response(
        app: &Router,
        scenario: WorkbenchScenario,
        model: &str,
        prompt: &str,
    ) -> (StatusCode, HeaderMap, String) {
        workbench_chat_response_with_max_tokens(app, scenario, model, prompt, None).await
    }

    async fn workbench_chat_response_with_max_tokens(
        app: &Router,
        scenario: WorkbenchScenario,
        model: &str,
        prompt: &str,
        max_tokens: Option<u32>,
    ) -> (StatusCode, HeaderMap, String) {
        let mut payload = json!({
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        if let Some(max_tokens) = max_tokens {
            payload["max_tokens"] = json!(max_tokens);
        }
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::HOST, "127.0.0.1:11436")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(
                        header::COOKIE,
                        format!("{WORKBENCH_SCENARIO_COOKIE}={}", scenario.id()),
                    )
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, headers, String::from_utf8(body.to_vec()).unwrap())
    }

    fn first_sse_payload(body: &str) -> Value {
        sse_payloads(body)
            .into_iter()
            .next()
            .expect("chat response should contain a JSON SSE event")
    }

    fn sse_payloads(body: &str) -> Vec<Value> {
        body.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter(|data| *data != "[DONE]")
            .map(|data| serde_json::from_str(data).expect("valid JSON SSE event"))
            .collect()
    }

    fn table_body_row_count(html: &str, caption: &str) -> usize {
        let caption = format!(r#"<caption class="sr-only">{caption}</caption>"#);
        let after_caption = html
            .split_once(&caption)
            .unwrap_or_else(|| panic!("missing table caption {caption}"))
            .1;
        let body = after_caption
            .split_once("<tbody>")
            .unwrap_or_else(|| panic!("missing table body after {caption}"))
            .1
            .split_once("</tbody>")
            .unwrap_or_else(|| panic!("unterminated table body after {caption}"))
            .0;
        body.matches("<tr").count()
    }

    #[tokio::test]
    async fn workbench_serves_index_and_real_dashboard_surfaces() {
        let app = dashboard_workbench_router().expect("workbench router");
        let index = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(index.status(), StatusCode::OK);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/mayhem/dashboard?scenario=showcase")
                    .header(header::HOST, "127.0.0.1:11436")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("<h1>Overview</h1>"));
        assert!(html.contains(r#"class="app-shell""#));
        assert!(html.contains("Recent activity"));
        assert!(html.contains(r#"<body class="has-workbench">"#));
        assert!(html.contains("data-workbench-chrome"));
        assert!(html.contains("Fixture: Showcase"));
        assert!(html.contains("/__workbench/reload.js"));
    }

    #[tokio::test]
    async fn auth_required_first_use_flow_stops_before_inference() {
        let app = dashboard_workbench_router().expect("workbench router");
        let home = rendered_workbench_page(&app, "/mayhem/dashboard?scenario=auth-required").await;
        assert!(home.contains("<h1>Overview</h1>"));
        assert!(home.contains("Credential needed"));
        assert!(home.contains(
            r#"href="/mayhem/dashboard/connect" data-product-event="use_ai_path_opened">Set up access"#
        ));
        assert!(home.contains("<h2>Your provider</h2>"));
        assert!(!home.contains("<h2>Getting started</h2>"));

        let playground =
            rendered_workbench_page(&app, "/mayhem/dashboard/playground?scenario=auth-required")
                .await;
        assert!(playground.contains("Create an access token first"));
        assert!(!playground.contains("data-playground-form"));
        assert_eq!(playground.matches(">Set up access</a>").count(), 1);

        let connect =
            rendered_workbench_page(&app, "/mayhem/dashboard/connect?scenario=auth-required").await;
        assert!(
            connect.contains(r##"class="primary-button" href="#access-tokens">Set up credential"##)
        );
        assert!(connect.contains("Available after an API key is configured."));
        assert!(!connect.contains(r#"class="primary-button" href="/mayhem/dashboard/playground""#));
    }

    #[tokio::test]
    async fn workbench_loading_scenario_exposes_progress_without_a_route() {
        let app = dashboard_workbench_router().expect("workbench router");
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/mayhem/dashboard/earn/machines?scenario=loading")
                    .header(header::HOST, "127.0.0.1:11436")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Preparing a model"));
        assert!(html.contains("download 68%"));
        assert!(html.contains(r#"<progress max="100" value="68""#));
        assert!(html.contains("No machine routes yet"));
    }

    #[tokio::test]
    async fn workbench_exercises_probation_and_read_only_provider_recovery() {
        let app = dashboard_workbench_router().expect("workbench router");

        let reliability =
            rendered_workbench_page(&app, "/mayhem/dashboard/earn/reliability?scenario=showcase")
                .await;
        assert!(reliability.contains("Probation active"));
        assert!(reliability.contains("7 / 25 successful sessions"));
        assert!(reliability.contains(
            r#"<progress max="25" value="7" aria-label="Probation successful-session requirement: 7 of 25">"#
        ));
        assert!(reliability.contains("Provider identity:"));
        assert!(!reliability.contains("Configured gateway identity"));

        let failure =
            rendered_workbench_page(&app, "/mayhem/dashboard/earn/machines?scenario=failure").await;
        assert!(failure.contains("Recover model preparation"));
        assert!(failure.contains("rerun the same mayhem provider start command"));
        assert!(failure.contains("Refresh snapshot"));
        assert!(failure.contains("does not change provider state"));

        let offline =
            rendered_workbench_page(&app, "/mayhem/dashboard/earn/machines?scenario=offline").await;
        assert!(offline.contains("Restore the provider route"));
        assert!(offline.contains("This page cannot publish a route or start a worker."));
        assert!(offline.contains("Refresh snapshot"));
    }

    #[tokio::test]
    async fn workbench_evidence_is_on_demand_and_scale_pages_stay_bounded() {
        let app = dashboard_workbench_router().expect("workbench router");
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/mayhem/dashboard/models?scenario=scale")
                    .header(header::HOST, "127.0.0.1:11436")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(
            body.len() < 300_000,
            "scale Models page unexpectedly large: {} bytes",
            body.len()
        );
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(html.matches(r#"id="dashboard-evidence-dialog""#).count(), 1);
        assert_eq!(html.matches(r#"id="model-detail-dialog""#).count(), 1);
        assert_eq!(html.matches("<dialog").count(), 2);
        let marker = "/mayhem/dashboard/evidence?kind=model&amp;id=";
        let start = html.find(marker).expect("model evidence link");
        let end = html[start..]
            .find('"')
            .map(|offset| start + offset)
            .expect("evidence href terminator");
        let href = html[start..end].replace("&amp;", "&");

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri(href)
                    .header(header::HOST, "127.0.0.1:11436")
                    .header(header::ACCEPT, "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: Value = serde_json::from_slice(&body).expect("evidence json");
        assert_eq!(
            payload.get("title").and_then(Value::as_str),
            Some("Model evidence")
        );
        assert!(payload.get("raw").and_then(|raw| raw.get("id")).is_some());
    }

    #[tokio::test]
    async fn workbench_scale_paginates_every_bounded_analytical_dataset() {
        const PROVIDER_PAGE_SIZE: usize = 25;
        let scale = scale_state();
        assert_eq!(scale.models_snapshot().len(), WORKBENCH_SCALE_MODEL_COUNT);
        assert_eq!(scale.receipts().len(), WORKBENCH_SCALE_RECEIPT_COUNT);
        let catalog_route_count = scale
            .models_snapshot()
            .iter()
            .map(|model| model.mayhem.route_candidates.len())
            .sum::<usize>();
        assert_eq!(catalog_route_count, 128);
        assert_eq!(scale.probes().len(), 64);
        let catalog_market_count = scale
            .models_snapshot()
            .iter()
            .map(|model| model.mayhem.markets.len())
            .sum::<usize>();
        assert!(catalog_market_count > PROVIDER_PAGE_SIZE);

        let app = dashboard_workbench_router().expect("workbench router");
        let scale_cookie = format!(
            "{WORKBENCH_SCENARIO_COOKIE}={}",
            WorkbenchScenario::Scale.id()
        );

        let models = rendered_workbench_page(&app, "/mayhem/dashboard/models?scenario=scale").await;
        assert_eq!(
            table_body_row_count(&models, "Models in this gateway catalog"),
            25
        );
        assert!(models.contains(r#"id="models-count">25 shown rows"#));
        assert!(models.contains("Showing rows 1&ndash;25 of 96 catalog models. Page 1 of 4."));
        assert!(models.contains(r#"rel="next" href="/mayhem/dashboard/models?page=2""#));
        let models_second = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/models?page=2",
            Some(scale_cookie.as_str()),
        )
        .await;
        assert_eq!(
            table_body_row_count(&models_second, "Models in this gateway catalog"),
            25
        );
        assert!(
            models_second.contains("Showing rows 26&ndash;50 of 96 catalog models. Page 2 of 4.")
        );
        assert!(models_second.contains(r#"rel="prev" href="/mayhem/dashboard/models?page=1""#));
        let models_invalid = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/models?page=not-a-page",
            Some(scale_cookie.as_str()),
        )
        .await;
        assert!(
            models_invalid.contains("Showing rows 1&ndash;25 of 96 catalog models. Page 1 of 4.")
        );
        let models_clamped = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/models?page=999",
            Some(scale_cookie.as_str()),
        )
        .await;
        assert!(
            models_clamped.contains("Showing rows 76&ndash;96 of 96 catalog models. Page 4 of 4.")
        );
        assert!(models_clamped.contains("workbench/96-"));

        let activity =
            rendered_workbench_page(&app, "/mayhem/dashboard/activity?scenario=scale").await;
        assert_eq!(
            table_body_row_count(
                &activity,
                "Prioritized incomplete records, final receipts, and retained pause records from this gateway process",
            ),
            25
        );
        assert!(activity.contains(r#"id="activity-count">25 shown rows"#));
        assert!(activity.contains("Showing rows 1&ndash;25 of 96 recorded sessions. Page 1 of 4."));
        assert!(activity.contains(r#"<span class="metric-label">Final receipts</span>"#));
        assert!(!activity.contains(r#"<span class="metric-label">Checkpoints</span>"#));
        let activity_second = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/activity?page=4",
            Some(scale_cookie.as_str()),
        )
        .await;
        assert_eq!(
            table_body_row_count(
                &activity_second,
                "Prioritized incomplete records, final receipts, and retained pause records from this gateway process",
            ),
            21
        );
        assert!(activity_second
            .contains("Showing rows 76&ndash;96 of 96 recorded sessions. Page 4 of 4."));
        assert!(activity_second.contains("workbench-session-96"));

        let connect =
            rendered_workbench_page(&app, "/mayhem/dashboard/connect?scenario=scale").await;
        assert_eq!(
            table_body_row_count(
                &connect,
                "Gateway access tokens, budgets, scopes, and status",
            ),
            25
        );
        assert!(connect.contains("Showing rows 1&ndash;25 of 64 access tokens. Page 1 of 3."));
        assert!(connect.contains(r##"data-table-filter="#access-tokens-table""##));
        assert_eq!(connect.matches("data-filter-row").count(), 25);
        assert!(connect.contains(r#"rel="next" href="/mayhem/dashboard/connect?page=2""#));
        assert!(connect.contains("Scale active 64"));
        assert!(connect.contains("8 active &middot; 64 total"));
        let connect_second = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/connect?page=3",
            Some(scale_cookie.as_str()),
        )
        .await;
        assert_eq!(
            table_body_row_count(
                &connect_second,
                "Gateway access tokens, budgets, scopes, and status",
            ),
            14
        );
        assert!(
            connect_second.contains("Showing rows 51&ndash;64 of 64 access tokens. Page 3 of 3.")
        );
        assert!(connect_second.contains("Scale inactive 56"));

        let earn_overview = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/earn?provider=provider%20scope",
            Some(scale_cookie.as_str()),
        )
        .await;
        assert_eq!(
            table_body_row_count(
                &earn_overview,
                "Configured provider serving routes and current capacity",
            ),
            PROVIDER_PAGE_SIZE
        );
        assert!(earn_overview
            .contains("Showing rows 1&ndash;25 of 128 configured serving routes. Page 1 of 6."));
        assert!(earn_overview.contains(r##"data-table-filter="#earn-routes-table""##));
        assert!(earn_overview
            .contains(r#"href="/mayhem/dashboard/earn?provider=provider%20scope&amp;page=2""#));
        let earn_overview_last = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/earn?page=6",
            Some(scale_cookie.as_str()),
        )
        .await;
        assert_eq!(
            table_body_row_count(
                &earn_overview_last,
                "Configured provider serving routes and current capacity",
            ),
            3
        );
        assert!(earn_overview_last
            .contains("Showing rows 126&ndash;128 of 128 configured serving routes. Page 6 of 6."));

        let machines_last = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/earn/machines?page=6",
            Some(scale_cookie.as_str()),
        )
        .await;
        assert_eq!(
            table_body_row_count(
                &machines_last,
                "Machine routes for the configured provider identity",
            ),
            3
        );
        assert!(machines_last.contains(r##"data-table-filter="#machine-routes-table""##));
        assert!(machines_last
            .contains("Showing rows 126&ndash;128 of 128 configured machine routes. Page 6 of 6."));

        let reliability_last = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/earn/reliability?page=6",
            Some(scale_cookie.as_str()),
        )
        .await;
        assert_eq!(
            table_body_row_count(
                &reliability_last,
                "Provider route reputation, probation, and gateway observations",
            ),
            3
        );
        assert!(reliability_last.contains(r##"data-table-filter="#reliability-routes-table""##));
        assert!(reliability_last.contains(
            "Showing rows 126&ndash;128 of 128 provider reliability routes. Page 6 of 6."
        ));

        let opportunities =
            rendered_workbench_page(&app, "/mayhem/dashboard/earn/opportunities?scenario=scale")
                .await;
        assert_eq!(
            table_body_row_count(
                &opportunities,
                "Catalog models, gateway-host compatibility, and advertised supply",
            ),
            25
        );
        assert!(
            opportunities.contains("Showing rows 1&ndash;25 of 96 catalog models. Page 1 of 4.")
        );
        let opportunities_second = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/earn/opportunities?page=4",
            Some(scale_cookie.as_str()),
        )
        .await;
        assert_eq!(
            table_body_row_count(
                &opportunities_second,
                "Catalog models, gateway-host compatibility, and advertised supply",
            ),
            21
        );
        assert!(opportunities_second
            .contains("Showing rows 76&ndash;96 of 96 catalog models. Page 4 of 4."));
        assert!(opportunities_second.contains("workbench/96-"));

        let network_models =
            rendered_workbench_page(&app, "/mayhem/dashboard/network/models?scenario=scale").await;
        assert_eq!(
            table_body_row_count(
                &network_models,
                "Network models, advertised capacity, capabilities, and price",
            ),
            25
        );
        assert!(
            network_models.contains("Showing rows 1&ndash;25 of 96 network models. Page 1 of 4.")
        );
        let network_models_second = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/network/models?page=4",
            Some(scale_cookie.as_str()),
        )
        .await;
        assert_eq!(
            table_body_row_count(
                &network_models_second,
                "Network models, advertised capacity, capabilities, and price",
            ),
            21
        );
        assert!(network_models_second
            .contains("Showing rows 76&ndash;96 of 96 network models. Page 4 of 4."));
        assert!(network_models_second.contains("workbench/96-"));

        let network_providers =
            rendered_workbench_page(&app, "/mayhem/dashboard/network/providers?scenario=scale")
                .await;
        assert_eq!(
            table_body_row_count(
                &network_providers,
                "Canonical provider routes and current operational evidence",
            ),
            25
        );
        assert!(network_providers.contains(r#"id="provider-count">25 shown rows"#));
        assert!(network_providers
            .contains("Showing rows 1&ndash;25 of 128 catalog provider routes. Page 1 of 6."));
        let network_providers_last = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/network/providers?page=6",
            Some(scale_cookie.as_str()),
        )
        .await;
        assert_eq!(
            table_body_row_count(
                &network_providers_last,
                "Canonical provider routes and current operational evidence",
            ),
            3
        );
        assert!(network_providers_last
            .contains("Showing rows 126&ndash;128 of 128 catalog provider routes. Page 6 of 6."));
        assert!(network_providers_last.contains("workbench/96-"));

        let market_last_page = catalog_market_count.div_ceil(PROVIDER_PAGE_SIZE);
        let market_last_start = (market_last_page - 1) * PROVIDER_PAGE_SIZE + 1;
        let network_markets_last = rendered_workbench_page_with_cookie(
            &app,
            &format!("/mayhem/dashboard/network/markets?page={market_last_page}"),
            Some(scale_cookie.as_str()),
        )
        .await;
        assert_eq!(
            table_body_row_count(
                &network_markets_last,
                "Catalog markets and reference prices"
            ),
            catalog_market_count - market_last_start + 1
        );
        assert!(network_markets_last.contains(&format!(
            "Showing rows {market_last_start}&ndash;{catalog_market_count} of {catalog_market_count} catalog markets. Page {market_last_page} of {market_last_page}."
        )));

        let network_activity =
            rendered_workbench_page(&app, "/mayhem/dashboard/network/activity?scenario=scale")
                .await;
        assert_eq!(
            table_body_row_count(
                &network_activity,
                "Provider route observations ordered by heartbeat freshness",
            ),
            25
        );
        assert!(network_activity
            .contains("Showing rows 1&ndash;25 of 128 route observations. Page 1 of 6."));
        let network_activity_last = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/network/activity?page=6",
            Some(scale_cookie.as_str()),
        )
        .await;
        assert_eq!(
            table_body_row_count(
                &network_activity_last,
                "Provider route observations ordered by heartbeat freshness",
            ),
            3
        );
        assert!(network_activity_last
            .contains("Showing rows 126&ndash;128 of 128 route observations. Page 6 of 6."));

        let network_evidence =
            rendered_workbench_page(&app, "/mayhem/dashboard/network/evidence?scenario=scale")
                .await;
        assert_eq!(
            table_body_row_count(&network_evidence, "Provider route evidence"),
            25
        );
        assert!(
            network_evidence.contains("Showing rows 1&ndash;25 of 128 route entries. Page 1 of 6.")
        );
        let network_evidence_last = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/network/evidence?page=6",
            Some(scale_cookie.as_str()),
        )
        .await;
        assert_eq!(
            table_body_row_count(&network_evidence_last, "Provider route evidence"),
            3
        );
        assert!(network_evidence_last
            .contains("Showing rows 126&ndash;128 of 128 route entries. Page 6 of 6."));

        assert_eq!(
            table_body_row_count(&network_evidence, "Verification probe evidence"),
            25
        );
        assert!(network_evidence.contains(r##"data-table-filter="#evidence-probes-table""##));
        assert!(network_evidence.contains(r#"data-table-query-prefix="probe""#));
        assert!(
            network_evidence.contains("Showing rows 1&ndash;25 of 64 probe events. Page 1 of 3.")
        );
        assert!(network_evidence
            .contains(r#"rel="next" href="/mayhem/dashboard/network/evidence?probe_page=2""#));
        let network_probe_last = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/network/evidence?page=2&probe_page=3",
            Some(scale_cookie.as_str()),
        )
        .await;
        assert_eq!(
            table_body_row_count(&network_probe_last, "Verification probe evidence"),
            14
        );
        assert!(network_probe_last
            .contains("Showing rows 51&ndash;64 of 64 probe events. Page 3 of 3."));
        assert!(network_probe_last.contains("workbench-scale-probe-64"));
        assert!(network_probe_last.contains(
            r#"rel="prev" href="/mayhem/dashboard/network/evidence?page=2&amp;probe_page=2""#
        ));
        assert!(network_probe_last
            .contains(r#"href="/mayhem/dashboard/network/evidence?probe_page=3&amp;page=1""#));
    }

    #[tokio::test]
    async fn showcase_chat_records_unique_receipts_visible_on_home_activity_and_verify() {
        let app = dashboard_workbench_router().expect("workbench router");
        let model = workbench_models(1)
            .into_iter()
            .next()
            .expect("workbench model")
            .id;
        let cookie = format!(
            "{WORKBENCH_SCENARIO_COOKIE}={}",
            WorkbenchScenario::Showcase.id()
        );

        let home_before =
            rendered_workbench_page_with_cookie(&app, "/mayhem/dashboard", Some(cookie.as_str()))
                .await;
        assert!(home_before.contains("4 receipt records"));

        let (first_status, first_headers, first_body) = workbench_chat_response(
            &app,
            WorkbenchScenario::Showcase,
            &model,
            "first deterministic request",
        )
        .await;
        assert_eq!(first_status, StatusCode::OK);
        assert!(first_headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream")));
        let first_event = first_sse_payload(&first_body);
        assert_eq!(
            first_event.get("model").and_then(Value::as_str),
            Some(model.as_str())
        );
        let first_session = first_event
            .get("id")
            .and_then(Value::as_str)
            .expect("first fixture session")
            .to_owned();
        let first_payloads = sse_payloads(&first_body);
        assert_eq!(first_payloads.len(), 3);
        let finish = first_payloads
            .iter()
            .find(|payload| {
                payload
                    .pointer("/choices/0/finish_reason")
                    .and_then(Value::as_str)
                    == Some("stop")
            })
            .expect("production-shaped finish chunk");
        assert_eq!(finish.pointer("/choices/0/delta"), Some(&json!({})));
        let first_usage = first_payloads
            .iter()
            .find(|payload| {
                payload
                    .get("choices")
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty)
            })
            .expect("include_usage fixture chunk");
        assert!(first_usage.get("usage").is_some_and(Value::is_object));
        let first_receipt = first_usage
            .get("mayhem")
            .and_then(|mayhem| mayhem.get("receipt"))
            .expect("fixture receipt summary");
        assert_eq!(
            first_receipt.get("session_id").and_then(Value::as_str),
            Some(first_session.as_str())
        );
        assert_eq!(
            first_receipt.get("final").and_then(Value::as_bool),
            Some(true)
        );
        assert!(first_receipt.get("au_owed_cum").is_some());

        let (second_status, _, second_body) = workbench_chat_response(
            &app,
            WorkbenchScenario::Showcase,
            &model,
            "second deterministic request",
        )
        .await;
        assert_eq!(second_status, StatusCode::OK);
        let second_event = first_sse_payload(&second_body);
        assert_eq!(
            second_event.get("model").and_then(Value::as_str),
            Some(model.as_str())
        );
        let second_session = second_event
            .get("id")
            .and_then(Value::as_str)
            .expect("second fixture session")
            .to_owned();
        assert_ne!(first_session, second_session);

        let home_after =
            rendered_workbench_page_with_cookie(&app, "/mayhem/dashboard", Some(cookie.as_str()))
                .await;
        assert!(home_after.contains("6 receipt records"));

        let activity = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/activity",
            Some(cookie.as_str()),
        )
        .await;
        assert!(activity.contains(&first_session));
        assert!(activity.contains(&second_session));
        assert!(activity.contains(r#"<span class="metric-label">Final receipts</span>"#));
        assert!(!activity.contains(r#"<span class="metric-label">Checkpoints</span>"#));

        let evidence_uri = format!("/mayhem/dashboard/evidence?kind=receipt&id={second_session}");
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri(evidence_uri)
                    .header(header::HOST, "127.0.0.1:11436")
                    .header(header::COOKIE, cookie)
                    .header(header::ACCEPT, "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let evidence: Value = serde_json::from_slice(&body).expect("receipt evidence JSON");
        let receipt_body = evidence
            .get("raw")
            .and_then(|raw| raw.get("receipt"))
            .expect("raw receipt body");
        assert_eq!(
            receipt_body.get("session_id").and_then(Value::as_str),
            Some(second_session.as_str())
        );
        assert_eq!(
            receipt_body.get("model_id").and_then(Value::as_str),
            Some(model.as_str())
        );
        assert_eq!(
            receipt_body.get("final").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[tokio::test]
    async fn minimum_output_limit_emits_production_shaped_length_finish() {
        let app = dashboard_workbench_router().expect("workbench router");
        let model = workbench_models(1)
            .into_iter()
            .next()
            .expect("workbench model")
            .id;
        let (status, _, body) = workbench_chat_response_with_max_tokens(
            &app,
            WorkbenchScenario::Scale,
            &model,
            "exercise the deterministic output limit",
            Some(64),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let payloads = sse_payloads(&body);
        assert!(payloads.iter().any(|payload| {
            payload
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
                .is_some_and(|content| content.contains("Workbench output-limit fixture"))
        }));
        assert!(payloads.iter().any(|payload| {
            payload
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
                == Some("length")
                && payload.pointer("/choices/0/delta") == Some(&json!({}))
        }));
        assert!(payloads.iter().any(|payload| {
            payload
                .pointer("/usage/completion_tokens")
                .and_then(Value::as_u64)
                == Some(64)
        }));
        assert!(body.ends_with("data: [DONE]\n\n"));
    }

    #[tokio::test]
    async fn chat_failure_scenarios_return_truthful_errors_without_receipts() {
        let app = dashboard_workbench_router().expect("workbench router");
        let model = workbench_models(1)
            .into_iter()
            .next()
            .expect("workbench model")
            .id;
        let cases = [
            (
                WorkbenchScenario::AuthRequired,
                model.as_str(),
                StatusCode::UNAUTHORIZED,
                "fixture_credential_required",
            ),
            (
                WorkbenchScenario::Empty,
                model.as_str(),
                StatusCode::SERVICE_UNAVAILABLE,
                "fixture_catalog_unavailable",
            ),
            (
                WorkbenchScenario::Loading,
                model.as_str(),
                StatusCode::SERVICE_UNAVAILABLE,
                "fixture_route_preparing",
            ),
            (
                WorkbenchScenario::Failure,
                model.as_str(),
                StatusCode::SERVICE_UNAVAILABLE,
                "fixture_provider_failure",
            ),
            (
                WorkbenchScenario::Offline,
                model.as_str(),
                StatusCode::SERVICE_UNAVAILABLE,
                "fixture_no_fresh_route",
            ),
            (
                WorkbenchScenario::UpdateRequired,
                "workbench/catalog-next",
                StatusCode::UPGRADE_REQUIRED,
                "fixture_update_required",
            ),
        ];

        for (scenario, requested_model, expected_status, expected_code) in cases {
            let (status, headers, body) =
                workbench_chat_response(&app, scenario, requested_model, "must not succeed").await;
            assert_eq!(
                status, expected_status,
                "unexpected status for {scenario:?}"
            );
            assert!(headers
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("application/json")));
            assert!(!body.contains("data:"));
            let payload: Value = serde_json::from_str(&body).expect("scenario error JSON");
            assert_eq!(
                payload
                    .get("error")
                    .and_then(|error| error.get("code"))
                    .and_then(Value::as_str),
                Some(expected_code)
            );
            assert_eq!(
                payload
                    .get("error")
                    .and_then(|error| error.get("scenario"))
                    .and_then(Value::as_str),
                Some(scenario.id())
            );
        }

        let offline_cookie = format!(
            "{WORKBENCH_SCENARIO_COOKIE}={}",
            WorkbenchScenario::Offline.id()
        );
        let offline_activity = rendered_workbench_page_with_cookie(
            &app,
            "/mayhem/dashboard/activity",
            Some(offline_cookie.as_str()),
        )
        .await;
        assert!(offline_activity.contains(r#"<span class="metric-label">Open records</span><span class="metric-state">Records</span></div><div class="metric-value">0</div>"#));
    }

    #[test]
    fn workbench_stream_options_only_enable_requested_usage_metadata() {
        assert_eq!(workbench_include_usage(&json!({})), Ok(false));
        assert_eq!(
            workbench_include_usage(&json!({"stream_options": null})),
            Ok(false)
        );
        assert_eq!(
            workbench_include_usage(&json!({"stream_options": {"include_usage": false}})),
            Ok(false)
        );
        assert_eq!(
            workbench_include_usage(&json!({"stream_options": {"include_usage": true}})),
            Ok(true)
        );
        assert_eq!(
            workbench_include_usage(&json!({"stream_options": {"invented": true}}))
                .expect_err("unknown stream option must not be silently accepted")
                .0,
            "fixture_unsupported_stream_option"
        );
    }
}
