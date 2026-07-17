use super::dashboard_ui::{dashboard_app_shell, DashboardAppPage, DashboardShell};
use super::*;

const MAX_ACTIVITY_ROWS: usize = 25;
const MAX_MODEL_ROWS: usize = 25;
const MAX_PROVIDER_ROWS: usize = 25;
const MAX_EVIDENCE_ROWS: usize = 25;
const MAX_TOKEN_ROWS: usize = 25;
const PAYMENT_SNAPSHOT_FRESH_SECONDS: u64 = 30;
const EARNINGS_SNAPSHOT_FRESH_SECONDS: u64 = 60;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DashboardProductPage {
    Home,
    Playground,
    Models,
    Activity,
    Wallet,
    Connect,
    Earn,
    EarnJobs,
    EarnMachines,
    EarnOpportunities,
    EarnEarnings,
    EarnReliability,
    Network,
    NetworkModels,
    NetworkProviders,
    NetworkMarkets,
    NetworkActivity,
    NetworkEvidence,
    Help,
    Settings,
}

impl DashboardProductPage {
    pub(super) fn from_path(path: &str) -> Option<Self> {
        match path.trim_matches('/') {
            "playground" => Some(Self::Playground),
            "models" => Some(Self::Models),
            "activity" => Some(Self::Activity),
            "wallet" => Some(Self::Wallet),
            "connect" => Some(Self::Connect),
            "earn" => Some(Self::Earn),
            "earn/jobs" => Some(Self::EarnJobs),
            "earn/machines" => Some(Self::EarnMachines),
            "earn/opportunities" => Some(Self::EarnOpportunities),
            "earn/earnings" => Some(Self::EarnEarnings),
            "earn/reliability" => Some(Self::EarnReliability),
            "network" => Some(Self::Network),
            "network/models" => Some(Self::NetworkModels),
            "network/providers" => Some(Self::NetworkProviders),
            "network/markets" => Some(Self::NetworkMarkets),
            "network/activity" => Some(Self::NetworkActivity),
            "network/evidence" => Some(Self::NetworkEvidence),
            "help" => Some(Self::Help),
            "settings" => Some(Self::Settings),
            _ => None,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Playground => "Playground",
            Self::Models => "Models",
            Self::Activity => "Activity",
            Self::Wallet => "Wallet",
            Self::Connect => "Connect",
            Self::Earn => "Earn",
            Self::EarnJobs => "Jobs",
            Self::EarnMachines => "Machines",
            Self::EarnOpportunities => "Model opportunities",
            Self::EarnEarnings => "Earnings and payouts",
            Self::EarnReliability => "Reliability",
            Self::Network => "Network",
            Self::NetworkModels => "Network models",
            Self::NetworkProviders => "Network providers",
            Self::NetworkMarkets => "Network markets",
            Self::NetworkActivity => "Network activity",
            Self::NetworkEvidence => "Network evidence",
            Self::Help => "Help",
            Self::Settings => "Settings",
        }
    }
}

#[derive(Clone)]
struct DashboardData {
    generated_at_millis: u64,
    history_persistent: bool,
    provider_heartbeat_ttl_millis: u64,
    models: Arc<Vec<GatewayModel>>,
    entries: Vec<ProviderTableEntry>,
    receipts: Vec<StoredReceipt>,
    receipt_checkpoint_count: usize,
    paused_sessions: Vec<PausedSession>,
    probes: Vec<StoredProbeEvent>,
    balance_au: MoneyAu,
    payment_directory: Option<Value>,
    access: Value,
    update_notice: Option<GatewayUpdateNotice>,
    earnings: GatewayProviderEarningsSnapshot,
    local_provider_id: Option<String>,
    rail: String,
    provider_load_progress: BTreeMap<(String, String), DashboardProviderLoadProgress>,
}

impl DashboardData {
    fn from_state(state: &GatewayState) -> Self {
        let generated_at_millis = now_millis_u64();
        let entries = state
            .provider_table
            .lock_recover("provider table")
            .entries(generated_at_millis);
        let all_receipts = state.receipts();
        Self {
            generated_at_millis,
            history_persistent: state.dashboard_history_path.as_ref().is_some(),
            provider_heartbeat_ttl_millis: state.provider_heartbeat_ttl_millis,
            models: state.models_snapshot(),
            entries,
            receipts: dashboard_latest_receipts(&all_receipts),
            receipt_checkpoint_count: all_receipts.len(),
            paused_sessions: state.paused_sessions(),
            probes: state.probes(),
            balance_au: state.ledger_balance_au(),
            payment_directory: state.payment_directory(),
            access: state.access_summary(),
            update_notice: state.update_notice(),
            earnings: state.provider_earnings_snapshot(),
            local_provider_id: state.local_provider_id().map(str::to_owned),
            rail: state.receipt_config.rail.clone(),
            provider_load_progress: dashboard_provider_load_progress(state),
        }
    }

    fn accepting_routes(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| route_operational_state(entry).kind == RouteStateKind::Accepting)
            .count()
    }

    fn fresh_routes(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.heartbeat.is_some())
            .count()
    }

    fn accepting_models(&self) -> usize {
        self.models
            .iter()
            .filter(|model| {
                model.mayhem.route_candidates.iter().any(|candidate| {
                    dashboard_entry_for_route(&self.entries, candidate).is_some_and(|entry| {
                        route_operational_state(entry).kind == RouteStateKind::Accepting
                    })
                })
            })
            .count()
    }

    fn completed_requests(&self) -> usize {
        self.receipts
            .iter()
            .filter(|receipt| receipt.receipt.body.final_receipt)
            .count()
    }

    fn incomplete_session_count(&self) -> usize {
        let final_sessions = self
            .receipts
            .iter()
            .filter(|receipt| receipt.receipt.body.final_receipt)
            .map(|receipt| receipt.receipt.body.session_id.as_str())
            .collect::<BTreeSet<_>>();
        let mut incomplete = self
            .receipts
            .iter()
            .filter(|receipt| {
                !receipt.receipt.body.final_receipt
                    && !final_sessions.contains(receipt.receipt.body.session_id.as_str())
            })
            .map(|receipt| receipt.receipt.body.session_id.as_str())
            .collect::<BTreeSet<_>>();
        incomplete.extend(
            self.paused_sessions
                .iter()
                .filter(|paused| !final_sessions.contains(paused.session_id.as_str()))
                .map(|paused| paused.session_id.as_str()),
        );
        incomplete.len()
    }

    fn pause_only_sessions(&self) -> Vec<&PausedSession> {
        let receipt_sessions = self
            .receipts
            .iter()
            .map(|receipt| receipt.receipt.body.session_id.as_str())
            .collect::<BTreeSet<_>>();
        self.paused_sessions
            .iter()
            .filter(|paused| !receipt_sessions.contains(paused.session_id.as_str()))
            .collect()
    }

    fn active_token_count(&self) -> u64 {
        self.access
            .get("active_token_count")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    }

    fn requires_auth(&self) -> bool {
        self.access
            .get("require_auth")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    fn history_scope(&self) -> &'static str {
        if self.history_persistent {
            "Durable gateway history"
        } else {
            "Current gateway run"
        }
    }

    fn payment_observed_at(&self) -> Option<u64> {
        self.payment_directory
            .as_ref()
            .and_then(|value| value.get("observed_at"))
            .and_then(Value::as_u64)
            .map(timestamp_seconds)
    }

    fn payment_freshness(&self) -> String {
        self.payment_observed_at()
            .map(|timestamp| {
                format!(
                    "Ledger snapshot refreshed {} ago",
                    format_elapsed_since(timestamp)
                )
            })
            .unwrap_or_else(|| "Snapshot freshness unavailable".to_owned())
    }

    fn payment_snapshot_is_fresh(&self) -> Option<bool> {
        self.payment_observed_at()
            .map(|timestamp| now_secs().saturating_sub(timestamp) <= PAYMENT_SNAPSHOT_FRESH_SECONDS)
    }

    fn provider_entries<'a>(&'a self, _requested: Option<&str>) -> Vec<&'a ProviderTableEntry> {
        let Some(provider_id) = self.local_provider_id.as_deref() else {
            return Vec::new();
        };
        self.entries
            .iter()
            .filter(|entry| entry.key.provider == provider_id)
            .collect()
    }

    fn has_provider_evidence(&self) -> bool {
        let Some(provider_id) = self.local_provider_id.as_deref() else {
            return false;
        };
        self.entries
            .iter()
            .any(|entry| entry.key.provider == provider_id)
            || self
                .provider_load_progress
                .values()
                .any(|progress| progress.provider == provider_id)
            || self
                .earnings
                .entries
                .iter()
                .any(|entry| entry.get("provider").and_then(Value::as_str) == Some(provider_id))
    }
}

#[derive(Clone, Copy)]
enum ActivityRecord<'a> {
    Receipt(&'a StoredReceipt),
    Paused(&'a PausedSession),
}

fn prioritized_activity_records(data: &DashboardData) -> Vec<ActivityRecord<'_>> {
    let pause_only = data.pause_only_sessions();
    let final_sessions = data
        .receipts
        .iter()
        .filter(|receipt| receipt.receipt.body.final_receipt)
        .map(|receipt| receipt.receipt.body.session_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut records = Vec::with_capacity(data.receipts.len().saturating_add(pause_only.len()));
    records.extend(
        data.receipts
            .iter()
            .filter(|receipt| {
                !receipt.receipt.body.final_receipt
                    && !final_sessions.contains(receipt.receipt.body.session_id.as_str())
            })
            .map(ActivityRecord::Receipt),
    );
    records.extend(pause_only.into_iter().map(ActivityRecord::Paused));
    records.extend(
        data.receipts
            .iter()
            .filter(|receipt| receipt.receipt.body.final_receipt)
            .map(ActivityRecord::Receipt),
    );
    records
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RouteStateKind {
    Accepting,
    Mixed,
    Blocked,
    Failed,
    Capacity,
    Draining,
    Stale,
    Waiting,
}

#[derive(Clone, Debug)]
struct RouteState {
    kind: RouteStateKind,
    label: &'static str,
    tone: &'static str,
    explanation: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FreshnessWindow {
    observed_at_millis: u64,
    expires_at_millis: u64,
}

fn heartbeat_freshness_window(
    data: &DashboardData,
    entry: &ProviderTableEntry,
) -> Option<FreshnessWindow> {
    entry.heartbeat.as_ref()?;
    let age = entry.heartbeat_age_millis?;
    if age > data.provider_heartbeat_ttl_millis {
        return None;
    }
    let observed_at_millis = data.generated_at_millis.saturating_sub(age);
    Some(FreshnessWindow {
        observed_at_millis,
        expires_at_millis: observed_at_millis.saturating_add(data.provider_heartbeat_ttl_millis),
    })
}

fn earliest_freshness_window<'a>(
    data: &DashboardData,
    entries: impl IntoIterator<Item = &'a ProviderTableEntry>,
) -> Option<FreshnessWindow> {
    entries
        .into_iter()
        .filter_map(|entry| heartbeat_freshness_window(data, entry))
        .min_by_key(|window| window.expires_at_millis)
}

fn timestamp_freshness_window(
    observed_at_seconds: u64,
    ttl_seconds: u64,
) -> Option<FreshnessWindow> {
    let observed_at_millis = observed_at_seconds.saturating_mul(1_000);
    let expires_at_millis = observed_at_millis.saturating_add(ttl_seconds.saturating_mul(1_000));
    (now_millis_u64() <= expires_at_millis).then_some(FreshnessWindow {
        observed_at_millis,
        expires_at_millis,
    })
}

fn payment_freshness_window(data: &DashboardData) -> Option<FreshnessWindow> {
    data.payment_observed_at()
        .and_then(|timestamp| timestamp_freshness_window(timestamp, PAYMENT_SNAPSHOT_FRESH_SECONDS))
}

fn earnings_freshness_window(
    snapshot: &GatewayProviderEarningsSnapshot,
) -> Option<FreshnessWindow> {
    if snapshot.last_error.is_some() {
        return None;
    }
    snapshot.refreshed_at_seconds.and_then(|timestamp| {
        timestamp_freshness_window(timestamp, EARNINGS_SNAPSHOT_FRESH_SECONDS)
    })
}

fn progress_freshness_window(progress: &DashboardProviderLoadProgress) -> Option<FreshnessWindow> {
    if !progress_is_fresh(progress) {
        return None;
    }
    Some(FreshnessWindow {
        observed_at_millis: progress.updated_at_ms,
        expires_at_millis: progress
            .updated_at_ms
            .saturating_add(DASHBOARD_PROVIDER_PROGRESS_ONLY_TTL_MS),
    })
}

fn provider_progress_freshness_window(data: &DashboardData) -> Option<FreshnessWindow> {
    let provider = data.local_provider_id.as_deref()?;
    progress_freshness_window(latest_provider_progress(data, provider)?)
}

fn model_freshness_window(data: &DashboardData, model: &GatewayModel) -> Option<FreshnessWindow> {
    earliest_freshness_window(
        data,
        model
            .mayhem
            .route_candidates
            .iter()
            .filter_map(|candidate| dashboard_entry_for_route(&data.entries, candidate)),
    )
}

fn volatile_text(value: &str, window: FreshnessWindow, expired_text: &str) -> String {
    format!(
        r#"<span data-volatile-value data-observed-at-ms="{}" data-volatile-expires-at-ms="{}" data-expired-text="{}">{}</span>"#,
        window.observed_at_millis,
        window.expires_at_millis,
        html_escape(expired_text),
        html_escape(value),
    )
}

fn relative_time(window: FreshnessWindow) -> String {
    format!(
        r#"<span data-relative-time data-observed-at-ms="{}">{} ago</span>"#,
        window.observed_at_millis,
        html_escape(&format_millis_age(data_now_age_millis(
            window.observed_at_millis
        ))),
    )
}

fn volatile_relative_label(prefix: &str, window: FreshnessWindow, expired_text: &str) -> String {
    format!(
        r#"<span data-volatile-value data-observed-at-ms="{}" data-volatile-expires-at-ms="{}" data-expired-text="{}">{} {}</span>"#,
        window.observed_at_millis,
        window.expires_at_millis,
        html_escape(expired_text),
        html_escape(prefix),
        relative_time(window),
    )
}

fn volatile_age(window: FreshnessWindow, expired_text: &str) -> String {
    format!(
        r#"<span data-volatile-value data-observed-at-ms="{}" data-volatile-expires-at-ms="{}" data-expired-text="{}">{}</span>"#,
        window.observed_at_millis,
        window.expires_at_millis,
        html_escape(expired_text),
        relative_time(window),
    )
}

fn volatile_status_badge(
    label: &str,
    tone: &str,
    window: FreshnessWindow,
    expired_text: &str,
) -> String {
    format!(
        r#"<span class="status-badge {}" data-volatile-status data-observed-at-ms="{}" data-volatile-expires-at-ms="{}" data-expired-text="{}">{}</span>"#,
        html_escape(tone),
        window.observed_at_millis,
        window.expires_at_millis,
        html_escape(expired_text),
        html_escape(label),
    )
}

fn payment_freshness_markup(data: &DashboardData) -> String {
    payment_freshness_window(data).map_or_else(
        || html_escape(&data.payment_freshness()),
        |window| {
            volatile_relative_label(
                "Ledger snapshot refreshed",
                window,
                "Ledger snapshot expired; refresh to reconfirm",
            )
        },
    )
}

fn data_now_age_millis(observed_at_millis: u64) -> u64 {
    now_millis_u64().saturating_sub(observed_at_millis)
}

fn route_operational_state(entry: &ProviderTableEntry) -> RouteState {
    let Some(heartbeat) = entry.heartbeat.as_ref() else {
        return if let Some(age) = entry.heartbeat_age_millis {
            RouteState {
                kind: RouteStateKind::Stale,
                label: "Telemetry delayed",
                tone: "warn",
                explanation: format!("Last heartbeat received {} ago", format_millis_age(age)),
            }
        } else {
            RouteState {
                kind: RouteStateKind::Waiting,
                label: "Waiting for heartbeat",
                tone: "",
                explanation: "No heartbeat has been received for this route".to_owned(),
            }
        };
    };
    let age = entry
        .heartbeat_age_millis
        .map(format_millis_age)
        .unwrap_or_else(|| "just now".to_owned());
    if !heartbeat.accepting_new {
        return RouteState {
            kind: RouteStateKind::Draining,
            label: "Draining",
            tone: "warn",
            explanation: format!("Fresh heartbeat {age}; new work is not being accepted"),
        };
    }
    if heartbeat.slots.active >= heartbeat.slots.max || heartbeat.q.free_slots == 0 {
        return RouteState {
            kind: RouteStateKind::Capacity,
            label: "At capacity",
            tone: "warn",
            explanation: format!(
                "Fresh heartbeat {age}; {} of {} slots active, queue {}",
                heartbeat.slots.active, heartbeat.slots.max, heartbeat.q.engine_backlog
            ),
        };
    }
    RouteState {
        kind: RouteStateKind::Accepting,
        label: "Capacity advertised",
        tone: "good",
        explanation: format!(
            "Fresh heartbeat {age}; {} free slot{}",
            heartbeat.q.free_slots,
            if heartbeat.q.free_slots == 1 { "" } else { "s" }
        ),
    }
}

pub(super) fn render_dashboard_product_page(
    state: &GatewayState,
    expires_in_seconds: u64,
    origin: &str,
    query: &DashboardQuery,
    page: DashboardProductPage,
) -> String {
    let data = DashboardData::from_state(state);
    let inner = match page {
        DashboardProductPage::Home => home_page(&data, expires_in_seconds),
        DashboardProductPage::Playground => {
            playground_page(&data, expires_in_seconds, query.model.as_deref())
        }
        DashboardProductPage::Models => {
            models_page(&data, expires_in_seconds, query.page.as_deref())
        }
        DashboardProductPage::Activity => {
            activity_page(&data, expires_in_seconds, query.page.as_deref())
        }
        DashboardProductPage::Wallet => wallet_page(&data, expires_in_seconds),
        DashboardProductPage::Connect => {
            connect_page(&data, expires_in_seconds, origin, query.page.as_deref())
        }
        DashboardProductPage::Earn => earn_overview_page(
            &data,
            expires_in_seconds,
            query.provider.as_deref(),
            query.page.as_deref(),
        ),
        DashboardProductPage::EarnJobs => earn_jobs_page(
            &data,
            expires_in_seconds,
            query.provider.as_deref(),
            query.page.as_deref(),
        ),
        DashboardProductPage::EarnMachines => earn_machines_page(
            &data,
            expires_in_seconds,
            query.provider.as_deref(),
            query.page.as_deref(),
        ),
        DashboardProductPage::EarnOpportunities => earn_opportunities_page(
            &data,
            expires_in_seconds,
            query.provider.as_deref(),
            query.page.as_deref(),
        ),
        DashboardProductPage::EarnEarnings => {
            earn_earnings_page(&data, expires_in_seconds, query.provider.as_deref())
        }
        DashboardProductPage::EarnReliability => earn_reliability_page(
            &data,
            expires_in_seconds,
            query.provider.as_deref(),
            query.page.as_deref(),
        ),
        DashboardProductPage::Network => network_overview_page(&data, expires_in_seconds),
        DashboardProductPage::NetworkModels => {
            network_models_page(&data, expires_in_seconds, query.page.as_deref())
        }
        DashboardProductPage::NetworkProviders => {
            network_providers_page(&data, expires_in_seconds, query.page.as_deref())
        }
        DashboardProductPage::NetworkMarkets => {
            network_markets_page(&data, expires_in_seconds, query.page.as_deref())
        }
        DashboardProductPage::NetworkActivity => {
            network_activity_page(&data, expires_in_seconds, query.page.as_deref())
        }
        DashboardProductPage::NetworkEvidence => network_evidence_page(
            &data,
            expires_in_seconds,
            query.page.as_deref(),
            query.probe_page.as_deref(),
        ),
        DashboardProductPage::Help => help_page(&data, expires_in_seconds),
        DashboardProductPage::Settings => settings_page(&data, expires_in_seconds),
    };
    dashboard_html_document(page.title(), &inner)
}

pub(super) fn dashboard_evidence_payload(
    state: &GatewayState,
    query: &DashboardQuery,
) -> Option<Value> {
    let data = DashboardData::from_state(state);
    match query.kind.as_deref()?.trim() {
        "model" => {
            let model_id = query.id.as_deref().or(query.model.as_deref())?;
            let model = data.models.iter().find(|model| model.id == model_id)?;
            let availability = model_availability(&data, model);
            let route_sources = model
                .mayhem
                .route_candidates
                .iter()
                .map(|candidate| {
                    let entry = dashboard_entry_for_route(&data.entries, candidate);
                    let freshness =
                        entry.and_then(|entry| heartbeat_freshness_window(&data, entry));
                    json!({
                        "catalog_route": candidate,
                        "provider_table_entry": entry,
                        "heartbeat_observed_at_millis": freshness.map(|window| window.observed_at_millis),
                        "heartbeat_expires_at_millis": freshness.map(|window| window.expires_at_millis),
                    })
                })
                .collect::<Vec<_>>();
            let matched_heartbeats = route_sources
                .iter()
                .filter(|source| {
                    source
                        .get("heartbeat_observed_at_millis")
                        .is_some_and(|value| !value.is_null())
                })
                .count();
            Some(evidence_payload(
                data.generated_at_millis,
                "Model evidence",
                &model.id,
                "Advertised state is a point-in-time interpretation of matching catalog routes and heartbeat snapshots, not a guarantee that a request will route.",
                vec![
                    (
                        "Catalog source",
                        model.mayhem.source.clone(),
                        "Catalog contract",
                    ),
                    (
                        "Canonical routes",
                        model.mayhem.route_candidates.len().to_string(),
                        "Catalog contract",
                    ),
                    (
                        "Advertised state",
                        availability.label.to_owned(),
                        "Interpretation derived from matching route snapshots inside the configured heartbeat TTL",
                    ),
                    (
                        "Fresh heartbeat coverage",
                        format!(
                            "{matched_heartbeats} of {}",
                            count_noun(
                                model.mayhem.route_candidates.len() as u64,
                                "canonical route"
                            )
                        ),
                        "Matching provider-table entries observed when this evidence payload was generated",
                    ),
                ],
                json!({
                    // Preserve the compact model identifier used by existing evidence
                    // consumers while exposing the complete source snapshots below.
                    "id": &model.id,
                    "generated_at_millis": data.generated_at_millis,
                    "heartbeat_ttl_millis": data.provider_heartbeat_ttl_millis,
                    "catalog_model": model,
                    "matching_route_sources": route_sources,
                }),
            ))
        }
        "receipt" => {
            let session_id = query.id.as_deref()?;
            let receipt = data
                .receipts
                .iter()
                .find(|receipt| receipt.receipt.body.session_id == session_id)?;
            let body = &receipt.receipt.body;
            let state = if body.final_receipt {
                "Final receipt"
            } else {
                "Non-final receipt"
            };
            let usage = receipt_usage_context(&body.usage);
            let charge_label = if body.final_receipt {
                "Final cumulative charge"
            } else {
                "Cumulative charge at checkpoint"
            };
            let interpretation = if body.final_receipt {
                "This signed final receipt completes metering for the recorded session; it is not a payout or settlement record."
            } else {
                "This signed non-final checkpoint records cumulative metering only; it does not prove that execution is still active."
            };
            Some(evidence_payload(
                data.generated_at_millis,
                "Receipt evidence",
                &body.session_id,
                interpretation,
                vec![
                    ("Metering state", state.to_owned(), "Signed route receipt"),
                    ("Model", body.model_id.clone(), "Signed route receipt"),
                    ("Rail", body.rail.clone(), "Signed route receipt"),
                    ("Provider", body.provider.clone(), "Signed route receipt"),
                    (
                        charge_label,
                        format_au_usd(body.au_owed_cum),
                        "Signed au_owed_cum at this receipt sequence; cumulative metering, not payout state",
                    ),
                    (
                        "Usage context",
                        usage,
                        "Nonzero units from the signed receipt usage map",
                    ),
                    (
                        "Receipt sequence",
                        body.seq.to_string(),
                        "Signed route receipt checkpoint sequence",
                    ),
                ],
                json!(receipt),
            ))
        }
        "paused" => {
            let session_id = query.id.as_deref()?;
            let paused = data
                .paused_sessions
                .iter()
                .find(|paused| paused.session_id == session_id)?;
            Some(evidence_payload(
                data.generated_at_millis,
                "Paused-session evidence",
                &paused.session_id,
                "This retained local pause record explains why the gateway stopped advancing the session; it does not claim that execution is still active.",
                vec![
                    ("State", "Paused".to_owned(), "Local gateway record"),
                    ("Reason", paused.reason.clone(), "Local gateway record"),
                ],
                json!(paused),
            ))
        }
        "probe" => {
            let probe_id = query.id.as_deref()?;
            let probe = data
                .probes
                .iter()
                .find(|probe| probe.probe_id == probe_id)?;
            Some(evidence_payload(
                data.generated_at_millis,
                "Verification probe evidence",
                &probe.probe_id,
                "This stored probe reports one recorded verification result and method; it is not a live provider-health claim.",
                vec![
                    ("Model", probe.model_id.clone(), "Stored verification probe"),
                    (
                        "Provider",
                        probe.provider.clone(),
                        "Stored verification probe",
                    ),
                    (
                        "Method",
                        probe.verification_method.clone(),
                        "Stored verification probe",
                    ),
                    (
                        "Result",
                        if probe.pass { "Passed" } else { "Failed" }.to_owned(),
                        "Stored verification probe",
                    ),
                ],
                json!(probe),
            ))
        }
        "earning" => {
            let provider = data.local_provider_id.as_deref()?;
            let rail = query.rail.as_deref()?;
            let entry = data.earnings.entries.iter().find(|entry| {
                entry.get("provider").and_then(Value::as_str) == Some(provider)
                    && entry.get("rail").and_then(Value::as_str) == Some(rail)
            })?;
            let epoch = entry
                .get("updated_epoch")
                .map(value_display)
                .unwrap_or_else(|| "Unavailable".to_owned());
            let payout = entry
                .get("last_payout_msb_tx_hash")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("Not provided");
            Some(evidence_payload(
                data.generated_at_millis,
                "Canonical earnings evidence",
                provider,
                "This is the latest canonical ledger snapshot known to the gateway, kept separate from live capacity and projected earnings.",
                vec![
                    ("Rail", rail.to_owned(), "Canonical ledger snapshot"),
                    ("Ledger epoch", epoch, "Canonical ledger snapshot"),
                    (
                        "Last payout reference",
                        payout.to_owned(),
                        "Canonical ledger snapshot",
                    ),
                ],
                entry.clone(),
            ))
        }
        "route" => {
            let provider = query.provider.as_deref()?;
            let enclave = query.enclave.as_deref()?;
            let room = query.room.as_deref()?;
            let model_filter = query.model.as_deref();
            let catalog_route = data.models.iter().find_map(|model| {
                if model_filter.is_some_and(|selected| selected != model.id) {
                    return None;
                }
                model.mayhem.route_candidates.iter().find_map(|candidate| {
                    (candidate.provider == provider
                        && candidate.enclave_id == enclave
                        && candidate.room_id == room)
                        .then_some((model, candidate))
                })
            });
            let entry = data.entries.iter().find(|entry| {
                entry.key.provider == provider
                    && entry.key.enclave_id == enclave
                    && entry.key.room_id == room
                    && model_filter.is_none_or(|model| entry.contract.model_id == model)
            });
            if catalog_route.is_none() && entry.is_none() {
                return None;
            }
            let model_id = catalog_route
                .map(|(model, _)| model.id.as_str())
                .or_else(|| entry.map(|entry| entry.contract.model_id.as_str()))
                .unwrap_or("Unknown model");
            let state = entry.map(route_operational_state).unwrap_or(RouteState {
                kind: RouteStateKind::Waiting,
                label: "No provider-table entry",
                tone: "",
                explanation: "The canonical route is not present in the current provider table."
                    .to_owned(),
            });
            let source_snapshot = match (catalog_route, entry) {
                (Some((_, candidate)), Some(entry)) => {
                    json!({"catalog_route": candidate, "provider_table_entry": entry})
                }
                (Some((_, candidate)), None) => {
                    json!({"catalog_route": candidate, "provider_table_entry": null})
                }
                (None, Some(entry)) => {
                    json!({"catalog_route": null, "provider_table_entry": entry})
                }
                (None, None) => unreachable!(),
            };
            let freshness = entry.and_then(|entry| heartbeat_freshness_window(&data, entry));
            let raw = json!({
                "generated_at_millis": data.generated_at_millis,
                "heartbeat_ttl_millis": data.provider_heartbeat_ttl_millis,
                "heartbeat_observed_at_millis": freshness.map(|window| window.observed_at_millis),
                "heartbeat_expires_at_millis": freshness.map(|window| window.expires_at_millis),
                "sources": source_snapshot,
            });
            Some(evidence_payload(
                data.generated_at_millis,
                "Provider route evidence",
                model_id,
                "Advertised state is derived at generation time from catalog identity plus heartbeat freshness, acceptance, and capacity.",
                vec![
                    (
                        "Provider",
                        provider.to_owned(),
                        "Catalog and provider-table identity",
                    ),
                    (
                        "Enclave",
                        enclave.to_owned(),
                        "Catalog and provider-table identity",
                    ),
                    (
                        "Room",
                        room.to_owned(),
                        "Catalog and provider-table identity",
                    ),
                    (
                        "Advertised state",
                        state.label.to_owned(),
                        "Derived from heartbeat freshness, acceptance, and capacity",
                    ),
                ],
                raw,
            ))
        }
        _ => None,
    }
}

pub(super) fn render_dashboard_evidence_page(payload: &Value) -> String {
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Evidence");
    let summary = payload
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("Requested dashboard evidence");
    let interpretation = payload
        .get("interpretation")
        .and_then(Value::as_str)
        .unwrap_or("Interpretation unavailable for this evidence payload.");
    let facts = payload
        .get("facts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|fact| {
            let label = fact.get("label").and_then(Value::as_str).unwrap_or("Fact");
            let value = fact
                .get("value")
                .map(value_display)
                .unwrap_or_else(|| "Unavailable".to_owned());
            let basis = fact
                .get("basis")
                .and_then(Value::as_str)
                .unwrap_or("Source unavailable");
            format!(
                r#"<div class="verify-fact"><span>{}</span><strong>{}</strong><small>{}</small></div>"#,
                html_escape(label),
                html_escape(&value),
                html_escape(basis)
            )
        })
        .collect::<String>();
    let raw = serde_json::to_string_pretty(payload.get("raw").unwrap_or(&Value::Null))
        .unwrap_or_else(|_| "null".to_owned());
    let body = format!(
        r##"<nav class="skip-links" aria-label="Skip links"><a class="skip-link" href="#main-content">Skip to evidence</a></nav><main class="evidence-standalone" id="main-content" tabindex="-1"><a class="soft-button" href="/mayhem/dashboard">&larr; Dashboard</a><section class="panel section-gap"><header class="panel-head"><div class="panel-title"><p class="page-eyebrow">Verifiable snapshot</p><h1>{}</h1><p>{}</p></div></header><div class="verify-body evidence-page-body"><p class="notice">{}</p><section class="verify-level"><h2>Structured facts</h2><div class="verify-grid">{facts}</div></section><section class="verify-level"><h2>Raw gateway snapshot</h2><pre class="raw-evidence">{}</pre></section></div><footer class="panel-footer"><span>This page shows one authenticated snapshot, loaded only when requested.</span><a href="/mayhem/dashboard">Return home</a></footer></section></main>"##,
        html_escape(title),
        html_escape(summary),
        html_escape(interpretation),
        html_escape(&raw),
    );
    dashboard_html_document("Evidence", &body)
}

fn evidence_payload(
    generated_at_millis: u64,
    title: &str,
    summary: &str,
    interpretation: &str,
    facts: Vec<(&str, String, &str)>,
    raw: Value,
) -> Value {
    json!({
        "schema_version": 1,
        "generated_at_millis": generated_at_millis,
        "title": title,
        "summary": summary,
        "interpretation": interpretation,
        "facts": facts.into_iter().map(|(label, value, basis)| json!({
            "label": label,
            "value": value,
            "basis": basis,
        })).collect::<Vec<_>>(),
        "raw": raw,
    })
}

fn receipt_usage_context(usage: &ReceiptUsage) -> String {
    if usage.units().is_empty() {
        return "No nonzero usage units recorded".to_owned();
    }
    usage
        .units()
        .iter()
        .map(|(unit, value)| format!("{unit}: {value}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn value_display(value: &Value) -> String {
    match value {
        Value::Null => "Unavailable".to_owned(),
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        value => serde_json::to_string(value).unwrap_or_else(|_| "Unavailable".to_owned()),
    }
}

fn evidence_href(kind: &str, parameters: &[(&str, &str)]) -> String {
    let mut href = format!(
        "/mayhem/dashboard/evidence?kind={}",
        dashboard_url_encode(kind)
    );
    for (key, value) in parameters {
        href.push('&');
        href.push_str(&dashboard_url_encode(key));
        href.push('=');
        href.push_str(&dashboard_url_encode(value));
    }
    href
}

fn evidence_link(href: &str, label: &str, context: &str) -> String {
    format!(
        r#"<a class="quiet-button" href="{}" data-evidence-url aria-label="{} evidence for {}" aria-haspopup="dialog" aria-controls="dashboard-evidence-dialog">{}</a>"#,
        html_escape(href),
        html_escape(label),
        html_escape(context),
        html_escape(label)
    )
}

fn volatile_capacity_window(data: &DashboardData) -> Option<FreshnessWindow> {
    earliest_freshness_window(data, data.entries.iter())
}

fn mark_volatile_capacity_badges(content: &str, window: Option<FreshnessWindow>) -> String {
    let Some(window) = window else {
        return content.to_owned();
    };
    [
        ("good", "Capacity advertised"),
        ("warn", "At capacity"),
        ("warn", "Draining"),
    ]
    .into_iter()
    .fold(content.to_owned(), |content, (tone, label)| {
        content.replace(
            &format!(r#"<span class="status-badge {tone}">{label}</span>"#),
            &format!(
                r#"<span class="status-badge {tone}" data-volatile-status data-observed-at-ms="{}" data-volatile-expires-at-ms="{}" data-expired-text="Refresh to reconfirm">{label}</span>"#,
                window.observed_at_millis, window.expires_at_millis,
            ),
        )
    })
}

fn volatile_page_status_marker(
    data: &DashboardData,
    page: DashboardAppPage,
    status: &str,
) -> String {
    let capacity_claim = matches!(
        status,
        "Capacity advertised"
            | "No advertised capacity"
            | "Routes unavailable"
            | "Supply exceptions"
            | "Route status"
            | "Online and accepting work"
            | "Online with route issues"
            | "Online with preparation issue"
            | "Live routes with preparation issue"
            | "At capacity"
            | "Draining"
            | "Multiple route issues"
    );
    let progress_claim = matches!(
        status,
        "Preparing a model"
            | "Prepared; waiting for heartbeat"
            | "Setup blocked by model failure"
            | "Online with preparation issue"
            | "Live routes with preparation issue"
    );
    let capacity_window = capacity_claim
        .then(|| volatile_capacity_window(data))
        .flatten();
    let progress_window = progress_claim
        .then(|| provider_progress_freshness_window(data))
        .flatten();
    let operational_window = [capacity_window, progress_window]
        .into_iter()
        .flatten()
        .min_by_key(|window| window.expires_at_millis);
    let (window, expired_summary) = if let Some(window) = operational_window {
        let summary = if progress_window.is_some() {
            "Provider preparation evidence expired in this tab. Refresh to reconfirm."
        } else {
            "Live capacity evidence expired in this tab. Refresh to reconfirm."
        };
        (Some(window), summary)
    } else if page == DashboardAppPage::Wallet
        && matches!(
            status,
            "Ledger snapshot current" | "Ready to use" | "Funding needed"
        )
    {
        (
            payment_freshness_window(data),
            "Ledger evidence expired in this tab. Refresh to reconfirm.",
        )
    } else if page == DashboardAppPage::Earn
        && matches!(status, "Ledger snapshot current" | "No earnings records")
    {
        (
            earnings_freshness_window(&data.earnings),
            "Earnings ledger evidence expired in this tab. Refresh to reconfirm.",
        )
    } else {
        return String::new();
    };
    window.map_or_else(String::new, |window| {
        format!(
            r#"<span hidden data-page-status-freshness data-observed-at-ms="{}" data-expires-at-ms="{}" data-expired-summary="{}"></span>"#,
            window.observed_at_millis,
            window.expires_at_millis,
            html_escape(expired_summary),
        )
    })
}

// Keeping each page's product copy together at the call site makes the route
// hierarchy auditable; this helper intentionally mirrors those shell slots.
#[allow(clippy::too_many_arguments)]
fn shell(
    data: &DashboardData,
    expires: u64,
    page: DashboardAppPage,
    eyebrow: &str,
    heading: &str,
    summary: &str,
    status: &str,
    tone: &str,
    actions: &str,
    content: &str,
) -> String {
    shell_impl(
        data, expires, page, eyebrow, heading, summary, status, tone, actions, content, false,
    )
}

// Table-dense routes get the wider content tier; prose routes keep the
// bounded reading measure.
#[allow(clippy::too_many_arguments)]
fn shell_wide(
    data: &DashboardData,
    expires: u64,
    page: DashboardAppPage,
    eyebrow: &str,
    heading: &str,
    summary: &str,
    status: &str,
    tone: &str,
    actions: &str,
    content: &str,
) -> String {
    shell_impl(
        data, expires, page, eyebrow, heading, summary, status, tone, actions, content, true,
    )
}

#[allow(clippy::too_many_arguments)]
fn shell_impl(
    data: &DashboardData,
    expires: u64,
    page: DashboardAppPage,
    eyebrow: &str,
    heading: &str,
    summary: &str,
    status: &str,
    tone: &str,
    actions: &str,
    content: &str,
    wide: bool,
) -> String {
    // A required update renders a red banner; a green topbar dot beside it
    // would send mixed signals, so the tone is capped at warn.
    let tone = if tone == "good"
        && data
            .update_notice
            .as_ref()
            .is_some_and(|notice| notice.level == "required")
    {
        "warn"
    } else {
        tone
    };
    let content = mark_volatile_capacity_badges(content, volatile_capacity_window(data));
    let status_freshness = volatile_page_status_marker(data, page, status);
    let content = if !matches!(page, DashboardAppPage::Home | DashboardAppPage::Settings) {
        format!(
            "{}{}{content}",
            global_update_attention(data),
            status_freshness
        )
    } else {
        format!("{status_freshness}{content}")
    };
    let content = keyboard_accessible_table_regions(&content);
    dashboard_app_shell(DashboardShell {
        page,
        eyebrow,
        heading,
        summary,
        status,
        status_tone: tone,
        actions,
        content: &content,
        footer: "Controls and evidence belong to this gateway process.",
        expires_in_seconds: expires,
        wide,
    })
}

fn keyboard_accessible_table_regions(content: &str) -> String {
    const WRAP_OPEN: &str = r#"<div class="data-table-wrap">"#;
    const CAPTION_OPEN: &str = r#"<caption class="sr-only">"#;
    const CAPTION_CLOSE: &str = "</caption>";
    const TABLE_CLOSE: &str = "</table>";

    if !content.contains(WRAP_OPEN) {
        return content.to_owned();
    }

    let mut rendered = String::with_capacity(content.len() + 160);
    let mut remaining = content;
    while let Some(wrap_offset) = remaining.find(WRAP_OPEN) {
        rendered.push_str(&remaining[..wrap_offset]);
        let after_wrap = &remaining[wrap_offset + WRAP_OPEN.len()..];
        let table_end = after_wrap.find(TABLE_CLOSE).unwrap_or(after_wrap.len());
        let caption = after_wrap
            .find(CAPTION_OPEN)
            .filter(|offset| *offset < table_end)
            .and_then(|caption_offset| {
                let value = &after_wrap[caption_offset + CAPTION_OPEN.len()..];
                value.find(CAPTION_CLOSE).map(|end| value[..end].trim())
            })
            .filter(|value| !value.is_empty())
            .unwrap_or("Data table");
        let label = format!("{caption}. Scroll horizontally to view all columns.");
        rendered.push_str(&format!(
            r#"<div class="data-table-wrap" role="region" tabindex="0" aria-label="{}">"#,
            html_escape(&label)
        ));
        remaining = after_wrap;
    }
    rendered.push_str(remaining);
    rendered
}

fn global_update_attention(data: &DashboardData) -> String {
    let Some(notice) = data.update_notice.as_ref() else {
        return String::new();
    };
    let tone = if notice.level == "required" {
        "danger"
    } else {
        "warn"
    };
    attention(
        tone,
        "!",
        if notice.level == "required" {
            "Update required"
        } else {
            "Update available"
        },
        &format!(
            "Installed {}. A newer app version (minimum {}) is required for {}.",
            notice.installed_app_version,
            notice.required_min_app_version,
            count_noun(notice.affected_model_count as u64, "model")
        ),
        Some(("Review update", "/mayhem/dashboard/settings")),
    )
}

fn home_page(data: &DashboardData, expires: u64) -> String {
    let accepting_models = data.accepting_models();
    let completed = data.completed_requests();
    let incomplete = data.incomplete_session_count();
    let required_update = data
        .update_notice
        .as_ref()
        .is_some_and(|notice| notice.level == "required");
    let credential_needed = data.requires_auth() && data.active_token_count() == 0;
    let funding_needed = data.payment_directory.is_some() && data.balance_au == 0;
    let (summary, status, tone) = if required_update {
        (
            "The gateway is running, but one or more catalog routes are blocked until Mayhem is updated.",
            "Update required",
            "danger",
        )
    } else if credential_needed {
        (
            "This gateway requires an active access token before Playground or connected apps can send requests.",
            "Credential needed",
            "warn",
        )
    } else if funding_needed {
        (
            "The gateway and model catalog are ready, but the current ledger balance is empty.",
            "Funding needed",
            "warn",
        )
    } else if accepting_models > 0 {
        (
            "At least one provider is online with free capacity right now. Final price and eligibility are confirmed when you send.",
            "Capacity advertised",
            "good",
        )
    } else if data.models.is_empty() {
        (
            "The gateway is running — it just has no models in its catalog yet.",
            "No models yet",
            "warn",
        )
    } else {
        (
            "This gateway is running, but no model currently has a fresh provider advertising free capacity.",
            "No advertised capacity",
            "warn",
        )
    };
    let attention = home_attention(data, accepting_models);
    let launch_paths = home_launch_paths(data);
    let payment_freshness = payment_freshness_markup(data);
    let balance_metric = if data.payment_directory.is_some() {
        metric_with_meta_html(
            "Ledger balance",
            &money_html(&format_au_usd(data.balance_au)),
            &payment_freshness,
            "Ledger",
        )
    } else {
        metric_status(
            "Ledger balance",
            &status_badge("Unavailable", "warn"),
            "The payment directory has not answered yet.",
            "Ledger",
        )
    };
    let metrics = format!(
        r##"<section class="metric-grid metric-grid--three" aria-label="Current gateway summary">{balance_metric}{}{}</section>"##,
        metric(
            "Completed requests",
            &completed.to_string(),
            &format!(
                "Final receipts in {}",
                data.history_scope().to_ascii_lowercase()
            ),
            "Activity",
        ),
        metric(
            "Open records",
            &incomplete.to_string(),
            "Sessions without a final receipt yet",
            "Activity",
        ),
    );
    let activity = recent_activity_panel(data, 3);
    let usage = home_usage_chart(data);
    let right = match home_secondary_panel(data) {
        HomeSecondaryPanel::Provider => provider_home_panel(data),
        HomeSecondaryPanel::FirstValue => activation_panel(data),
        HomeSecondaryPanel::None => String::new(),
    };
    let main_stack = format!(r##"{usage}{activity}"##);
    let content = if right.is_empty() {
        format!(r##"{attention}{launch_paths}{metrics}{main_stack}"##)
    } else {
        format!(
            r##"{attention}{launch_paths}{metrics}<section class="dashboard-layout"><div class="stack">{main_stack}</div><aside class="stack" aria-label="Next actions">{right}</aside></section>"##
        )
    };
    shell(
        data,
        expires,
        DashboardAppPage::Home,
        "Overview",
        "Overview",
        summary,
        status,
        tone,
        "",
        &content,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HomeSecondaryPanel {
    Provider,
    FirstValue,
    None,
}

fn home_secondary_panel(data: &DashboardData) -> HomeSecondaryPanel {
    let provider_entries = data.provider_entries(None);
    let provider_state = data
        .local_provider_id
        .as_ref()
        .map(|_| provider_page_state(data, None, &provider_entries).kind);
    let provider_progress_needs_attention = data
        .local_provider_id
        .as_deref()
        .and_then(|provider| latest_provider_progress(data, provider))
        .is_some_and(|progress| {
            progress_is_failed(progress)
                || !progress_is_terminal(progress)
                || provider_entries.is_empty()
        });

    choose_home_secondary_panel(
        data.completed_requests(),
        provider_state,
        provider_progress_needs_attention,
        data.has_provider_evidence(),
    )
}

fn choose_home_secondary_panel(
    completed_requests: usize,
    provider_state: Option<RouteStateKind>,
    provider_progress_needs_attention: bool,
    has_provider_evidence: bool,
) -> HomeSecondaryPanel {
    let provider_state_needs_attention =
        provider_state.is_some_and(|state| state != RouteStateKind::Accepting);
    let active_configured_provider = provider_state == Some(RouteStateKind::Accepting);

    if provider_progress_needs_attention
        || provider_state_needs_attention
        || active_configured_provider
        || has_provider_evidence
    {
        HomeSecondaryPanel::Provider
    } else if completed_requests == 0 {
        // Non-final checkpoints are recovery evidence, not completed first value.
        HomeSecondaryPanel::FirstValue
    } else {
        HomeSecondaryPanel::None
    }
}

fn home_attention(data: &DashboardData, accepting_models: usize) -> String {
    if let Some(notice) = data
        .update_notice
        .as_ref()
        .filter(|notice| notice.level == "required")
    {
        return attention(
            "danger",
            "!",
            "Update blocks catalog routes",
            &format!(
                "Mayhem {} is installed; catalog minimum {} affects {} model(s).",
                notice.installed_app_version,
                notice.required_min_app_version,
                notice.affected_model_count
            ),
            Some(("Review update", "/mayhem/dashboard/settings")),
        );
    }
    if data.payment_directory.is_some() && data.balance_au == 0 {
        return attention(
            "warn",
            "!",
            "Your ledger balance is empty",
            "Add funds before sending paid work. Billing shows the exact rail and a reviewable command without starting a transaction in the browser.",
            Some(("Open Billing", "/mayhem/dashboard/wallet")),
        );
    }
    if accepting_models == 0 && !data.models.is_empty() {
        return attention(
            "warn",
            "!",
            "No provider advertises accepting capacity",
            "Freshness, draining state, and advertised slot capacity are all checked before a route is called available.",
            Some(("Inspect supply", "/mayhem/dashboard/network/providers")),
        );
    }
    String::new()
}

fn home_launch_paths(data: &DashboardData) -> String {
    let credential_ready = !data.requires_auth() || data.active_token_count() > 0;
    let funds_ready = data.payment_directory.is_none() || data.balance_au > 0;
    let capacity_ready = data.accepting_models() > 0;
    let user_ready = credential_ready && funds_ready && capacity_ready;
    let user_status = if !credential_ready {
        (
            "Credential needed",
            "warn",
            "/mayhem/dashboard/connect",
            "Set up access",
        )
    } else if !funds_ready {
        (
            "Funding needed",
            "warn",
            "/mayhem/dashboard/wallet",
            "Add funds",
        )
    } else if !capacity_ready {
        (
            "Waiting for capacity",
            "warn",
            "/mayhem/dashboard/models",
            "Check availability",
        )
    } else {
        (
            "Ready",
            "good",
            "/mayhem/dashboard/playground",
            if data.completed_requests() > 0 {
                "Continue in Playground"
            } else {
                "Try your first request"
            },
        )
    };
    // A path that cannot currently succeed never gets the brightest button.
    let user_button_class = if capacity_ready || !credential_ready || !funds_ready {
        "primary-button"
    } else {
        "soft-button"
    };
    let entries = data.provider_entries(None);
    let provider_state = provider_page_state(data, None, &entries);
    let provider_started = data.local_provider_id.is_some() || data.has_provider_evidence();
    let provider_status = if provider_started {
        (
            provider_state.label,
            provider_state.tone,
            "Open provider workspace",
        )
    } else {
        ("Not set up", "", "Check this machine")
    };
    format!(
        r##"<section class="launch-paths" aria-label="Choose what you want to do"><article class="launch-path-card {}"><div class="launch-path-icon" aria-hidden="true">&#10022;</div><div class="launch-path-copy"><span class="status-badge {}">{}</span><h2>Use AI</h2><p>Ask a question, get a useful result, and inspect its signed receipt.</p></div><a class="{user_button_class}" href="{}" data-product-event="use_ai_path_opened">{}</a></article><article class="launch-path-card"><div class="launch-path-icon" aria-hidden="true">&#9881;</div><div class="launch-path-copy"><span class="status-badge {}">{}</span><h2>Earn with this machine</h2><p>Run a model on this machine and watch each job through to settlement.</p></div><a class="soft-button" href="/mayhem/dashboard/earn" data-product-event="earn_path_opened">{}</a></article></section>"##,
        if user_ready { "is-ready" } else { "" },
        user_status.1,
        html_escape(user_status.0),
        user_status.2,
        html_escape(user_status.3),
        provider_status.1,
        html_escape(provider_status.0),
        html_escape(provider_status.2),
    )
}

fn home_usage_chart(data: &DashboardData) -> String {
    let today = now_secs() / 86_400;
    let mut days = [0usize; 7];
    for receipt in data
        .receipts
        .iter()
        .filter(|receipt| receipt.receipt.body.final_receipt)
    {
        let receipt_day = timestamp_seconds(receipt.receipt.body.ts) / 86_400;
        let age = today.saturating_sub(receipt_day);
        if age < 7 {
            days[6 - age as usize] += 1;
        }
    }
    let total = days.iter().sum::<usize>();
    if total == 0 {
        return String::new();
    }
    let max = days.iter().copied().max().unwrap_or(1).max(1);
    let bars = days
        .into_iter()
        .enumerate()
        .map(|(index, count)| {
            let level = if count == 0 {
                0
            } else {
                ((count * 10).div_ceil(max)).max(1)
            };
            let label = if index == 6 {
                "Today".to_owned()
            } else {
                format!("{}d ago", 6 - index)
            };
            format!(
                r##"<li aria-label="{label}: {count} completed request{}"><span class="usage-bar level-{level}"><span></span></span><strong>{count}</strong><small>{label}</small></li>"##,
                if count == 1 { "" } else { "s" },
            )
        })
        .collect::<String>();
    // A near-empty week reads as intentional, not broken, when it is named.
    let active_days = days.iter().filter(|count| **count > 0).count();
    let caption = if active_days <= 1 {
        format!(
            "First requests this week &middot; last 7 calendar days from {}",
            html_escape(&data.history_scope().to_ascii_lowercase())
        )
    } else {
        format!(
            "Last 7 calendar days from {}",
            html_escape(&data.history_scope().to_ascii_lowercase())
        )
    };
    format!(
        r##"<figure class="panel usage-chart"><figcaption class="panel-head"><div class="panel-title"><h2>Completed requests</h2><p>{caption}</p></div><strong>{total}</strong></figcaption><div class="panel-body"><ol class="usage-bars">{bars}</ol></div></figure>"##,
    )
}

fn activation_panel(data: &DashboardData) -> String {
    let gateway_done = true;
    let route_done = data.accepting_models() > 0;
    let request_done = data.completed_requests() > 0;
    let connect_done = !data.requires_auth() || data.active_token_count() > 0;
    let steps = [
        (gateway_done, "Gateway running", "This control surface loaded from the gateway successfully."),
        (route_done, "Provider capacity advertised", "A fresh heartbeat advertises accepting capacity; request eligibility is checked later."),
        (connect_done, "Connection ready", "Authentication is optional or an active token exists."),
        (request_done, "Complete a first request", "Use Playground to produce signed metering evidence."),
    ];
    let done = steps.iter().filter(|(complete, _, _)| *complete).count();
    let active_index = steps.iter().position(|(complete, _, _)| !complete);
    let steps = steps
        .into_iter()
        .enumerate()
        .map(|(index, (complete, label, help))| {
            let current = Some(index) == active_index;
            let class_name = if complete {
                "done"
            } else if current {
                "active"
            } else {
                ""
            };
            let state = if complete {
                "Complete"
            } else if current {
                "Current"
            } else {
                "Not started"
            };
            let mark = if complete { "&#10003;" } else { "" };
            let volatile_step = if index == 1 && complete {
                volatile_capacity_window(data).map_or_else(String::new, |window| {
                    format!(
                        r#" data-volatile-step data-observed-at-ms="{}" data-expires-at-ms="{}""#,
                        window.observed_at_millis, window.expires_at_millis
                    )
                })
            } else {
                String::new()
            };
            format!(
                r##"<li class="check-step {class_name}"{volatile_step}><span class="check-mark" data-check-mark aria-hidden="true">{mark}</span><div class="check-copy"><span class="sr-only" data-check-state>{state}: </span><strong data-check-label>{}</strong><span data-check-help>{}</span></div></li>"##,
                html_escape(label),
                html_escape(help)
            )
        })
        .collect::<String>();
    format!(
        r##"<section class="panel"><header class="panel-head"><div class="panel-title"><h2>Getting started</h2><p>{done} of 4 confirmed from gateway state</p></div></header><div class="panel-body"><ol class="checklist">{steps}</ol></div><footer class="panel-footer"><span>Each step confirms itself from gateway evidence.</span></footer></section>"##
    )
}

fn provider_home_panel(data: &DashboardData) -> String {
    let entries = data.provider_entries(None);
    let state = provider_page_state(data, None, &entries);
    let slots = provider_slot_totals(data, &entries);
    let freshness = provider_freshness_window(data, &entries);
    let current_slots = provider_current_value(
        format!("{} / {}", slots.active, slots.max),
        &slots,
        freshness,
    );
    let current_queue = provider_current_value(slots.backlog.to_string(), &slots, freshness);
    let coverage = provider_coverage_notice(&slots, freshness);
    let explanation = freshness.map_or_else(
        || html_escape(&state.explanation),
        |window| volatile_text(&state.explanation, window, "Refresh to reconfirm"),
    );
    format!(
        r##"<section class="panel"><header class="panel-head"><div class="panel-title"><h2>Your provider</h2><p>The provider identity attached to this gateway</p></div><span class="status-badge {}">{}</span></header><div class="panel-body"><div class="fact-grid"><div class="fact"><span>Active slots</span><strong>{}</strong></div><div class="fact"><span>Queue</span><strong>{}</strong></div></div>{coverage}<p class="notice">{}</p></div><footer class="panel-footer"><span>{}</span><a href="/mayhem/dashboard/earn">Open Earn</a></footer></section>"##,
        state.tone,
        html_escape(state.label),
        current_slots,
        current_queue,
        explanation,
        count_noun(entries.len() as u64, "route"),
    )
}

fn recent_activity_panel(data: &DashboardData, limit: usize) -> String {
    let rows = prioritized_activity_records(data)
        .into_iter()
        .take(limit)
        .map(|record| match record {
            ActivityRecord::Receipt(receipt) => activity_row(receipt),
            ActivityRecord::Paused(paused) => paused_activity_row(paused),
        })
        .collect::<String>();
    let body = if rows.is_empty() {
        empty_block(
            "No requests recorded",
            "A completed Playground or connected-client request will appear here.",
            None,
        )
    } else {
        format!(r##"<div class="activity-list">{rows}</div>"##)
    };
    let stopping_cue = if data.incomplete_session_count() > 0 {
        "Open records listed first"
    } else {
        "No open records"
    };
    format!(
        r##"<section class="panel"><header class="panel-head"><div class="panel-title"><h2>Recent activity</h2><p>Open records first, then the latest final receipt per session</p></div><a class="quiet-button" href="/mayhem/dashboard/activity">View all</a></header>{body}<footer class="panel-footer"><span>{}</span><span>{stopping_cue}</span></footer></section>"##,
        count_noun(data.receipt_checkpoint_count as u64, "receipt record"),
    )
}

fn paused_activity_row(paused: &PausedSession) -> String {
    format!(
        r##"<div class="activity-row"><span class="activity-state failed" aria-hidden="true">!</span><div class="activity-main"><strong>Paused session</strong><span>{}</span></div><div class="activity-value"><strong>Needs review</strong><span>{}</span></div></div>"##,
        html_escape(short_text(&paused.session_id, 24).as_ref()),
        html_escape(short_text(&paused.reason, 42).as_ref()),
    )
}

fn activity_row(receipt: &StoredReceipt) -> String {
    let body = &receipt.receipt.body;
    let (icon, class_name, label) = if body.final_receipt {
        ("&#10003;", "", "Final receipt")
    } else {
        ("!", "pending", "Non-final receipt")
    };
    format!(
        r##"<div class="activity-row"><span class="activity-state {class_name}" aria-hidden="true">{icon}</span><div class="activity-main"><strong>{}</strong><span>{} &middot; {} ago</span></div><div class="activity-value" data-money><strong class="money-value">{}</strong><span>{label}</span></div></div>"##,
        html_escape(short_text(&body.model_id, 42).as_ref()),
        html_escape(short_text(&body.provider, 16).as_ref()),
        html_escape(&format_elapsed_since(timestamp_seconds(body.ts))),
        html_escape(&format_au_usd(body.au_owed_cum)),
    )
}

#[derive(Clone)]
struct DashboardModelLab {
    key: &'static str,
    name: String,
    initials: String,
}

fn dashboard_model_lab(model: &GatewayModel) -> DashboardModelLab {
    let search =
        format!("{} {} {}", model.id, model.mayhem.family, model.owned_by).to_ascii_lowercase();
    let known = [
        ("qwen", "Qwen", "Q", &["qwen", "tongyi-qianwen"] as &[&str]),
        ("hauhau", "HauHau", "HH", &["hauhau", "hauhaucs"] as &[&str]),
        (
            "deepmind",
            "Google DeepMind",
            "DM",
            &["deepmind"] as &[&str],
        ),
        ("google", "Google", "G", &["google", "gemma"] as &[&str]),
        (
            "openai",
            "OpenAI",
            "OA",
            &["openai/", "gpt-", "o1-", "o3-"] as &[&str],
        ),
        ("deepseek", "DeepSeek", "DS", &["deepseek"] as &[&str]),
        (
            "mistral",
            "Mistral AI",
            "M",
            &["mistral", "mixtral"] as &[&str],
        ),
        (
            "meta-ai",
            "Meta AI",
            "M",
            &["meta-llama", "meta/", "llama"] as &[&str],
        ),
        (
            "moonshot-ai",
            "Moonshot AI",
            "K",
            &["moonshot", "kimi"] as &[&str],
        ),
        ("minimax", "MiniMax", "MM", &["minimax"] as &[&str]),
        ("nvidia", "NVIDIA", "N", &["nvidia", "nemotron"] as &[&str]),
        ("z-ai", "Z.ai", "Z", &["z-ai", "zai", "glm-"] as &[&str]),
        (
            "huggingface",
            "Hugging Face",
            "HF",
            &["huggingface", "smollm"] as &[&str],
        ),
        (
            "microsoft",
            "Microsoft",
            "MS",
            &["microsoft", "phi-"] as &[&str],
        ),
        (
            "black-forest-labs",
            "Black Forest Labs",
            "BFL",
            &["black-forest", "flux"] as &[&str],
        ),
        ("baai", "BAAI", "BA", &["baai", "bge-"] as &[&str]),
        (
            "stability-ai",
            "Stability AI",
            "SA",
            &["stability", "stable-diffusion"] as &[&str],
        ),
        (
            "tencent",
            "Tencent Hunyuan",
            "T",
            &["tencent", "hunyuan"] as &[&str],
        ),
        ("jina-ai", "Jina AI", "J", &["jina"] as &[&str]),
        ("nomic-ai", "Nomic AI", "N", &["nomic"] as &[&str]),
        (
            "lightricks",
            "Lightricks",
            "L",
            &["lightricks", "ltx-"] as &[&str],
        ),
        ("resemble-ai", "Resemble AI", "R", &["resemble"] as &[&str]),
        ("ace-step", "ACE-Step", "AS", &["ace-step"] as &[&str]),
        (
            "deepreinforce",
            "DeepReinforce",
            "DR",
            &["deepreinforce"] as &[&str],
        ),
        ("empero-ai", "Empero AI", "E", &["empero"] as &[&str]),
        ("hexgrad", "Hexgrad", "H", &["hexgrad"] as &[&str]),
        ("huihui-ai", "Huihui AI", "H", &["huihui"] as &[&str]),
        ("lodestones", "Lodestones", "L", &["lodestones"] as &[&str]),
        (
            "tongyi-mai",
            "Tongyi-MAI",
            "TM",
            &["tongyi-mai", "tongyi/", "z-image"] as &[&str],
        ),
        ("wepiqx", "Wepiqx", "W", &["wepiqx"] as &[&str]),
    ];
    if let Some((key, name, initials, _)) = known
        .iter()
        .find(|(_, _, _, needles)| needles.iter().any(|needle| search.contains(needle)))
    {
        return DashboardModelLab {
            key,
            name: (*name).to_owned(),
            initials: (*initials).to_owned(),
        };
    }

    let raw_name = model
        .id
        .split('/')
        .next()
        .filter(|value| !value.is_empty() && *value != "workbench")
        .unwrap_or("Independent lab");
    let name = humanize_model_label(raw_name);
    let initials = name
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect::<String>()
        .to_ascii_uppercase();
    DashboardModelLab {
        key: "other",
        name,
        initials: if initials.is_empty() {
            "AI".to_owned()
        } else {
            initials
        },
    }
}

fn humanize_model_label(value: &str) -> String {
    let mut label = value.replace(['-', '_'], " ");
    if let Some(first) = label.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    label
}

fn dashboard_model_name(model: &GatewayModel) -> String {
    let mut value = model.id.rsplit('/').next().unwrap_or(&model.id);
    if model.id.starts_with("workbench/") {
        if let Some((prefix, remainder)) = value.split_once('-') {
            if prefix.chars().all(|character| character.is_ascii_digit()) {
                value = remainder;
            }
        }
        for prefix in ["hauhaucs-", "google-"] {
            if let Some(remainder) = value.strip_prefix(prefix) {
                value = remainder;
                break;
            }
        }
    }
    humanize_model_label(value)
}

fn dashboard_model_lab_icon(lab: &DashboardModelLab) -> String {
    let asset = match lab.key {
        "openai" => Some("openai-symbol.svg"),
        "ace-step" | "baai" | "black-forest-labs" | "deepreinforce" | "empero-ai" | "hexgrad"
        | "huihui-ai" | "jina-ai" | "lightricks" | "lodestones" | "microsoft" | "nomic-ai"
        | "resemble-ai" | "stability-ai" | "tencent" | "tongyi-mai" | "wepiqx" => {
            Some(match lab.key {
                "ace-step" => "ace-step.webp",
                "baai" => "baai.webp",
                "black-forest-labs" => "black-forest-labs.webp",
                "deepreinforce" => "deepreinforce.webp",
                "empero-ai" => "empero-ai.webp",
                "hexgrad" => "hexgrad.webp",
                "huihui-ai" => "huihui-ai.webp",
                "jina-ai" => "jina-ai.webp",
                "lightricks" => "lightricks.webp",
                "lodestones" => "lodestones.webp",
                "microsoft" => "microsoft.webp",
                "nomic-ai" => "nomic-ai.webp",
                "resemble-ai" => "resemble-ai.webp",
                "stability-ai" => "stability-ai.webp",
                "tencent" => "tencent.webp",
                "tongyi-mai" => "tongyi-mai.webp",
                _ => "wepiqx.webp",
            })
        }
        "hauhau" => Some("hauhau.svg"),
        "deepmind" => Some("deepmind.svg"),
        "deepseek" => Some("deepseek.svg"),
        "google" => Some("google.svg"),
        "huggingface" => Some("huggingface.svg"),
        "meta-ai" => Some("meta-ai.svg"),
        "minimax" => Some("minimax.svg"),
        "mistral" => Some("mistral.svg"),
        "moonshot-ai" => Some("moonshot-ai.svg"),
        "nvidia" => Some("nvidia.svg"),
        "qwen" => Some("qwen.svg"),
        "z-ai" => Some("z-ai.svg"),
        _ => None,
    };
    let contents = asset.map_or_else(
        || format!(r#"<span>{}</span>"#, html_escape(&lab.initials)),
        |asset| {
            let fit_class = if asset.ends_with(".svg") && lab.key != "hauhau" {
                " model-lab-image--contain"
            } else {
                ""
            };
            format!(
                r#"<img class="model-lab-image{fit_class}" src="/mayhem/dashboard/assets/brand/{asset}" alt="" width="32" height="32">"#
            )
        },
    );
    format!(
        r#"<span class="model-lab-mark model-lab--{}" aria-hidden="true">{contents}</span>"#,
        lab.key
    )
}

fn playground_model_mode(model: &GatewayModel) -> Option<&'static str> {
    match model.mayhem.model_class.as_str() {
        DEFAULT_MODEL_CLASS => Some("chat"),
        "image-generation" => Some("image"),
        "tts" => Some("speech"),
        _ => None,
    }
}

fn playground_mode_icon(mode: &str) -> &'static str {
    match mode {
        "image" => {
            r#"<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="3"/><circle cx="9" cy="10" r="2"/><path d="m5 18 5-5 3 3 2-2 4 4"/></svg>"#
        }
        "speech" => {
            r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M11 5 6.6 8.5H3.8a.8.8 0 0 0-.8.8v5.4a.8.8 0 0 0 .8.8h2.8L11 19Z"/><path d="M14.8 9.2a4.1 4.1 0 0 1 0 5.6M17.6 6.5a8 8 0 0 1 0 11"/></svg>"#
        }
        _ => {
            r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20 15a4 4 0 0 1-4 4H8l-4 3v-7a4 4 0 0 1-1-2.6V7a4 4 0 0 1 4-4h9a4 4 0 0 1 4 4Z"/></svg>"#
        }
    }
}

const PLAYGROUND_IMAGE_RATIOS: [(&str, u32, u32, u32); 4] = [
    ("1:1", 1, 1, 512),
    ("4:3", 4, 3, 160),
    ("3:4", 3, 4, 160),
    ("16:9", 16, 9, 48),
];

#[derive(Debug, Eq, PartialEq)]
struct PlaygroundImageRequestConfig {
    dimension_mode: &'static str,
    sizes: BTreeMap<&'static str, String>,
}

fn playground_image_request_config(model: &GatewayModel) -> Option<PlaygroundImageRequestConfig> {
    let contract = model
        .mayhem
        .adapter
        .endpoint_families
        .iter()
        .find(|contract| contract.family == mayhem_proto::ENDPOINT_OPENAI_IMAGE_GENERATIONS)?;
    let supports_size = contract
        .request_attributes
        .iter()
        .any(|path| path == "size");
    let supports_dimensions = ["width", "height"].iter().all(|required| {
        contract
            .request_attributes
            .iter()
            .any(|path| path == required)
    });
    let dimension_mode = if supports_size {
        "size"
    } else if supports_dimensions {
        "width-height"
    } else {
        return None;
    };
    let width_spec = contract.request_attribute_specs.get("width");
    let height_spec = contract.request_attribute_specs.get("height");
    let minimum_width = signed_dimension_minimum(width_spec);
    let minimum_height = signed_dimension_minimum(height_spec);
    let maximum_width = signed_dimension_maximum(width_spec, model.mayhem.caps.max_image_width);
    let maximum_height = signed_dimension_maximum(height_spec, model.mayhem.caps.max_image_height);
    let mut sizes = BTreeMap::new();

    for (label, ratio_width, ratio_height, preferred_scale) in PLAYGROUND_IMAGE_RATIOS {
        let minimum_scale = preferred_scale
            .max(minimum_width.div_ceil(ratio_width))
            .max(minimum_height.div_ceil(ratio_height));
        let maximum_scale = (maximum_width / ratio_width).min(maximum_height / ratio_height);
        for scale in minimum_scale..=maximum_scale {
            let width = ratio_width.saturating_mul(scale);
            let height = ratio_height.saturating_mul(scale);
            if !signed_dimension_allows(width_spec, width)
                || !signed_dimension_allows(height_spec, height)
            {
                continue;
            }
            let request = if dimension_mode == "size" {
                json!({
                    "model": model.id,
                    "prompt": "Playground image dimension compatibility check",
                    "size": format!("{width}x{height}"),
                })
            } else {
                json!({
                    "model": model.id,
                    "prompt": "Playground image dimension compatibility check",
                    "width": width,
                    "height": height,
                })
            };
            if mayhem_proto::validate_endpoint_request(contract, &request).is_ok() {
                sizes.insert(label, format!("{width}x{height}"));
                break;
            }
        }
    }

    Some(PlaygroundImageRequestConfig {
        dimension_mode,
        sizes,
    })
}

fn signed_dimension_minimum(spec: Option<&mayhem_proto::EndpointAttributeSpec>) -> u32 {
    spec.and_then(|spec| spec.minimum)
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| value.ceil().min(f64::from(u32::MAX)) as u32)
        .unwrap_or(1)
}

fn signed_dimension_maximum(
    spec: Option<&mayhem_proto::EndpointAttributeSpec>,
    caps_maximum: Option<u32>,
) -> u32 {
    let signed_maximum = spec
        .and_then(|spec| spec.maximum)
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| value.floor().min(f64::from(u32::MAX)) as u32)
        .unwrap_or(4_096);
    caps_maximum.map_or(signed_maximum, |value| value.min(signed_maximum))
}

fn signed_dimension_allows(spec: Option<&mayhem_proto::EndpointAttributeSpec>, value: u32) -> bool {
    let Some(spec) = spec else {
        return true;
    };
    let value = f64::from(value);
    if spec.minimum.is_some_and(|minimum| value < minimum)
        || spec.maximum.is_some_and(|maximum| value > maximum)
    {
        return false;
    }
    if spec.multiple_of.is_some_and(|multiple| {
        !multiple.is_finite()
            || multiple <= 0.0
            || (value / multiple - (value / multiple).round()).abs() > f64::EPSILON * 16.0
    }) {
        return false;
    }
    spec.enum_values.is_empty()
        || spec
            .enum_values
            .iter()
            .any(|candidate| candidate.as_u64() == Some(value as u64))
}

fn playground_page(data: &DashboardData, expires: u64, selected_model: Option<&str>) -> String {
    let credential_needed = data.requires_auth() && data.active_token_count() == 0;
    let mut choices = data
        .models
        .iter()
        .filter(|model| playground_model_mode(model).is_some())
        .map(|model| (model, model_availability(data, model)))
        .collect::<Vec<_>>();
    choices.sort_by_key(|(_, availability)| availability.tone != "good");

    let explicit_selection =
        selected_model.filter(|candidate| choices.iter().any(|(model, _)| model.id == *candidate));
    let default_index = explicit_selection
        .and_then(|candidate| choices.iter().position(|(model, _)| model.id == candidate))
        .unwrap_or(0);
    let default_model_id = choices
        .get(default_index)
        .map(|(model, _)| model.id.clone())
        .unwrap_or_default();
    let active_mode = choices
        .get(default_index)
        .and_then(|(model, _)| playground_model_mode(model))
        .unwrap_or("chat");

    let mut mode_counts = [0usize; 3];
    let mut options = String::new();
    let mut model_cards = String::new();
    let mut default_model_icon = String::new();
    let mut default_model_name = String::new();
    let mut default_model_meta = String::new();

    for (index, (model, availability)) in choices.iter().enumerate() {
        let Some(mode) = playground_model_mode(model) else {
            continue;
        };
        match mode {
            "image" => mode_counts[1] += 1,
            "speech" => mode_counts[2] += 1,
            _ => mode_counts[0] += 1,
        }
        let is_selected = index == default_index;
        let selected = if is_selected { " selected" } else { "" };
        let protection = if model.mayhem.attestation_tiers.is_empty() {
            "No catalog attestation tier listed".to_owned()
        } else {
            format!(
                "Catalog tiers {}",
                model
                    .mayhem
                    .attestation_tiers
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let price_mode = if model.mayhem.price_ref_au.rate_map.is_empty() {
            "fixed"
        } else {
            "rate"
        };
        let lab = dashboard_model_lab(model);
        let lab_icon = dashboard_model_lab_icon(&lab);
        let display_name = dashboard_model_name(model);
        let context = format_token_count(u64::from(model.mayhem.caps.ctx));
        let purpose = match mode {
            "image" => "Image generation",
            "speech" => "Natural text to speech",
            _ => "Chat and text generation",
        };
        let image_attributes =
            playground_image_request_config(model).map_or_else(String::new, |config| {
                let sizes =
                    serde_json::to_string(&config.sizes).unwrap_or_else(|_| "{}".to_owned());
                format!(
                    r#" data-image-dimension-mode="{}" data-image-sizes="{}""#,
                    config.dimension_mode,
                    html_escape(&sizes),
                )
            });
        options.push_str(&format!(
            r##"<option value="{}" data-playground-mode="{mode}" data-availability="{}" data-price="{}" data-price-mode="{price_mode}" data-location="Network provider route" data-protection="{}" data-context="Up to {context} catalog tokens" data-model-name="{}" data-model-lab="{}" data-model-purpose="{purpose}"{image_attributes}{selected}>{} &mdash; {}</option>"##,
            html_escape(&model.id),
            html_escape(availability.label),
            html_escape(&dashboard_model_price(model)),
            html_escape(&protection),
            html_escape(&display_name),
            html_escape(&lab.name),
            html_escape(&model.id),
            html_escape(availability.label),
        ));
        model_cards.push_str(&format!(
            r##"<button class="pg-model-option{}" id="playground-model-option-{index}" type="button" role="option" aria-selected="{is_selected}" tabindex="{}" data-playground-model-option="{}" data-playground-mode="{mode}"{}><span class="pg-logo-tile pg-model-option-logo">{lab_icon}</span><span class="pg-model-option-copy"><span class="pg-model-option-name">{}</span><span class="pg-model-option-purpose">{purpose}</span><span class="pg-model-option-meta"><span class="pg-model-option-provider">{}</span><span class="pg-model-option-context">{context} context</span><span>{}</span></span></span><span class="pg-model-option-check" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="m5 12 4 4L19 6"/></svg></span></button>"##,
            if is_selected { " is-selected" } else { "" },
            if is_selected { "0" } else { "-1" },
            html_escape(&model.id),
            if mode == active_mode { "" } else { " hidden" },
            html_escape(&display_name),
            html_escape(&lab.name),
            html_escape(availability.label),
        ));
        if is_selected {
            default_model_icon = lab_icon;
            default_model_name = html_escape(&display_name);
            default_model_meta = purpose.to_owned();
        }
    }

    let mode_tab = |mode: &str, label: &str, count: usize| {
        let selected = mode == active_mode;
        let unavailable = count == 0;
        format!(
            r##"<button type="button" role="tab" data-playground-mode-tab="{mode}" data-empty="{unavailable}" aria-selected="{selected}" aria-controls="playground-{mode}-panel" aria-label="{}" tabindex="{}" class="{}"{}>{}<span class="pg-mode-label">{label}</span>{}</button>"##,
            if unavailable {
                format!("{label}, no compatible model")
            } else {
                label.to_owned()
            },
            if selected { "0" } else { "-1" },
            if selected { "is-active" } else { "" },
            if unavailable { " disabled" } else { "" },
            playground_mode_icon(mode),
            if unavailable {
                r#"<span class="pg-mode-soon">Unavailable</span>"#
            } else {
                ""
            },
        )
    };
    let mode_tabs = format!(
        "{}{}{}",
        mode_tab("chat", "Text", mode_counts[0]),
        mode_tab("image", "Image", mode_counts[1]),
        mode_tab("speech", "Speech", mode_counts[2]),
    );
    let mode_label = match active_mode {
        "image" => "Image model",
        "speech" => "Speech model",
        _ => "Text model",
    };
    let active_model_count = match active_mode {
        "image" => mode_counts[1],
        "speech" => mode_counts[2],
        _ => mode_counts[0],
    };
    let default_model_value = html_escape(&default_model_id);

    let actions = if credential_needed {
        r##"<a class="primary-button" href="/mayhem/dashboard/connect">Set up access</a>"##
    } else {
        ""
    };
    let token_field = if data.requires_auth() {
        r##"<label class="pg-advanced-field span-all" for="playground-token"><span>Access token</span><input id="playground-token" data-playground-token type="password" autocomplete="off" spellcheck="false" required><small>Kept only in this page's memory.</small></label>"##
    } else {
        r##"<input data-playground-token type="hidden" value="">"##
    };

    let chat_hidden = if active_mode == "chat" { "" } else { " hidden" };
    let image_hidden = if active_mode == "image" {
        ""
    } else {
        " hidden"
    };
    let speech_hidden = if active_mode == "speech" {
        ""
    } else {
        " hidden"
    };
    let content = if credential_needed {
        page_empty_block(
            "Create an access token first",
            "This gateway requires authentication and currently has no active token, so a Playground request cannot succeed yet.",
            None,
        )
    } else if choices.is_empty() {
        page_empty_block(
            "No compatible models available",
            "Playground needs at least one text-generation, image-generation, or speech model in the gateway catalog.",
            Some(("Open Models", "/mayhem/dashboard/models")),
        )
    } else {
        format!(
            r##"<section class="playground-layout pg-page" aria-label="AI Playground" data-playground-mode="{active_mode}">
<noscript><div class="notice warn"><strong>The in-browser Playground needs JavaScript.</strong><p>Enable JavaScript and reload before entering a prompt or access token.</p></div></noscript>
<div class="playground-interactive js-only">
<form data-playground-form>
  <div class="pg-toolbar">
    <div class="pg-mode-tabs" role="tablist" aria-label="Playground mode"><span class="pg-mode-pill" aria-hidden="true"></span>{mode_tabs}</div>
    <div class="pg-model" data-playground-model-picker>
      <select id="playground-model" data-playground-model data-playground-draft data-default-value="{default_model_value}" hidden tabindex="-1" aria-hidden="true">{options}</select>
      <button class="pg-model-trigger" type="button" data-playground-model-trigger aria-haspopup="listbox" aria-expanded="false" aria-controls="playground-model-list" aria-label="Choose model, {default_model_name} selected"><span class="pg-logo-tile pg-model-trigger-logo" data-playground-model-trigger-icon>{default_model_icon}</span><span class="pg-model-trigger-copy"><span class="pg-model-trigger-name" data-playground-model-trigger-name>{default_model_name}</span><span class="pg-model-trigger-purpose" data-playground-model-trigger-meta>{default_model_meta}</span></span><svg class="pg-model-trigger-chevron" viewBox="0 0 24 24" aria-hidden="true"><path d="m7 10 5 5 5-5"/></svg></button>
      <button class="pg-model-backdrop" type="button" data-playground-model-close aria-label="Close model list" hidden></button>
      <div class="pg-model-panel" data-playground-model-panel hidden><span class="pg-model-panel-grip" aria-hidden="true"></span><header class="pg-model-panel-head"><div class="pg-model-panel-head-copy"><strong data-playground-model-panel-label>{mode_label}</strong><span><span data-playground-model-count>{active_model_count}</span> available</span></div><button type="button" data-playground-model-close aria-label="Close model list"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 6 12 12M18 6 6 18"/></svg></button></header><div class="pg-model-list" id="playground-model-list" role="listbox" aria-label="{mode_label}">{model_cards}</div></div>
    </div>
  </div>

  <div class="pg-meta-row">
    <p class="pg-preview-note" role="note"><span aria-hidden="true"></span>Live through this OpenMayhem gateway. Requests are processed by the selected provider.</p>
    <a class="pg-evidence-link" href="/mayhem/dashboard/activity">Receipts and evidence</a>
  </div>

  <div class="pg-experience">
    <div class="pg-surface">
      <div class="pg-mode-stack">
        <section id="playground-chat-panel" role="tabpanel" aria-label="Text playground" class="pg-mode-panel" data-playground-mode-panel="chat"{chat_hidden}>
          <section class="pg-chat" aria-label="Text playground">
            <div class="pg-chat-thread is-empty" data-playground-thread role="log" aria-live="polite" aria-relevant="additions" aria-label="Conversation">
              <div class="pg-chat-empty" data-playground-empty>
                <p class="pg-empty-model"><span class="pg-logo-tile pg-empty-model-logo" data-playground-active-model-icon>{default_model_icon}</span><span data-playground-active-model-name>{default_model_name}</span><em data-playground-active-model-context>Live route</em></p>
                <h2>How can I help?</h2><p>Choose a starting point or write your own message.</p>
                <div class="pg-starters"><button type="button" data-playground-starter data-playground-starter-prompt="Explain public-key cryptography with one concrete analogy and no jargon.">Explain a concept</button><button type="button" data-playground-starter data-playground-starter-prompt="Create a focused three-day plan for learning the basics of TypeScript.">Make a plan</button><button type="button" data-playground-starter data-playground-starter-prompt="Rewrite a short product announcement so it is clear, confident, and concise.">Improve writing</button></div>
              </div>
            </div>
            <div class="pg-composer-wrap">
              <div class="pg-composer"><label class="sr-only" for="playground-prompt">Message OpenMayhem</label><textarea id="playground-prompt" data-playground-prompt data-playground-draft maxlength="1600" rows="1" aria-describedby="playground-prompt-help" placeholder="Message OpenMayhem" required></textarea><span class="pg-composer-count" data-playground-chat-count hidden>0/1600</span><button class="pg-stop" type="button" data-playground-stop hidden><svg viewBox="0 0 24 24" aria-hidden="true"><rect x="6" y="6" width="12" height="12" rx="2"/></svg>Stop</button><button class="pg-send" type="submit" data-playground-send aria-label="Send message"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="m22 2-7 20-4-9-9-4Z"/><path d="M22 2 11 13"/></svg></button></div>
              <p id="playground-prompt-help">Live provider request. Do not enter sensitive information.</p>
            </div>
          </section>
        </section>

        <section id="playground-image-panel" role="tabpanel" aria-label="Image playground" class="pg-mode-panel" data-playground-mode-panel="image"{image_hidden}>
          <section class="pg-media" aria-label="Image generation playground">
            <div class="pg-settings"><div class="pg-section-heading"><h2>Generate an image</h2><p>Describe what you want to create.</p></div><label class="pg-field" for="playground-image-prompt"><span>Prompt <em class="pg-count" data-playground-image-count>0/1200</em></span><textarea id="playground-image-prompt" data-playground-image-prompt data-playground-draft maxlength="1200" rows="7" placeholder="A quiet observatory above a sea of clouds…"></textarea></label><fieldset class="pg-ratio-field"><legend><span>Aspect ratio</span><em class="pg-count" data-playground-image-size>Model dimensions</em></legend><div><button type="button" class="is-active" aria-pressed="true" data-playground-aspect-ratio="1:1"><span class="pg-ratio-glyph ratio-1-1" aria-hidden="true"></span>1:1</button><button type="button" aria-pressed="false" data-playground-aspect-ratio="4:3"><span class="pg-ratio-glyph ratio-4-3" aria-hidden="true"></span>4:3</button><button type="button" aria-pressed="false" data-playground-aspect-ratio="3:4"><span class="pg-ratio-glyph ratio-3-4" aria-hidden="true"></span>3:4</button><button type="button" aria-pressed="false" data-playground-aspect-ratio="16:9"><span class="pg-ratio-glyph ratio-16-9" aria-hidden="true"></span>16:9</button></div></fieldset><button type="button" class="pg-primary-action" data-playground-generate-image disabled>{image_icon}<span>Generate image</span></button></div>
            <div class="pg-output" data-playground-image-output aria-live="polite"><div class="pg-output-empty">{image_icon}<strong>Image output</strong><span>Your generated image will appear here.</span></div></div>
          </section>
        </section>

        <section id="playground-speech-panel" role="tabpanel" aria-label="Speech playground" class="pg-mode-panel" data-playground-mode-panel="speech"{speech_hidden}>
          <section class="pg-media" aria-label="Speech generation playground">
            <div class="pg-settings"><div class="pg-section-heading"><h2>Generate speech</h2><p>Enter text and choose a voice.</p></div><label class="pg-field" for="playground-speech-text"><span>Text <em class="pg-count" data-playground-speech-count>0/800</em></span><textarea id="playground-speech-text" data-playground-speech-text data-playground-draft maxlength="800" rows="8" placeholder="Write a short passage to hear it spoken…"></textarea></label><fieldset class="pg-voice-field"><legend>Voice</legend><div class="pg-voice-grid"><label class="pg-voice-option is-selected"><input class="sr-only" data-playground-voice type="radio" name="pg-voice" value="af_heart" checked><span class="pg-voice-copy"><strong>Heart</strong><em>Warm American English</em></span><span class="pg-voice-check" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="m5 12 4 4L19 6"/></svg></span></label><label class="pg-voice-option"><input class="sr-only" data-playground-voice type="radio" name="pg-voice" value="af_bella"><span class="pg-voice-copy"><strong>Bella</strong><em>Clear American English</em></span><span class="pg-voice-check" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="m5 12 4 4L19 6"/></svg></span></label><label class="pg-voice-option"><input class="sr-only" data-playground-voice type="radio" name="pg-voice" value="bf_emma"><span class="pg-voice-copy"><strong>Emma</strong><em>Natural British English</em></span><span class="pg-voice-check" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="m5 12 4 4L19 6"/></svg></span></label><label class="pg-voice-option"><input class="sr-only" data-playground-voice type="radio" name="pg-voice" value="bm_george"><span class="pg-voice-copy"><strong>George</strong><em>Measured British English</em></span><span class="pg-voice-check" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="m5 12 4 4L19 6"/></svg></span></label></div></fieldset><button type="button" class="pg-primary-action" data-playground-generate-speech disabled>{speech_icon}<span>Generate speech</span></button></div>
            <div class="pg-output" data-playground-speech-output aria-live="polite"><div class="pg-output-empty">{speech_icon}<strong>Audio output</strong><span>Generated speech will appear here.</span></div></div>
          </section>
        </section>
      </div>
    </div>

    <section class="pg-network" data-playground-network>
      <button type="button" class="pg-network-summary" data-playground-network-toggle aria-expanded="false" aria-controls="playground-network-body"><span class="pg-network-grip" aria-hidden="true"></span><span class="pg-network-dot" aria-hidden="true"></span><span class="pg-network-labels"><span class="pg-network-title">Network activity</span><span class="pg-network-state" data-playground-network-state>Ready</span></span><span class="pg-network-model"><span class="pg-logo-tile pg-network-model-logo" data-playground-network-icon>{default_model_icon}</span><span data-playground-network-model>{default_model_name}</span></span><svg class="pg-network-chevron" viewBox="0 0 24 24" aria-hidden="true"><path d="m7 10 5 5 5-5"/></svg></button>
      <div class="pg-network-body" id="playground-network-body" data-playground-network-body hidden><ol class="pg-network-steps" aria-label="Request progression"><li data-playground-step="request"><span class="pg-step-marker" aria-hidden="true"></span><span class="pg-step-copy"><span class="pg-step-label">Request</span><span class="pg-step-detail">Waiting for input</span></span></li><li data-playground-step="provider"><span class="pg-step-marker" aria-hidden="true"></span><span class="pg-step-copy"><span class="pg-step-label">Provider route</span><span class="pg-step-detail">Not started</span></span></li><li data-playground-step="generate"><span class="pg-step-marker" aria-hidden="true"></span><span class="pg-step-copy"><span class="pg-step-label">Generate</span><span class="pg-step-detail">Not started</span></span></li><li data-playground-step="receipt"><span class="pg-step-marker" aria-hidden="true"></span><span class="pg-step-copy"><span class="pg-step-label">Signed receipt</span><span class="pg-step-detail">Pending generation</span></span></li></ol><dl class="pg-network-facts"><div><dt>Model</dt><dd data-playground-fact="model">{default_model_name}</dd></div><div><dt>Provider</dt><dd data-playground-fact="provider">Revealed after receipt</dd></div><div><dt>Timing</dt><dd data-playground-fact="timing">Pending</dd></div><div><dt>Cost</dt><dd data-playground-fact="cost">Pending</dd></div><div><dt>Request ID</dt><dd data-playground-fact="request">Not started</dd></div></dl><p class="pg-network-footnote">Live values appear only as the gateway confirms them. Provider identity is revealed after a signed receipt is returned.</p></div>
    </section>
  </div>

  <details class="pg-advanced" data-playground-request-settings><summary><span>Advanced request controls</span><span data-playground-request-summary>Gateway price and trust defaults</span></summary><div class="pg-advanced-grid">{token_field}<label class="pg-advanced-field span-all" for="playground-system"><span>System instructions <em>Text only</em></span><textarea id="playground-system" data-playground-system data-playground-draft rows="3"></textarea></label><label class="pg-advanced-field" for="playground-max-tokens"><span>Output limit <em>Text only</em></span><input id="playground-max-tokens" data-playground-max-tokens data-playground-draft type="number" inputmode="numeric" min="64" max="4096" step="64" value="512"></label><label class="pg-advanced-field" for="playground-max-price"><span><span data-playground-price-label>Route price ceiling</span><em data-playground-price-unit>USD</em></span><input id="playground-max-price" data-playground-max-price data-playground-draft data-money-input type="text" inputmode="decimal" autocomplete="off" spellcheck="false" pattern="[0-9]+([.][0-9]{{1,18}})?" placeholder="Optional USD ceiling"></label><label class="pg-advanced-field" for="playground-min-att-tier"><span>Minimum attestation tier</span><select id="playground-min-att-tier" data-playground-min-att-tier data-playground-draft><option value="">Gateway default</option><option value="1">At least T1 numerically</option><option value="2">At least T2 numerically</option><option value="3">At least T3 numerically</option><option value="4">T4 only</option></select><small>Numeric identity tier does not promise confidential compute.</small></label><div class="preflight span-all" data-playground-preflight><span><strong>Capacity:</strong> <span data-preflight-value="availability">Checking</span></span><span><strong>Protection:</strong> <span data-preflight-value="protection">Checking catalog</span></span><span><strong>Context:</strong> <span data-preflight-value="context">Checking catalog</span></span><span><strong>Catalog rates:</strong> <span class="money-value" data-money data-preflight-value="price">Unavailable</span></span></div><button class="pg-text-action span-all" type="button" data-playground-reset-draft>Reset saved draft and settings</button></div></details>
  <p class="pg-local-note" data-playground-meta>No request sent · drafts and history stay in this browser tab · access tokens are never saved</p>
</form>
</div>
</section>"##,
            image_icon = playground_mode_icon("image"),
            speech_icon = playground_mode_icon("speech"),
        )
    };

    shell(
        data,
        expires,
        DashboardAppPage::Playground,
        "Use AI",
        "Playground",
        if credential_needed {
            "Create a gateway token under Integrations, then return here to send a real request."
        } else {
            "Choose Text, Image, or Speech, then create through a live provider route."
        },
        if credential_needed {
            "Credential needed"
        } else if data.accepting_models() > 0 {
            "Capacity advertised"
        } else {
            "Routes unavailable"
        },
        if credential_needed {
            "warn"
        } else if data.accepting_models() > 0 {
            "good"
        } else {
            "warn"
        },
        actions,
        &content,
    )
}

fn models_page(data: &DashboardData, expires: u64, requested_page: Option<&str>) -> String {
    let page = PageWindow::from_query(data.models.len(), MAX_MODEL_ROWS, requested_page);
    let rows = model_rows(data, page);
    let model_summary = page.status("catalog models");
    let pagination = pagination_nav(page, "/mayhem/dashboard/models", &[], "catalog models");
    let (filter_controls, filter_empty) = shown_rows_filter(
        "models",
        "models-table",
        "Filter the shown models by name or capability",
        page.len(),
        "models",
    );
    let content = format!(
        r##"<section class="panel model-catalog-panel"><header class="panel-head"><div class="panel-title"><h2>Model catalog</h2><p>Compare what each model does, whether it is available, and its starting catalog rates.</p></div>{filter_controls}</header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table model-catalog-table" id="models-table"><caption class="sr-only">Models in this gateway catalog</caption><colgroup><col class="model-col"><col class="availability-col"><col class="capabilities-col"><col class="price-col"><col class="action-col"></colgroup><thead><tr><th>Model</th><th>Availability</th><th>Capabilities</th><th>Starting price</th><th><span class="sr-only">Actions</span></th></tr></thead><tbody>{rows}</tbody></table></div>{filter_empty}</div><footer class="panel-footer"><span>{model_summary}</span>{pagination}<a href="/mayhem/dashboard/network/models">Open network models</a></footer></section>"##,
    );
    shell_wide(
        data,
        expires,
        DashboardAppPage::Models,
        "Use AI",
        "Model catalog",
        "Compare models by capability, context, protection, and price. Final price and eligibility are confirmed when you send.",
        if data.accepting_models() > 0 { "Capacity advertised" } else { "No advertised capacity" },
        if data.accepting_models() > 0 { "good" } else { "warn" },
        if data.models.is_empty() {
            ""
        } else if data.requires_auth() && data.active_token_count() == 0 {
            r##"<a class="primary-button" href="/mayhem/dashboard/connect">Set up access</a>"##
        } else if data.accepting_models() == 0 {
            ""
        } else {
            r##"<a class="primary-button" href="/mayhem/dashboard/playground">Open Playground</a>"##
        },
        &content,
    )
}

fn model_rows(data: &DashboardData, page: PageWindow) -> String {
    if data.models.is_empty() {
        return format!(
            r##"<tr><td colspan="5">{}</td></tr>"##,
            empty_block(
                "No catalog models",
                "Wait for the catalog to load or inspect gateway status.",
                None
            )
        );
    }
    data.models
        .iter()
        .skip(page.start)
        .take(page.len())
        .map(|model| {
            let availability = model_availability(data, model);
            let lab = dashboard_model_lab(model);
            let lab_icon = dashboard_model_lab_icon(&lab);
            let ability_values = dashboard_model_abilities(model)
                .into_iter()
                .filter(|value| !value.starts_with("api:"))
                .collect::<Vec<_>>();
            let ability_filter = ability_values.join(" ");
            let ability_export = ability_values.join(" / ");
            let abilities = model_catalog_capabilities_html(&ability_values);
            let catalog_price = model_catalog_price_html(model);
            let evidence_url = evidence_href("model", &[("id", model.id.as_str())]);
            let evidence = evidence_link(
                &evidence_url,
                "Verify",
                &model.id,
            );
            let availability_explanation = model_freshness_window(data, model).map_or_else(
                || html_escape(&availability.explanation),
                |window| {
                    volatile_text(&availability.explanation, window, "Refresh to reconfirm")
                },
            );
            let display_name = dashboard_model_name(model);
            let (model_type, purpose) = match model.mayhem.model_class.as_str() {
                "image-generation" => ("Image generation", "Create images from written prompts."),
                "tts" => ("Text to speech", "Turn written text into generated speech."),
                "embedding" => ("Embeddings", "Create vector representations for search and retrieval."),
                "reranking" => ("Reranking", "Reorder candidate results by relevance."),
                _ => ("Text generation", "Chat, writing, reasoning, and structured model responses."),
            };
            let detail_template = format!(
                r##"<template data-model-detail-template><article class="model-detail-content"><header class="model-detail-hero"><span class="model-detail-logo">{}</span><div class="model-detail-identity"><span class="model-detail-lab">{}</span><h3>{}</h3><code>{}</code></div><span class="status-badge {}"><span class="status-dot" aria-hidden="true"></span>{}</span></header><p class="model-detail-purpose">{purpose}</p><dl class="model-detail-facts"><div><dt>Model type</dt><dd>{model_type}</dd></div><div><dt>Context window</dt><dd>{} tokens</dd></div><div><dt>Current availability</dt><dd>{}</dd><small>{}</small></div></dl><section class="model-detail-section"><div class="model-detail-section-head"><div><span>What it supports</span><h4>Capabilities</h4></div><span>{} total</span></div><div class="model-detail-capabilities">{}</div></section><section class="model-detail-section"><div class="model-detail-section-head"><div><span>Reference terms</span><h4>Catalog pricing</h4></div></div><div class="model-detail-price" data-money>{}</div><p>Final price and route eligibility are confirmed when you send.</p></section><footer class="model-detail-actions"><a class="primary-button" href="/mayhem/dashboard/playground?model={}">Use in Playground</a><a class="quiet-button" href="{}">Verify evidence</a></footer></article></template>"##,
                lab_icon,
                html_escape(&lab.name),
                html_escape(&display_name),
                html_escape(&model.id),
                availability.tone,
                html_escape(availability.label),
                format_token_count(u64::from(model.mayhem.caps.ctx)),
                html_escape(availability.label),
                availability_explanation,
                ability_values.len(),
                model_detail_capabilities_html(&ability_values),
                model_catalog_price_html_with_limit(model, None),
                dashboard_url_encode(&model.id),
                html_escape(&evidence_url),
            );
            format!(
                r##"<tr data-filter-row data-filter-text="{} {} {} {} {}"><th scope="row" data-export-value="{}"><button class="catalog-model catalog-model-button" type="button" data-model-detail-open aria-haspopup="dialog" aria-controls="model-detail-dialog" aria-label="View details for {}"><span class="catalog-model-logo">{}</span><span class="catalog-model-copy"><span class="catalog-model-lab">{}</span><span class="catalog-model-name">{}</span><span class="catalog-model-id mono">{}</span></span><svg class="catalog-model-chevron" viewBox="0 0 24 24" aria-hidden="true"><path d="m9 6 6 6-6 6"/></svg></button>{}</th><td><span class="status-badge {}"><span class="status-dot" aria-hidden="true"></span>{}</span><span class="table-secondary catalog-availability-detail">{}</span></td><td data-export-value="{}" data-sort-value="{}"><div class="catalog-capabilities">{}</div></td><td data-money data-export-value="{}"><div class="catalog-price">{}</div></td><td class="table-action"><div class="inline-actions catalog-actions"><a class="quiet-button" href="/mayhem/dashboard/playground?model={}" aria-label="Use {} in Playground">Use</a>{}</div></td></tr>"##,
                html_escape(&model.id),
                html_escape(&model.mayhem.family),
                html_escape(&lab.name),
                html_escape(availability.label),
                html_escape(&ability_filter),
                html_escape(&model.id),
                html_escape(&model.id),
                lab_icon,
                html_escape(&lab.name),
                html_escape(&display_name),
                html_escape(&model.id),
                detail_template,
                availability.tone,
                html_escape(availability.label),
                availability_explanation,
                html_escape(&ability_export),
                html_escape(&ability_export),
                abilities,
                html_escape(&dashboard_model_price(model)),
                catalog_price,
                dashboard_url_encode(&model.id),
                html_escape(&model.id),
                evidence,
            )
        })
        .collect()
}

fn model_catalog_capability_label(value: &str) -> String {
    if let Some(context) = value.strip_prefix("ctx ") {
        return format!("{context} context");
    }
    if let Some((name, levels)) = value.split_once(':') {
        let name = humanize_model_label(name);
        let options = levels
            .split('|')
            .map(|level| {
                let level = if name.to_ascii_lowercase().ends_with("budget") {
                    level.strip_prefix("budget ").unwrap_or(level)
                } else {
                    level
                };
                humanize_model_label(level)
            })
            .collect::<Vec<_>>()
            .join(" / ");
        return format!("{name}: {options}");
    }
    match value {
        "tools" => "Tool use".to_owned(),
        "json" => "Structured JSON".to_owned(),
        "tts" => "Text to speech".to_owned(),
        _ => humanize_model_label(&value.replace('_', " ")),
    }
}

fn model_catalog_capabilities_html(values: &[String]) -> String {
    let mut ordered = values.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|value| if value.starts_with("ctx ") { 0 } else { 1 });
    let shown_count = ordered.len().min(4);
    let omitted = ordered.len().saturating_sub(shown_count);
    let mut html = ordered
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let class = if value.starts_with("ctx ") {
                "catalog-capability is-context"
            } else {
                "catalog-capability"
            };
            format!(
                r#"<span class="{class}{}"{}>{}</span>"#,
                if index >= shown_count {
                    " catalog-capability-extra"
                } else {
                    ""
                },
                if index >= shown_count { " hidden" } else { "" },
                html_escape(&model_catalog_capability_label(value))
            )
        })
        .collect::<String>();
    if omitted > 0 {
        html.push_str(&format!(
            r#"<button class="catalog-capability-more" type="button" data-catalog-capabilities-toggle data-collapsed-label="+{omitted} more" aria-expanded="false"><span data-catalog-capabilities-label>+{omitted} more</span></button>"#,
        ));
    }
    html
}

fn model_detail_capabilities_html(values: &[String]) -> String {
    let mut ordered = values.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|value| if value.starts_with("ctx ") { 0 } else { 1 });
    ordered
        .iter()
        .map(|value| {
            format!(
                r#"<span class="model-detail-capability{}">{}</span>"#,
                if value.starts_with("ctx ") {
                    " is-context"
                } else {
                    ""
                },
                html_escape(&model_catalog_capability_label(value)),
            )
        })
        .collect()
}

fn compact_catalog_money(value: u128) -> String {
    let exact = format_au_usd(value);
    if exact.len() <= 10 {
        return exact;
    }
    let Some(number) = exact
        .strip_prefix('$')
        .and_then(|value| value.parse::<f64>().ok())
    else {
        return exact;
    };
    if number > 0.0 && number < 0.000_001 {
        return "<$0.000001".to_owned();
    }
    let rounded = format!("{number:.6}");
    format!("${}", rounded.trim_end_matches('0').trim_end_matches('.'))
}

fn model_catalog_rate_unit(unit: &str, granularity: u64) -> String {
    let token_label = match unit {
        "input_token" => Some("input tokens"),
        "cached_input_token" => Some("cached input tokens"),
        "output_token" => Some("output tokens"),
        _ => None,
    };
    if let Some(label) = token_label {
        let granularity = u128::from(granularity);
        if granularity > 0 && 1_000_000_u128 % granularity == 0 {
            return format!("per 1M {label}");
        }
    }
    let mut label = unit.replace('_', " ");
    if granularity == 1 {
        format!("per {label}")
    } else {
        if !label.ends_with('s') {
            label.push('s');
        }
        format!("per {granularity} {label}")
    }
}

fn model_catalog_price_html(model: &GatewayModel) -> String {
    model_catalog_price_html_with_limit(model, Some(3))
}

fn model_catalog_price_html_with_limit(model: &GatewayModel, limit: Option<usize>) -> String {
    let price = &model.mayhem.price_ref_au;
    let mut entries = Vec::new();
    if price.per_req_au > 0 {
        entries.push((
            compact_catalog_money(price.per_req_au),
            "per request".to_owned(),
        ));
    }
    if price.min_session_au > 0 {
        entries.push((
            compact_catalog_money(price.min_session_au),
            "minimum per session".to_owned(),
        ));
    }
    entries.extend(price.rate_map.iter().map(|rate| {
        (
            compact_catalog_money(rate.per_unit_au),
            model_catalog_rate_unit(&rate.unit, rate.granularity),
        )
    }));
    if entries.is_empty() {
        return r#"<span class="catalog-price-unavailable">Not priced</span>"#.to_owned();
    }
    let shown_count = limit.unwrap_or(entries.len()).min(entries.len());
    let omitted = entries.len().saturating_sub(shown_count);
    let mut html = entries
        .iter()
        .take(shown_count)
        .enumerate()
        .map(|(index, (amount, unit))| {
            format!(
                r#"<span class="catalog-price-line{}"><span class="money-value catalog-price-amount">{}</span><span class="catalog-price-unit">{}</span></span>"#,
                if index == 0 { " is-primary" } else { "" },
                html_escape(amount),
                html_escape(unit),
            )
        })
        .collect::<String>();
    if omitted > 0 {
        html.push_str(&format!(
            r#"<span class="catalog-price-more">+{omitted} other rates</span>"#
        ));
    }
    html
}

fn activity_page(data: &DashboardData, expires: u64, requested_page: Option<&str>) -> String {
    let records = prioritized_activity_records(data);
    let activity_count = records.len();
    let incomplete_count = data.incomplete_session_count();
    let page = PageWindow::from_query(activity_count, MAX_ACTIVITY_ROWS, requested_page);
    let activity_summary = page.status("recorded sessions");
    let pagination = pagination_nav(page, "/mayhem/dashboard/activity", &[], "recorded sessions");
    let rows = if activity_count == 0 {
        format!(
            r##"<tr><td colspan="6">{}</td></tr>"##,
            empty_block(
                "No activity yet",
                if data.history_persistent {
                    "Requests recorded by this gateway will remain available after restart."
                } else {
                    "Requests recorded by this gateway run will appear here."
                },
                None
            )
        )
    } else {
        records
            .iter()
            .skip(page.start)
            .take(page.len())
            .enumerate()
            .map(|(index, record)| match record {
                ActivityRecord::Receipt(receipt) => activity_table_row(receipt, index),
                ActivityRecord::Paused(paused) => paused_activity_table_row(paused, index),
            })
            .collect::<String>()
    };
    let (filter_controls, filter_empty) = shown_rows_filter(
        "activity",
        "activity-table",
        "Filter the shown rows by model, provider, token, or status",
        page.len(),
        "activity",
    );
    let incomplete_attention = if incomplete_count == 0 {
        String::new()
    } else {
        attention(
            "warn",
            "!",
            "Open records to review",
            &format!(
                "{} still waiting on a final receipt. Open records are listed first; an open record does not mean work is still running.",
                count_noun(incomplete_count as u64, "session")
            ),
            Some(("Review open records", "#incomplete-activity")),
        )
    };
    let content = format!(
        r##"{incomplete_attention}<section class="metric-grid" aria-label="Activity summary">{}{}</section><section class="panel" id="incomplete-activity"><header class="panel-head"><div class="panel-title"><h2>Session activity</h2><p>Open records first, then the latest final receipt per session.</p></div>{filter_controls}</header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table" id="activity-table"><caption class="sr-only">Prioritized incomplete records, final receipts, and retained pause records from this gateway process</caption><thead><tr><th>Session</th><th>Model</th><th>Usage</th><th>Charge</th><th>Status</th><th>Evidence</th></tr></thead><tbody>{rows}</tbody></table></div>{filter_empty}</div><footer class="panel-footer"><span>{activity_summary}</span>{pagination}</footer></section>"##,
        metric(
            "Final receipts",
            &data.completed_requests().to_string(),
            "Requests with a signed final receipt",
            data.history_scope()
        ),
        metric(
            "Open records",
            &incomplete_count.to_string(),
            "Sessions without a final receipt yet",
            "Records"
        ),
    );
    let action = if data.requires_auth() && data.active_token_count() == 0 {
        r##"<a class="primary-button" href="/mayhem/dashboard/connect">Set up access</a>"##
    } else if data.models.is_empty() {
        r##"<a class="primary-button" href="/mayhem/dashboard/models">Inspect catalog</a>"##
    } else if data.accepting_models() == 0 {
        r##"<a class="primary-button" href="/mayhem/dashboard/models">Inspect availability</a>"##
    } else {
        r##"<a class="primary-button" href="/mayhem/dashboard/playground">New request</a>"##
    };
    shell_wide(
        data,
        expires,
        DashboardAppPage::Activity,
        "Use AI",
        "Requests and receipts",
        "Every request ends in a signed receipt. Open records are listed first — an open record does not mean work is still running.",
        if incomplete_count > 0 { "Open records to review" } else { "Caught up" },
        if incomplete_count > 0 { "warn" } else { "good" },
        action,
        &content,
    )
}

fn activity_table_row(receipt: &StoredReceipt, _index: usize) -> String {
    let body = &receipt.receipt.body;
    let evidence = evidence_link(
        &evidence_href("receipt", &[("id", body.session_id.as_str())]),
        "Verify",
        &body.session_id,
    );
    let status = if body.final_receipt {
        "Final receipt"
    } else {
        "Non-final receipt"
    };
    let tone = if body.final_receipt { "good" } else { "info" };
    let status_detail = String::new();
    let metered_note = if body.final_receipt {
        "metered total"
    } else {
        "metered so far"
    };
    let access = receipt
        .access_token
        .as_ref()
        .map(|token| token.name.as_str())
        .unwrap_or("Direct gateway use");
    format!(
        r##"<tr data-filter-row data-filter-text="{} {} {} {} {}"><td data-export-value="{}"><span class="table-primary mono">{}</span><span class="table-secondary">{} ago &middot; {}</span></td><td data-export-value="{} / provider {}"><span class="table-primary">{}</span><span class="table-secondary">provider {}</span></td><td><span class="table-primary mono">{} in / {} out</span><span class="table-secondary">receipt #{}</span></td><td data-money><span class="table-primary mono money-value">{}</span><span class="table-secondary">{metered_note}</span></td><td><span class="status-badge {tone}">{status}</span>{status_detail}</td><td>{}</td></tr>"##,
        html_escape(&body.session_id),
        html_escape(&body.model_id),
        html_escape(&body.provider),
        html_escape(access),
        html_escape(status),
        html_escape(&body.session_id),
        html_escape(short_text(&body.session_id, 14).as_ref()),
        html_escape(&format_elapsed_since(timestamp_seconds(body.ts))),
        html_escape(access),
        html_escape(&body.model_id),
        html_escape(&body.provider),
        html_escape(short_text(&body.model_id, 32).as_ref()),
        html_escape(short_text(&body.provider, 12).as_ref()),
        body.usage.prompt_tokens(),
        body.usage.output_tokens(),
        body.seq,
        html_escape(&format_au_usd(body.au_owed_cum)),
        evidence,
    )
}

fn paused_activity_table_row(paused: &PausedSession, _index: usize) -> String {
    let evidence = evidence_link(
        &evidence_href("paused", &[("id", paused.session_id.as_str())]),
        "Verify",
        &paused.session_id,
    );
    format!(
        r##"<tr data-filter-row data-filter-text="{} {} paused"><td data-export-value="{}"><span class="table-primary mono">{}</span><span class="table-secondary">Retained pause record</span></td><td>Unavailable</td><td>Unavailable</td><td>Unavailable</td><td><span class="status-badge warn">Paused</span><span class="table-secondary">{}</span></td><td>{}</td></tr>"##,
        html_escape(&paused.session_id),
        html_escape(&paused.reason),
        html_escape(&paused.session_id),
        html_escape(short_text(&paused.session_id, 24).as_ref()),
        html_escape(&paused.reason),
        evidence,
    )
}

struct WalletFundingGuide {
    label: &'static str,
    command: &'static str,
    help: &'static str,
    label_amount: Option<&'static str>,
    command_amount: Option<&'static str>,
}

fn wallet_funding_guide(rail: &str) -> WalletFundingGuide {
    match rail {
        "fiat" => WalletFundingGuide {
            label: "Start a $10 Stripe checkout",
            command: "mayhem pay stripe --amount 10",
            help: "Edit the amount if needed. The hosted checkout still requires your review before payment.",
            label_amount: Some("$10"),
            command_amount: Some("10"),
        },
        "tap" => WalletFundingGuide {
            label: "Prepare a 10 TAP deposit",
            command: "mayhem pay tap --amount-tap 10",
            help: "This is a dry run. Review the transaction plan, then add --confirm only when you are ready to broadcast.",
            label_amount: Some("10"),
            command_amount: Some("10"),
        },
        "tnk" => WalletFundingGuide {
            label: "Prepare a $10 TNK deposit intent",
            command: "mayhem pay tnk --amount 10",
            help: "This prepares an intent without submitting it. Review the output before adding submission flags.",
            label_amount: Some("$10"),
            command_amount: Some("10"),
        },
        _ => WalletFundingGuide {
            label: "Inspect available payment rails",
            command: "mayhem payments",
            help: "This gateway reports a rail without a dashboard funding recipe. Inspect the CLI output before choosing an action.",
            label_amount: None,
            command_amount: None,
        },
    }
}

fn wallet_deposit_status_command(rail: &str) -> String {
    match rail {
        "fiat" | "tap" | "tnk" => format!("mayhem deposit status --rail {rail}"),
        _ => "mayhem deposit status --help".to_owned(),
    }
}

fn wallet_page(data: &DashboardData, expires: u64) -> String {
    let rail = data.rail.to_ascii_uppercase();
    let funding = wallet_funding_guide(&data.rail);
    let funding_label = privacy_amount_text(funding.label, funding.label_amount);
    let funding_command = privacy_amount_text(funding.command, funding.command_amount);
    let deposit_status_command = wallet_deposit_status_command(&data.rail);
    let observed = payment_freshness_markup(data);
    let payment_ok = data
        .payment_directory
        .as_ref()
        .and_then(|value| value.get("ok"))
        .and_then(Value::as_bool);
    let balance_ready = data.payment_directory.is_some() && data.balance_au > 0;
    let funding_needed = data.payment_directory.is_some() && data.balance_au == 0;
    let (ledger_status, ledger_tone) = match (payment_ok, data.payment_snapshot_is_fresh()) {
        (Some(true), Some(true)) => ("Ledger snapshot current", "good"),
        (Some(false), _) => ("Payment rates stale", "warn"),
        (Some(true), Some(false)) => ("Ledger snapshot out of date", "warn"),
        (Some(true), None) => ("Ledger freshness unavailable", "warn"),
        (None, _) => ("Payment directory unavailable", "warn"),
    };
    let (status, tone) = if funding_needed {
        ("Funding needed", "warn")
    } else if balance_ready {
        ("Ready to use", "good")
    } else {
        (ledger_status, ledger_tone)
    };
    let ledger_status_badge = if ledger_status == "Ledger snapshot current" {
        payment_freshness_window(data).map_or_else(
            || status_badge("Current", "good"),
            |window| volatile_status_badge("Current", "good", window, "Refresh to reconfirm"),
        )
    } else {
        let label = match ledger_status {
            "Ledger snapshot out of date" => "Refresh to reconfirm",
            "Payment rates stale" => "Rates stale",
            "Ledger freshness unavailable" => "Freshness unknown",
            _ => "Directory unavailable",
        };
        status_badge(label, "warn")
    };
    let ledger_status_meta = match ledger_status {
        "Ledger snapshot current" => "The balance above matches the latest confirmed snapshot.",
        "Ledger snapshot out of date" => {
            "The balance above is the last confirmed snapshot. Refresh before sending."
        }
        _ => "The balance above cannot be confirmed right now.",
    };
    let next_step = if funding_needed {
        attention(
            "warn",
            "1",
            "Fund, confirm, then send",
            "Copy the funding command below, complete it in the CLI, then refresh this page until the ledger balance updates. The dashboard never starts a transaction without your review.",
            Some(("View funding command", "#wallet-funding-command")),
        )
    } else if balance_ready {
        attention(
            "good",
            "✓",
            "Balance observed",
            "The last ledger snapshot shows funds. Refresh this page if its freshness state expires before you send; the final signed receipt records the metered charge.",
            Some(("Open Playground", "/mayhem/dashboard/playground")),
        )
    } else {
        attention(
            "warn",
            "!",
            "Confirm the payment directory first",
            "Funding guidance is shown, but this gateway cannot currently confirm the canonical balance or payment freshness.",
            None,
        )
    };
    let content = format!(
        r##"{next_step}<section class="metric-grid">{}{}</section><section class="dashboard-layout"><div class="stack"><section class="panel"><header class="panel-head"><div class="panel-title"><h2>Fund your requests</h2><p>Three steps: fund in the CLI, confirm the ledger, then return to Playground.</p></div></header><div class="panel-body"><div class="field"><span class="field-label">1. {}</span><pre class="code-block"><code id="wallet-funding-command">{}</code><button class="quiet-button copy-corner js-only" type="button" data-copy data-copy-target="#wallet-funding-command" data-product-event="billing_funding_command_copied" aria-label="Copy funding command"><span data-copy-label>Copy</span></button></pre><p class="result-summary">{} This page does not start a transaction.</p></div><div class="field-gap" aria-hidden="true"></div><div class="field"><span class="field-label">2. Check a pending deposit</span><pre class="code-block"><code id="wallet-deposit-status-command">{}</code><button class="quiet-button copy-corner js-only" type="button" data-copy data-copy-target="#wallet-deposit-status-command" data-product-event="billing_deposit_check_copied" aria-label="Copy deposit status command"><span data-copy-label>Copy</span></button></pre><p class="result-summary">Reads the configured rail's canonical ledger state without changing it. Refresh this page after confirmation.</p></div><div class="field-gap" aria-hidden="true"></div><div class="field"><span class="field-label">3. Send a request</span><p class="result-summary">Once the balance appears above, open Playground. The receipt—not an estimate—shows the metered result.</p><a class="soft-button" href="/mayhem/dashboard/playground" data-product-event="billing_to_playground">Open Playground</a></div></div></section></div><aside class="stack"><section class="panel"><header class="panel-head"><div class="panel-title"><h2>Recovery readiness</h2><p>Secret material is never rendered in this dashboard.</p></div></header><div class="panel-body"><p class="notice warn"><strong>Backup status is not exposed to the gateway.</strong> Check it on the gateway host before relying on this wallet for provider payouts.</p><pre class="code-block"><code id="wallet-backup-command">mayhem wallet backup</code><button class="quiet-button copy-corner js-only" type="button" data-copy data-copy-target="#wallet-backup-command" aria-label="Copy wallet backup command"><span data-copy-label>Copy</span></button></pre><p class="result-summary">The CLI requires explicit confirmation before revealing a mnemonic. Anyone who sees it can restore the wallet.</p></div></section></aside></section>"##,
        if data.payment_directory.is_some() {
            metric_with_meta_html(
                "Ledger balance",
                &money_html(&format_au_usd(data.balance_au)),
                &observed,
                &rail,
            )
        } else {
            metric_status(
                "Ledger balance",
                &status_badge("Unavailable", "warn"),
                "The payment directory has not answered yet.",
                &rail,
            )
        },
        metric_status(
            "Ledger status",
            &ledger_status_badge,
            ledger_status_meta,
            "Ledger",
        ),
        funding_label,
        funding_command,
        funding.help,
        html_escape(&deposit_status_command),
    );
    shell(
        data,
        expires,
        DashboardAppPage::Wallet,
        "Billing",
        "Billing",
        "Fund requests, verify the ledger, and keep secret material outside the browser.",
        status,
        tone,
        "",
        &content,
    )
}

fn connect_page(
    data: &DashboardData,
    expires: u64,
    origin: &str,
    requested_page: Option<&str>,
) -> String {
    let root = origin.trim_end_matches('/');
    let base_url = format!("{root}/v1");
    let token_count = data.active_token_count();
    let credential_ready = !data.requires_auth() || token_count > 0;
    let accepting_models = data.accepting_models();
    let env_block = if data.requires_auth() {
        format!("OPENAI_BASE_URL={base_url}\nOPENAI_API_KEY=<your active Mayhem token>")
    } else {
        format!("OPENAI_BASE_URL={base_url}\nOPENAI_API_KEY=not-required")
    };
    let token_total = data
        .access
        .get("tokens")
        .and_then(Value::as_array)
        .map(Vec::len)
        .or_else(|| {
            data.access
                .get("token_count")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
        })
        .unwrap_or(0);
    let token_page = PageWindow::from_query(token_total, MAX_TOKEN_ROWS, requested_page);
    let token_summary = token_page.status("access tokens");
    let token_pagination = pagination_nav(
        token_page,
        "/mayhem/dashboard/connect",
        &[],
        "access tokens",
    );
    let (token_filter_controls, token_filter_empty) = shown_rows_filter(
        "access-tokens",
        "access-tokens-table",
        "Filter access tokens on the shown page by name, identifier, scope, or status",
        token_page.len(),
        "access tokens",
    );
    let token_store_notice = if data.access.get("store_status").and_then(Value::as_str)
        == Some("error")
    {
        attention(
            "warn",
            "!",
            "Token list may be out of date",
            "The token store could not be refreshed, so this page is showing the last in-memory snapshot. Inspect the gateway log before relying on these counts.",
            None,
        )
    } else {
        String::new()
    };
    let token_rows = connect_token_rows(&data.access, token_page);
    let connection_action = if !credential_ready {
        r##"<a class="primary-button" href="#access-tokens">Set up credential</a>"##
    } else if data.models.is_empty() {
        r##"<a class="primary-button" href="/mayhem/dashboard/models">Inspect catalog</a>"##
    } else if accepting_models == 0 {
        r##"<a class="primary-button" href="/mayhem/dashboard/models">Inspect availability</a>"##
    } else {
        r##"<a class="primary-button" href="/mayhem/dashboard/playground">Test with Playground</a>"##
    };
    let connection_title = if credential_ready {
        "Ready to connect"
    } else {
        "API key needed"
    };
    let connection_tone = if credential_ready { "good" } else { "warn" };
    let connection_mark = if credential_ready { "&#10003;" } else { "!" };
    let auth_summary = if data.requires_auth() {
        "API key required"
    } else {
        "No API key required"
    };
    let active_token_summary = if token_count == 1 {
        "1 active access token".to_owned()
    } else {
        format!("{token_count} active access tokens")
    };
    let local_scope_copy = if root.contains("127.0.0.1") || root.contains("localhost") {
        "This address works only for applications running on this computer."
    } else {
        "Use this address in applications that can reach this gateway."
    };
    let api_key_copy = if data.requires_auth() {
        "Authentication is enabled. Paste an active Mayhem access token into your app's API-key field."
    } else {
        "No API key is required. Some apps still require text in the API-key field, so use not-required as a harmless placeholder."
    };
    let credential_class = if credential_ready { "done" } else { "active" };
    let credential_mark = if credential_ready { "&#10003;" } else { "2" };
    let credential_copy = if data.requires_auth() {
        if credential_ready {
            "An active Mayhem token is available for authenticated requests."
        } else {
            "Create an active Mayhem token before connecting an app."
        }
    } else {
        "This gateway accepts requests without an API key."
    };
    let request_copy = if !credential_ready {
        "Available after an API key is configured."
    } else if data.models.is_empty() {
        "Inspect the catalog before sending a test request."
    } else if accepting_models == 0 {
        "Wait for model capacity before sending a test request."
    } else {
        "Not tested yet. Send one Playground request to confirm a model route."
    };
    let token_panel_open = if !credential_ready { " open" } else { "" };
    let token_scope_copy = if data.requires_auth() {
        "View API keys used by connected apps and learn how to create or revoke them."
    } else {
        "Optional here; useful for shared apps, budgets, and usage tracking."
    };
    let base_url_html = html_escape(&base_url);
    let env_block_html = html_escape(&env_block);
    let content = format!(
        r##"<section class="connect-ready {connection_tone}"><span class="connect-ready-mark" aria-hidden="true">{connection_mark}</span><div class="connect-ready-copy"><span>Connection status</span><h2>{connection_title}</h2><p>{auth_summary}<span aria-hidden="true"> &middot; </span>{active_token_summary}</p></div><span class="status-badge info">OpenAI-compatible</span></section><section class="dashboard-layout connect-layout"><div class="stack"><section class="panel connect-setup"><header class="panel-head"><div class="panel-title"><h2>Connection details</h2><p>Use these values in an app that accepts a custom OpenAI API address.</p></div></header><div class="panel-body connect-steps"><section class="connect-step"><div class="connect-step-heading"><span aria-hidden="true">1</span><div><h3>Copy your Mayhem address</h3><p>This tells the app where to send AI requests.</p></div></div><div class="code-block"><code id="gateway-base-url">{base_url_html}</code><button class="quiet-button copy-corner js-only" type="button" data-copy data-copy-target="#gateway-base-url" data-product-event="integration_base_url_copied" aria-label="Copy Mayhem API address"><span data-copy-label>Copy</span></button></div><p class="connect-helper"><strong>On this device:</strong> {local_scope_copy}</p></section><section class="connect-step"><div class="connect-step-heading"><span aria-hidden="true">2</span><div><h3>Paste the values into your app</h3><p>Apps may call these fields Base URL and API key.</p></div></div><pre class="code-block"><code id="gateway-env">{env_block_html}</code><button class="quiet-button copy-corner js-only" type="button" data-copy data-copy-target="#gateway-env" data-product-event="integration_environment_copied" aria-label="Copy connection values"><span data-copy-label>Copy</span></button></pre><p class="connect-helper">{api_key_copy}</p></section><section class="connect-step"><div class="connect-step-heading"><span aria-hidden="true">3</span><div><h3>Send a test request</h3><p>Check the gateway, then use Playground to confirm a real model route.</p></div></div><div class="connect-step-actions"><button class="quiet-button js-only" type="button" data-connection-test data-result-target="#connection-result">Check gateway connection</button>{connection_action}</div><div class="notice" id="connection-result" hidden></div></section></div></section></div><aside class="stack"><section class="panel connect-checklist"><header class="panel-head"><div class="panel-title"><h2>Connection checklist</h2><p>Three clear checks before using another app.</p></div></header><div class="panel-body"><div class="checklist"><div class="check-step done"><span class="check-mark">&#10003;</span><div class="check-copy"><strong>Gateway connection</strong><span>This dashboard can reach the Mayhem gateway.</span></div></div><div class="check-step {credential_class}"><span class="check-mark">{credential_mark}</span><div class="check-copy"><strong>API key</strong><span>{credential_copy}</span></div></div><div class="check-step pending"><span class="check-mark">3</span><div class="check-copy"><strong>Model request</strong><span>{request_copy}</span></div></div></div></div></section></aside></section>{token_store_notice}<details class="panel disclosure-panel token-management section-gap" id="access-tokens"{token_panel_open}><summary><span class="token-management-summary"><strong>Access tokens</strong><small>{token_scope_copy}</small></span><span class="status-badge">{token_count} active &middot; {token_total} total</span></summary><div class="token-management-content"><section class="token-create-guide" aria-labelledby="token-create-title"><div class="token-create-copy"><span class="token-create-kicker">Terminal setup</span><h2 id="token-create-title">Need a new token?</h2><p>Run this command in a terminal where Mayhem is installed. Give each app its own descriptive name.</p></div><div class="code-block token-create-command"><code id="token-create-command">mayhem tokens create --name my-app --budget 10/day --max-rate 60</code><button class="quiet-button copy-corner js-only" type="button" data-copy data-copy-target="#token-create-command" data-product-event="access_token_command_copied" aria-label="Copy access token creation command"><span data-copy-label>Copy</span></button></div><div class="token-secret-note"><span aria-hidden="true">!</span><p><strong>Copy the generated token immediately.</strong> The <code>sk-mayhem-...</code> secret is shown only once. Paste it into your app's API-key field and keep it private.</p></div></section><header class="panel-head"><div class="panel-title"><h2>Existing tokens</h2><p>This dashboard is read-only. It shows names, limits, and usage&mdash;never token secrets.</p></div>{token_filter_controls}</header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table" id="access-tokens-table"><caption class="sr-only">Gateway access tokens, budgets, scopes, and status</caption><thead><tr><th>Name</th><th>Budget use</th><th>Last used</th><th>Scope</th><th>Status</th></tr></thead><tbody>{token_rows}</tbody></table></div>{token_filter_empty}</div><footer class="panel-footer"><span>List with <code>mayhem tokens list</code> or revoke with <code>mayhem tokens revoke &lt;name&gt;</code>.</span><span>{token_summary}</span>{token_pagination}</footer></div></details>"##,
    );
    shell(
        data,
        expires,
        DashboardAppPage::Connect,
        "Integrations",
        "Connect another AI app",
        "Use OpenMayhem from any app that supports a custom OpenAI API address.",
        if credential_ready {
            "Connection ready"
        } else {
            "Credential needed"
        },
        if credential_ready { "good" } else { "warn" },
        "",
        &content,
    )
}

fn connect_token_rows(access: &Value, page: PageWindow) -> String {
    let Some(tokens) = access.get("tokens").and_then(Value::as_array) else {
        return r##"<tr><td colspan="5">No access-token store is available.</td></tr>"##.to_owned();
    };
    if tokens.is_empty() {
        return r##"<tr><td colspan="5"><div class="empty-block"><div class="empty-block-inner"><h3>No gateway tokens</h3><p>Create one with <code>mayhem tokens create --name &lt;name&gt;</code> when authentication is required.</p></div></div></td></tr>"##.to_owned();
    }
    let mut tokens = tokens.iter().collect::<Vec<_>>();
    tokens.sort_by(|left, right| {
        let active = |token: &&Value| {
            token
                .get("active")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        };
        let last_used = |token: &&Value| {
            token
                .get("last_used_at")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        };
        active(right)
            .cmp(&active(left))
            .then_with(|| last_used(right).cmp(&last_used(left)))
    });
    tokens
        .into_iter()
        .skip(page.start)
        .take(page.len())
        .map(|token| {
            let name = token.get("name").and_then(Value::as_str).unwrap_or("Unnamed token");
            let id = token.get("token_id").and_then(Value::as_str).unwrap_or("unknown");
            let active = token.get("active").and_then(Value::as_bool).unwrap_or(false);
            let period = token
                .get("budget_period")
                .and_then(Value::as_str)
                .unwrap_or("total");
            let spent = token
                .get("effective_spent_au")
                .or_else(|| {
                    token.get(if matches!(period, "day" | "month") {
                        "spent_period_au"
                    } else {
                        "spent_total_au"
                    })
                })
                .and_then(value_as_money_au)
                .map(format_au_usd)
                .unwrap_or_else(|| "$0.00".to_owned());
            let spend_window = match period {
                "day" => "current day window",
                "month" => "current 30-day window",
                _ => "lifetime total",
            };
            let budget = token
                .get("budget_au")
                .and_then(value_as_money_au)
                .map(format_au_usd)
                .unwrap_or_else(|| "No cap".to_owned());
            let last = token
                .get("last_used_at")
                .and_then(Value::as_u64)
                .map(|value| format!("{} ago", format_elapsed_since(timestamp_seconds(value))))
                .unwrap_or_else(|| "Never".to_owned());
            let models = token
                .get("models")
                .and_then(Value::as_array)
                .map(|values| values.len())
                .unwrap_or(0);
            let scope = if models == 0 {
                "All models".to_owned()
            } else {
                format!("{models} allowed")
            };
            let status = if active { "Active" } else { "Inactive" };
            format!(
                r##"<tr data-filter-row data-filter-text="{} {} {} {} {}"><td data-export-value="{} / token {}" data-sort-value="{}"><span class="table-primary">{}</span><span class="table-secondary mono">{}</span></td><td data-money data-export-value="{} / {} / {}"><span class="money-value">{} / {}</span><span class="table-secondary">{}</span></td><td data-sort-value="{}">{}</td><td>{}</td><td><span class="status-badge {}">{}</span></td></tr>"##,
                html_escape(name),
                html_escape(id),
                html_escape(&scope),
                status,
                html_escape(spend_window),
                html_escape(name),
                html_escape(id),
                html_escape(name),
                html_escape(name),
                html_escape(short_text(id, 20).as_ref()),
                html_escape(&spent),
                html_escape(&budget),
                spend_window,
                html_escape(&spent),
                html_escape(&budget),
                spend_window,
                token.get("last_used_at").and_then(Value::as_u64).unwrap_or(0),
                html_escape(&last),
                scope,
                if active { "good" } else { "" },
                status,
            )
        })
        .collect()
}

fn earn_overview_page(
    data: &DashboardData,
    expires: u64,
    requested: Option<&str>,
    requested_page: Option<&str>,
) -> String {
    let entries = data.provider_entries(requested);
    let state = provider_page_state(data, requested, &entries);
    let slots = provider_slot_totals(data, &entries);
    let freshness = provider_freshness_window(data, &entries);
    let active_slots = provider_current_value(
        format!("{} / {}", slots.active, slots.max),
        &slots,
        freshness,
    );
    let free_capacity = provider_current_value(slots.free.to_string(), &slots, freshness);
    let queue_backlog = provider_current_value(slots.backlog.to_string(), &slots, freshness);
    let coverage = provider_coverage_notice(&slots, freshness);
    // Route inspection may be requested from the URL, but money is private to the
    // provider identity configured by this gateway process.
    let provider_id = data.local_provider_id.as_deref();
    let subnav = earn_subnav(DashboardProductPage::Earn);
    let identity_attention = if data.local_provider_id.is_none() && requested.is_none() {
        // The overview heading and status already own this state. Subpages still
        // receive the contextual notice because their headings describe a task.
        String::new()
    } else {
        provider_identity_attention(data, requested)
    };
    let reliability = provider_reliability_range(&entries);
    let claimable = provider_claimable(data, provider_id);
    let heartbeat_metrics_available = slots.total_routes > 0 && slots.fresh_routes > 0;
    let metrics = if heartbeat_metrics_available {
        format!(
            r##"<section class="metric-grid">{}{}{}{}</section>"##,
            metric(
                "Active slots",
                &active_slots,
                "Jobs running now out of the advertised maximum",
                "Fresh heartbeat"
            ),
            metric(
                "Free capacity",
                &free_capacity,
                "Open slots advertised to the network",
                "Fresh heartbeat"
            ),
            metric(
                "Queue backlog",
                &queue_backlog,
                "Requests waiting across fresh routes",
                "Fresh heartbeat"
            ),
            metric(
                "Lowest route reputation",
                &reliability,
                "The weakest reputation among this provider's contracts",
                "Contract"
            ),
        )
    } else {
        let (waiting_label, waiting_meta) = if slots.total_routes == 0 {
            (
                "No routes yet",
                "Slots, capacity, and queue totals appear once a route is configured and sends a heartbeat.",
            )
        } else {
            (
                "Waiting for fresh heartbeat",
                "Slots, capacity, and queue totals return with the next fresh heartbeat.",
            )
        };
        let reputation_metric = if reliability == "Unavailable" {
            String::new()
        } else {
            metric(
                "Lowest route reputation",
                &reliability,
                "The weakest reputation among this provider's contracts",
                "Contract",
            )
        };
        format!(
            r##"<section class="metric-grid">{}{reputation_metric}</section>"##,
            metric_status(
                "Live route metrics",
                &status_badge(waiting_label, "warn"),
                waiting_meta,
                "Fresh heartbeat"
            ),
        )
    };
    let metrics = format!("{metrics}{coverage}");
    let route_page = PageWindow::from_query(entries.len(), MAX_PROVIDER_ROWS, requested_page);
    let route_rows = provider_route_rows(data, &entries, route_page);
    let route_summary = route_page.status("configured serving routes");
    let route_pagination = pagination_nav_with_optional_provider(
        route_page,
        "/mayhem/dashboard/earn",
        requested,
        "configured serving routes",
    );
    let (route_filter_controls, route_filter_empty) = shown_rows_filter(
        "earn-routes",
        "earn-routes-table",
        "Filter serving routes on the shown page by model, room, or state",
        route_page.len(),
        "serving routes",
    );
    let action = provider_action_center(&state);
    let primary_action = match (state.kind, state.label) {
        (RouteStateKind::Accepting, _) => {
            r##"<a class="primary-button" href="/mayhem/dashboard/earn/machines">Inspect machines</a>"##
        }
        (RouteStateKind::Failed, _) => {
            r##"<a class="primary-button" href="/mayhem/dashboard/earn/machines">Inspect failure</a>"##
        }
        (RouteStateKind::Waiting, "Preparing a model" | "Prepared; waiting for heartbeat") => {
            r##"<a class="primary-button" href="/mayhem/dashboard/earn/machines">Inspect preparation</a>"##
        }
        _ => "",
    };
    let claimable_freshness = if claimable.confirmed {
        earnings_freshness_window(&data.earnings).map_or_else(
            || html_escape(&claimable.freshness),
            |window| volatile_relative_label("Refreshed", window, "Refresh to reconfirm amounts"),
        )
    } else {
        html_escape(&claimable.freshness)
    };
    let content = format!(
        r##"{subnav}{identity_attention}{activation}{metrics}<section class="dashboard-layout"><div class="stack"><section class="panel"><header class="panel-head"><div class="panel-title"><h2>Serving routes</h2><p>Every status combines heartbeat freshness, acceptance, and slot capacity.</p></div>{route_filter_controls}</header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table" id="earn-routes-table"><caption class="sr-only">Configured provider serving routes and current capacity</caption><thead><tr><th>Model / room</th><th>State</th><th>Slots</th><th>Queue</th><th>Performance</th></tr></thead><tbody>{route_rows}</tbody></table></div>{route_filter_empty}</div><footer class="panel-footer"><span>{route_summary}</span>{route_pagination}</footer></section></div><aside class="stack">{action}<section class="panel"><header class="panel-head"><div class="panel-title"><h2>Settlement snapshot</h2><p>Last-known canonical ledger fields with explicit freshness</p></div></header><div class="panel-body"><div class="fact-grid"><div class="fact"><span>Last known claimable on {}</span><strong data-money><span class="money-value">{}</span></strong></div><div class="fact"><span>Refresh</span><strong>{claimable_freshness}</strong></div></div></div><footer class="panel-footer"><span>{}</span><a href="/mayhem/dashboard/earn/earnings">Open earnings</a></footer></section></aside></section>"##,
        html_escape(&data.rail.to_ascii_uppercase()),
        html_escape(&claimable.value),
        html_escape(claimable.basis),
        activation = provider_activation_panel(data, &state, &entries),
    );
    shell(
        data,
        expires,
        DashboardAppPage::Earn,
        "Provider operations",
        "Provider overview",
        &state.explanation,
        state.label,
        state.tone,
        primary_action,
        &content,
    )
}

fn provider_job_records<'a>(
    data: &'a DashboardData,
    provider: Option<&str>,
) -> Vec<&'a StoredReceipt> {
    let Some(provider) = provider else {
        return Vec::new();
    };
    data.receipts
        .iter()
        .filter(|receipt| receipt.receipt.body.provider == provider)
        .collect()
}

fn provider_jobs_chart(data: &DashboardData, jobs: &[&StoredReceipt]) -> String {
    let today = now_secs() / 86_400;
    let mut days = [0usize; 7];
    for receipt in jobs
        .iter()
        .filter(|receipt| receipt.receipt.body.final_receipt)
    {
        let receipt_day = timestamp_seconds(receipt.receipt.body.ts) / 86_400;
        let age = today.saturating_sub(receipt_day);
        if age < 7 {
            days[6 - age as usize] += 1;
        }
    }
    let total = days.iter().sum::<usize>();
    if total == 0 {
        return String::new();
    }
    let max = days.iter().copied().max().unwrap_or(1).max(1);
    let bars = days
        .into_iter()
        .enumerate()
        .map(|(index, count)| {
            let level = if count == 0 {
                0
            } else {
                ((count * 10).div_ceil(max)).max(1)
            };
            let label = if index == 6 {
                "Today".to_owned()
            } else {
                format!("{}d ago", 6 - index)
            };
            format!(
                r##"<li aria-label="{label}: {count} completed provider job{}"><span class="usage-bar level-{level}"><span></span></span><strong>{count}</strong><small>{label}</small></li>"##,
                if count == 1 { "" } else { "s" },
            )
        })
        .collect::<String>();
    format!(
        r##"<figure class="panel usage-chart section-gap"><figcaption class="panel-head"><div class="panel-title"><h2>Completed provider work</h2><p>Last 7 calendar days from {}; receipt evidence, not payout state</p></div><strong>{total}</strong></figcaption><div class="panel-body"><ol class="usage-bars">{bars}</ol></div></figure>"##,
        html_escape(&data.history_scope().to_ascii_lowercase()),
    )
}

fn provider_activation_panel(
    data: &DashboardData,
    state: &RouteState,
    entries: &[&ProviderTableEntry],
) -> String {
    let provider = data.local_provider_id.as_deref();
    let progress = provider.and_then(|provider| latest_provider_progress(data, provider));
    let identity_done = provider.is_some();
    let prepared_done = !entries.is_empty() || progress.is_some_and(progress_is_terminal);
    let route_done = !entries.is_empty();
    let fresh_done = provider_freshness_window(data, entries).is_some();
    let first_job_done = provider_job_records(data, provider)
        .iter()
        .any(|receipt| receipt.receipt.body.final_receipt);
    let earnings_done = provider.is_some_and(|provider| {
        data.earnings
            .entries
            .iter()
            .any(|entry| entry.get("provider").and_then(Value::as_str) == Some(provider))
    });
    let steps = [
        (
            identity_done,
            "Provider identity",
            "This gateway can scope machine state and private earnings to the configured provider.",
        ),
        (
            prepared_done,
            "Model prepared",
            "The provider reports completed preparation or has already published a matching route.",
        ),
        (
            route_done && fresh_done,
            "Route confirmed",
            "A matching route is published and this gateway has current heartbeat evidence.",
        ),
        (
            first_job_done,
            "First completed job",
            if data.history_persistent {
                "A final signed receipt for this provider is present in durable gateway history."
            } else {
                "A final signed receipt for this provider is present in the current gateway run."
            },
        ),
        (
            earnings_done,
            "Earnings record",
            "The canonical ledger exposes an earnings record for this provider identity.",
        ),
    ];
    let current = steps.iter().position(|(done, _, _)| !done);
    let complete = steps.iter().filter(|(done, _, _)| *done).count();
    let step_markup = steps
        .into_iter()
        .enumerate()
        .map(|(index, (done, label, detail))| {
            let class_name = if done {
                "done"
            } else if Some(index) == current {
                "active"
            } else {
                ""
            };
            let marker = if done {
                "&#10003;".to_owned()
            } else {
                (index + 1).to_string()
            };
            let state_label = if done {
                "Complete"
            } else if Some(index) == current {
                "Current"
            } else {
                "Not started"
            };
            format!(
                r##"<li class="check-step {class_name}"><span class="check-mark" aria-hidden="true">{marker}</span><div class="check-copy"><span class="sr-only">{state_label}: </span><strong>{}</strong><span>{}</span></div></li>"##,
                html_escape(label),
                html_escape(detail),
            )
        })
        .collect::<String>();
    let next = if !identity_done {
        r##"<div class="provider-start-command"><span class="field-label">Run on this machine</span><pre class="code-block"><code id="provider-start-command">mayhem up --provider --yes</code><button class="quiet-button copy-corner js-only" type="button" data-copy data-copy-target="#provider-start-command" aria-label="Copy provider start command"><span data-copy-label>Copy</span></button></pre></div>"##.to_owned()
    } else if !prepared_done || !route_done || !fresh_done {
        // In a failed state the page-level "Inspect failure" action is the one
        // primary; this panel's link stays secondary to avoid competing CTAs.
        if state.kind == RouteStateKind::Failed {
            r##"<a class="soft-button" href="/mayhem/dashboard/earn/machines">Continue setup</a>"##
                .to_owned()
        } else {
            r##"<a class="primary-button" href="/mayhem/dashboard/earn/machines">Continue setup</a>"##
                .to_owned()
        }
    } else if !first_job_done {
        r##"<a class="primary-button" href="/mayhem/dashboard/earn/jobs">Open jobs</a>"##.to_owned()
    } else if !earnings_done {
        r##"<a class="primary-button" href="/mayhem/dashboard/earn/earnings">Check settlement</a>"##
            .to_owned()
    } else {
        r##"<a class="soft-button" href="/mayhem/dashboard/earn/jobs">Review jobs</a>"##.to_owned()
    };
    format!(
        r##"<section class="panel activation-panel"><header class="panel-head"><div class="panel-title"><h2>Provider activation</h2><p>{complete} of 5 milestones confirmed from gateway evidence</p></div><span class="status-badge {}">{}</span></header><div class="panel-body activation-grid"><ol class="checklist">{step_markup}</ol><div class="activation-next"><strong>Next best action</strong><p>{}</p>{next}</div></div></section>"##,
        state.tone,
        html_escape(state.label),
        html_escape(if complete == 5 {
            "Your provider path is complete. Keep the machine healthy and review only exceptions."
        } else if !identity_done {
            "Start the supervised provider flow; hardware fit, model preparation, and runtime health remain visible here."
        } else {
            "Complete the current milestone. Later steps will unlock only from authoritative evidence."
        }),
    )
}

fn earn_jobs_page(
    data: &DashboardData,
    expires: u64,
    requested: Option<&str>,
    requested_page: Option<&str>,
) -> String {
    let provider = data.local_provider_id.as_deref();
    let jobs = provider_job_records(data, provider);
    let completed = jobs
        .iter()
        .filter(|receipt| receipt.receipt.body.final_receipt)
        .count();
    let incomplete = jobs.len().saturating_sub(completed);
    let total_metered = jobs
        .iter()
        .filter(|receipt| receipt.receipt.body.final_receipt)
        .fold(0_u128, |total, receipt| {
            total.saturating_add(receipt.receipt.body.au_owed_cum)
        });
    let page = PageWindow::from_query(jobs.len(), MAX_ACTIVITY_ROWS, requested_page);
    let rows = jobs
        .iter()
        .skip(page.start)
        .take(page.len())
        .map(|receipt| {
            let body = &receipt.receipt.body;
            let status = if body.final_receipt {
                ("Completed", "good", "Final signed receipt")
            } else {
                ("Needs review", "warn", "Latest receipt is non-final")
            };
            let metered_note = if body.final_receipt {
                "metered total"
            } else {
                "metered so far"
            };
            let evidence = evidence_link(
                &evidence_href("receipt", &[("id", body.session_id.as_str())]),
                "Verify",
                &body.session_id,
            );
            format!(
                r##"<tr data-filter-row data-filter-text="{} {} {} {}"><td data-sort-value="{}"><span class="table-primary">{} ago</span><span class="table-secondary mono">{}</span></td><td><span class="table-primary">{}</span><span class="table-secondary">{} in / {} out</span></td><td data-money><span class="table-primary money-value">{}</span><span class="table-secondary">{metered_note}</span></td><td><span class="status-badge {}">{}</span><span class="table-secondary">{}</span></td><td><span class="table-secondary">Settles on the ledger</span></td><td>{}</td></tr>"##,
                html_escape(&body.model_id),
                html_escape(&body.session_id),
                status.0,
                html_escape(&body.provider),
                timestamp_seconds(body.ts),
                html_escape(&format_elapsed_since(timestamp_seconds(body.ts))),
                html_escape(short_text(&body.session_id, 18).as_ref()),
                html_escape(short_text(&body.model_id, 32).as_ref()),
                body.usage.prompt_tokens(),
                body.usage.output_tokens(),
                html_escape(&format_au_usd(body.au_owed_cum)),
                status.1,
                status.0,
                status.2,
                evidence,
            )
        })
        .collect::<String>();
    let body = if rows.is_empty() {
        format!(
            r##"<tr><td colspan="6">{}</td></tr>"##,
            empty_block(
                if provider.is_some() {
                    "Waiting for the first job"
                } else {
                    "Provider setup has not started"
                },
                if provider.is_some() {
                    "Keep the provider healthy. A signed receipt will appear here after this gateway observes completed work for the configured provider."
                } else {
                    "Start the provider flow from Earn overview before expecting job evidence."
                },
                None,
            )
        )
    } else {
        rows
    };
    let (filter, filter_empty) = shown_rows_filter(
        "provider-jobs",
        "provider-jobs-table",
        "Filter observed jobs by model, session, provider, or state",
        page.len(),
        "jobs",
    );
    let pagination = pagination_nav_with_optional_provider(
        page,
        "/mayhem/dashboard/earn/jobs",
        requested,
        "observed jobs",
    );
    let activity_chart = provider_jobs_chart(data, &jobs);
    let content = format!(
        r##"{}{}<section class="metric-grid">{}{}{}</section>{}{activity_chart}<section class="panel section-gap"><header class="panel-head"><div class="panel-title"><h2>Observed jobs</h2><p>The latest signed metering record for each session this provider served.</p></div>{filter}</header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table" id="provider-jobs-table"><caption class="sr-only">Gateway-observed provider jobs and signed receipt status</caption><thead><tr><th>Observed</th><th>Model and usage</th><th>Metered amount</th><th>Receipt status</th><th>Settlement</th><th>Evidence</th></tr></thead><tbody>{body}</tbody></table></div>{filter_empty}</div><footer class="panel-footer"><span>{}</span>{pagination}<a href="/mayhem/dashboard/earn/earnings">Open earnings</a></footer></section>"##,
        earn_subnav(DashboardProductPage::EarnJobs),
        provider_identity_attention(data, requested),
        metric(
            "Observed jobs",
            &jobs.len().to_string(),
            "Latest record per session",
            data.history_scope()
        ),
        metric(
            "Completed",
            &completed.to_string(),
            "Final signed receipts",
            "Gateway evidence"
        ),
        metric(
            "Metered total",
            &money_html(&format_au_usd(total_metered)),
            "Across completed jobs — payouts settle on the ledger",
            "Receipts"
        ),
        if incomplete > 0 {
            attention("warn", "!", "Open jobs to review", &format!("{} still waiting on a final receipt. An open record does not mean work is still running.", count_noun(incomplete as u64, "observed session")), Some(("Review activity", "/mayhem/dashboard/activity")))
        } else {
            String::new()
        },
        page.status("observed jobs"),
    );
    shell_wide(
        data,
        expires,
        DashboardAppPage::Earn,
        "Provider work",
        "Jobs",
        "Each job this gateway observed, with its signed metering record.",
        if provider.is_none() {
            "Setup required"
        } else if incomplete > 0 {
            "Review incomplete work"
        } else if jobs.is_empty() {
            "Waiting for first job"
        } else {
            "Jobs recorded"
        },
        if provider.is_none() || incomplete > 0 {
            "warn"
        } else if jobs.is_empty() {
            ""
        } else {
            "good"
        },
        if provider.is_none() {
            r##"<a class="primary-button" href="/mayhem/dashboard/earn">Start provider setup</a>"##
        } else {
            ""
        },
        &content,
    )
}

fn earn_machines_page(
    data: &DashboardData,
    expires: u64,
    requested: Option<&str>,
    requested_page: Option<&str>,
) -> String {
    let entries = data.provider_entries(requested);
    let state = provider_page_state(data, requested, &entries);
    let route_page = PageWindow::from_query(entries.len(), MAX_PROVIDER_ROWS, requested_page);
    let rows = provider_machine_rows(data, &entries, route_page);
    let progress = provider_progress_notice(data, None);
    let recovery = provider_recovery_panel(data, &state);
    let route_summary = route_page.status("configured machine routes");
    let route_pagination = pagination_nav_with_optional_provider(
        route_page,
        "/mayhem/dashboard/earn/machines",
        requested,
        "configured machine routes",
    );
    let (route_filter_controls, route_filter_empty) = shown_rows_filter(
        "machine-routes",
        "machine-routes-table",
        "Filter machine routes on the shown page by route, model, or state",
        route_page.len(),
        "machine routes",
    );
    let content = format!(
        r##"{}{identity}{progress}{recovery}<section class="panel"><header class="panel-head"><div class="panel-title"><h2>Routes on this provider identity</h2><p>The gateway currently identifies workers by provider, enclave, room, and model.</p></div>{route_filter_controls}</header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table" id="machine-routes-table"><caption class="sr-only">Machine routes for the configured provider identity</caption><thead><tr><th>Route</th><th>Model</th><th>Operational state</th><th>Capacity</th><th>Freshness</th><th>Evidence</th></tr></thead><tbody>{rows}</tbody></table></div>{route_filter_empty}</div><footer class="panel-footer"><span>{route_summary} Temperature, power, and VRAM telemetry are unavailable.</span>{route_pagination}</footer></section>"##,
        earn_subnav(DashboardProductPage::EarnMachines),
        identity = provider_identity_attention(data, requested),
    );
    shell_wide(
        data,
        expires,
        DashboardAppPage::Earn,
        "Provider operations",
        "Machines and serving routes",
        "Each configured route with its latest heartbeat, capacity, and performance.",
        state.label,
        state.tone,
        "",
        &content,
    )
}

fn earn_opportunities_page(
    data: &DashboardData,
    expires: u64,
    requested: Option<&str>,
    requested_page: Option<&str>,
) -> String {
    let page = PageWindow::from_query(data.models.len(), MAX_MODEL_ROWS, requested_page);
    let rows = if data.models.is_empty() {
        format!(
            r##"<tr><td colspan="4">{}</td></tr>"##,
            empty_block(
                "No catalog models available",
                "This gateway has no catalog snapshot to evaluate for host compatibility.",
                None,
            )
        )
    } else {
        data.models.iter().skip(page.start).take(page.len()).map(|model| {
            let local = model.mayhem.route_candidates.iter().find_map(|candidate| candidate.local_run.as_ref());
            let (fit, detail) = local.map(|badge| (badge.label.as_str(), format!("{} / memory {}/{} / download {}", badge.reason, badge.memory_required_human, badge.memory_budget_human, badge.download_human))).unwrap_or(("Not evaluated", "No gateway-host hardware-fit annotation in the catalog snapshot".to_owned()));
            let availability = model_availability(data, model);
            format!(r##"<tr data-filter-row><td><span class="table-primary mono">{}</span><span class="table-secondary">{}</span></td><td><span class="status-badge">{}</span><span class="table-secondary">{}</span></td><td><span class="status-badge {}">{}</span><span class="table-secondary">{}</span></td><td>{}</td></tr>"##, html_escape(&model.id), html_escape(&model.mayhem.model_class), html_escape(fit), html_escape(&detail), availability.tone, html_escape(availability.label), html_escape(&availability.explanation), money_html(&dashboard_model_price(model)))
        }).collect::<String>()
    };
    let catalog_summary = page.status("catalog models");
    let pagination = requested
        .filter(|provider| !provider.is_empty())
        .map(|provider| {
            pagination_nav(
                page,
                "/mayhem/dashboard/earn/opportunities",
                &[("provider", provider)],
                "catalog models",
            )
        })
        .unwrap_or_else(|| {
            pagination_nav(
                page,
                "/mayhem/dashboard/earn/opportunities",
                &[],
                "catalog models",
            )
        });
    let (filter_controls, filter_empty) = shown_rows_filter(
        "model-fit",
        "model-fit-table",
        "Filter the shown models by model, fit, supply, or price",
        page.len(),
        "models",
    );
    let content = format!(
        r##"{}{}<div class="notice"><strong>Gateway-host evidence only.</strong> A fit result does not prove that a remote worker can serve the model or that demand, revenue, or earnings exist.</div><section class="panel section-gap"><header class="panel-head"><div class="panel-title"><h2>Gateway-host model fit</h2><p>Catalog compatibility beside current advertised supply.</p></div>{filter_controls}</header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table" id="model-fit-table"><caption class="sr-only">Catalog models, gateway-host compatibility, and advertised supply</caption><thead><tr><th>Model</th><th>Gateway host fit</th><th>Advertised supply</th><th>Catalog price</th></tr></thead><tbody>{rows}</tbody></table></div>{filter_empty}</div><footer class="panel-footer"><span>{catalog_summary}</span>{pagination}</footer></section>"##,
        earn_subnav(DashboardProductPage::EarnOpportunities),
        provider_identity_attention(data, requested),
    );
    let provider_entries = data.provider_entries(requested);
    let state = provider_page_state(data, requested, &provider_entries);
    shell_wide(
        data,
        expires,
        DashboardAppPage::Earn,
        "Provider planning",
        "Model opportunities",
        "Which catalog models this machine can run, next to what the network currently supplies.",
        state.label,
        state.tone,
        "",
        &content,
    )
}

fn earn_earnings_page(data: &DashboardData, expires: u64, requested: Option<&str>) -> String {
    // Never use a query-selected provider as authority for earnings visibility.
    let provider_id = data.local_provider_id.as_deref();
    let rows = provider_earning_rows(data, provider_id);
    let snapshot_fresh = earnings_snapshot_is_fresh(&data.earnings);
    let snapshot_state = if data.earnings.entries.is_empty() && snapshot_fresh {
        "No earnings records"
    } else if data.earnings.entries.is_empty() && data.earnings.refreshed_at_seconds.is_none() {
        "No earnings snapshot"
    } else if snapshot_fresh {
        "Ledger snapshot current"
    } else {
        "Refresh to reconfirm"
    };
    let snapshot_tone = if snapshot_fresh && data.earnings.entries.is_empty() {
        ""
    } else if snapshot_fresh {
        "good"
    } else {
        ""
    };
    let snapshot_window = earnings_freshness_window(&data.earnings);
    let snapshot_badge = snapshot_window.map_or_else(
        || {
            format!(
                r#"<span class="status-badge {}">{}</span>"#,
                snapshot_tone,
                html_escape(snapshot_state)
            )
        },
        |window| {
            volatile_status_badge(
                snapshot_state,
                snapshot_tone,
                window,
                "Refresh to reconfirm",
            )
        },
    );
    let refresh_label = snapshot_window.map_or_else(
        || html_escape(&earnings_refresh_label(&data.earnings)),
        |window| volatile_relative_label("Refreshed", window, "Refresh to reconfirm amounts"),
    );
    let content = format!(
        r##"{}{}<div class="notice"><strong>Cumulative totals.</strong> Recorded, held, claimable, and paid are lifetime ledger fields &mdash; not hourly earnings.</div><section class="panel section-gap"><header class="panel-head"><div class="panel-title"><h2>Earnings records</h2><p>For this provider identity, grouped by payment rail.</p></div>{snapshot_badge}</header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table"><caption class="sr-only">Canonical provider earnings and payout state by rail</caption><thead><tr><th>Rail</th><th>Total recorded</th><th>Held</th><th>Claimable</th><th>Paid cumulative</th><th>Ledger epoch</th><th>Evidence</th></tr></thead><tbody>{rows}</tbody></table></div></div><footer class="panel-footer"><span>{refresh_label}</span><span>Verify opens the exact ledger record behind each row.</span></footer></section>"##,
        earn_subnav(DashboardProductPage::EarnEarnings),
        provider_identity_attention(data, requested),
    );
    shell(
        data,
        expires,
        DashboardAppPage::Earn,
        "Provider finance",
        "Earnings and payouts",
        "What this provider has recorded, what is held, and what is claimable — straight from the ledger.",
        snapshot_state,
        snapshot_tone,
        "",
        &content,
    )
}

fn earn_reliability_page(
    data: &DashboardData,
    expires: u64,
    requested: Option<&str>,
    requested_page: Option<&str>,
) -> String {
    let entries = data.provider_entries(requested);
    let route_page = PageWindow::from_query(entries.len(), MAX_PROVIDER_ROWS, requested_page);
    let rows = entries.iter().skip(route_page.start).take(route_page.len()).map(|entry| {
        let probation = provider_probation_progress(entry.contract.probation.as_ref());
        let probation_summary = provider_probation_summary(entry.contract.probation.as_ref());
        let observed = &entry.observed;
        let sample = if observed.samples == 0 { "No gateway samples".to_owned() } else { format!("{} samples / {:.1}% observed error", observed.samples, observed.ewma_error_rate.clamp(0.0, 1.0) * 100.0) };
        let evidence = evidence_link(&evidence_href("route", &[("provider", entry.key.provider.as_str()), ("enclave", entry.key.enclave_id.as_str()), ("room", entry.key.room_id.as_str()), ("model", entry.contract.model_id.as_str())]), "Verify", &entry.contract.model_id);
        let reputation = entry.contract.reputation.clamp(0.0, 1.0);
        format!(r##"<tr data-filter-row data-filter-text="{} {} {} {}"><td data-export-value="{} / room {}" data-sort-value="{} / {}"><span class="table-primary mono">{}</span><span class="table-secondary">room {}</span></td><td data-export-value="{:.2}% / canonical contract score" data-sort-value="{:.8}"><span class="table-primary">{:.2}%</span><span class="table-secondary">canonical contract score</span></td><td data-export-value="{}">{probation}</td><td data-sort-value="{}">{}</td><td>{}</td></tr>"##, html_escape(&entry.contract.model_id), html_escape(&entry.key.room_id), html_escape(&probation_summary), html_escape(&sample), html_escape(&entry.contract.model_id), html_escape(&entry.key.room_id), html_escape(&entry.contract.model_id), html_escape(&entry.key.room_id), html_escape(short_text(&entry.contract.model_id, 30).as_ref()), html_escape(short_text(&entry.key.room_id, 14).as_ref()), reputation * 100.0, reputation, reputation * 100.0, html_escape(&probation_summary), observed.samples, html_escape(&sample), evidence)
    }).collect::<String>();
    let route_summary = route_page.status("provider reliability routes");
    let route_pagination = pagination_nav_with_optional_provider(
        route_page,
        "/mayhem/dashboard/earn/reliability",
        requested,
        "provider reliability routes",
    );
    let (route_filter_controls, route_filter_empty) = shown_rows_filter(
        "reliability-routes",
        "reliability-routes-table",
        "Filter reliability routes on the shown page by model, room, probation, or observation",
        route_page.len(),
        "reliability routes",
    );
    let content = format!(
        r##"{}{}<section class="panel"><header class="panel-head"><div class="panel-title"><h2>Contract reputation and probation</h2><p>The protocol reputation field stays separate from exact probation requirements and gateway observations.</p></div>{route_filter_controls}</header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table" id="reliability-routes-table"><caption class="sr-only">Provider route reputation, probation, and gateway observations</caption><thead><tr><th>Route</th><th>Contract reputation</th><th>Probation</th><th>Gateway observations</th><th>Evidence</th></tr></thead><tbody>{}</tbody></table></div>{route_filter_empty}</div><footer class="panel-footer"><span>{route_summary} Gateway samples and the contract score remain separate.</span>{route_pagination}</footer></section>"##,
        earn_subnav(DashboardProductPage::EarnReliability),
        provider_identity_attention(data, requested),
        if rows.is_empty() {
            format!(
                r##"<tr><td colspan="5">{}</td></tr>"##,
                empty_block(
                    "No reliability evidence yet",
                    "Reputation and probation appear after a provider route contract matches this gateway identity.",
                    None,
                )
            )
        } else {
            rows
        }
    );
    let state = aggregate_provider_state(&entries);
    shell_wide(
        data,
        expires,
        DashboardAppPage::Earn,
        "Provider quality",
        "Reliability",
        "Network reputation, probation requirements, and what this gateway has observed.",
        state.label,
        state.tone,
        "",
        &content,
    )
}

fn provider_probation_progress(probation: Option<&ProviderProbation>) -> String {
    let Some(probation) = probation else {
        return r##"<span class="status-badge">Not reported</span><span class="table-secondary">No protocol probation state is present.</span>"##.to_owned();
    };
    if !probation.active {
        return format!(
            r##"<span class="status-badge good">Complete</span><span class="table-secondary">{} / {} successful sessions reported</span>"##,
            probation.successful_sessions, probation.required_successful_sessions,
        );
    }
    let required = probation.required_successful_sessions;
    if required == 0 {
        return r##"<span class="status-badge info">Probation active</span><span class="table-secondary">No nonzero successful-session requirement is reported, so no percentage is shown.</span>"##.to_owned();
    }
    let successful = probation.successful_sessions;
    let progress_value = successful.min(required);
    let session_state = if successful >= required {
        "Successful-session condition met".to_owned()
    } else {
        format!(
            "{} remaining",
            count_noun((required - successful) as u64, "successful session")
        )
    };
    let other_conditions = if probation.required_seconds > 0 {
        "The elapsed-time condition remains separate from this bar."
    } else {
        "This bar covers the reported successful-session condition only."
    };
    format!(
        r##"<div class="table-progress"><span class="status-badge info">Probation active</span><strong>{successful} / {required} successful sessions</strong><progress max="{required}" value="{progress_value}" aria-label="Probation successful-session requirement: {successful} of {required}">{successful} of {required}</progress><span class="table-secondary">{} {}</span></div>"##,
        html_escape(&session_state),
        html_escape(other_conditions),
    )
}

fn provider_probation_summary(probation: Option<&ProviderProbation>) -> String {
    let Some(probation) = probation else {
        return "Not reported / no protocol probation state".to_owned();
    };
    if !probation.active {
        return format!(
            "Complete / {} of {} successful sessions reported",
            probation.successful_sessions, probation.required_successful_sessions,
        );
    }
    if probation.required_successful_sessions == 0 {
        return "Probation active / no nonzero successful-session requirement reported".to_owned();
    }
    format!(
        "Probation active / {} of {} successful sessions / {} second elapsed-time condition",
        probation.successful_sessions,
        probation.required_successful_sessions,
        probation.required_seconds,
    )
}

fn network_overview_page(data: &DashboardData, expires: u64) -> String {
    let providers = data
        .models
        .iter()
        .flat_map(|model| {
            model
                .mayhem
                .route_candidates
                .iter()
                .map(|candidate| candidate.provider.as_str())
        })
        .collect::<BTreeSet<_>>()
        .len();
    let unavailable = data.models.len().saturating_sub(data.accepting_models());
    let capacity_window = volatile_capacity_window(data);
    let fresh_routes = capacity_window.map_or_else(
        || data.fresh_routes().to_string(),
        |window| volatile_text(&data.fresh_routes().to_string(), window, "Unavailable"),
    );
    let unavailable_models = capacity_window.map_or_else(
        || unavailable.to_string(),
        |window| volatile_text(&unavailable.to_string(), window, "Unavailable"),
    );
    let shortage_summary =
        bounded_rows_summary(unavailable, 12, "models without advertised capacity");
    let shortage_rows = data.models.iter().filter(|model| model_availability(data, model).tone != "good").take(12).map(|model| {
        let state = model_availability(data, model);
        let explanation = model_freshness_window(data, model).map_or_else(
            || html_escape(&state.explanation),
            |window| volatile_text(&state.explanation, window, "Refresh to reconfirm"),
        );
        format!(r##"<tr><td><span class="table-primary mono">{}</span></td><td><span class="status-badge {}">{}</span></td><td>{}</td><td>{}</td></tr>"##, html_escape(&model.id), state.tone, html_escape(state.label), model.mayhem.route_candidates.len(), explanation)
    }).collect::<String>();
    let content = format!(
        r##"{}<section class="metric-grid">{}{}{}{}</section><section class="panel"><header class="panel-head"><div class="panel-title"><h2>Supply exceptions</h2><p>Models without a fresh route advertising free capacity</p></div></header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table"><caption class="sr-only">Catalog models without fresh advertised capacity</caption><thead><tr><th>Model</th><th>State</th><th>Routes</th><th>Why</th></tr></thead><tbody>{}</tbody></table></div></div><footer class="panel-footer"><span>{shortage_summary}</span></footer></section>"##,
        network_subnav(DashboardProductPage::Network),
        metric(
            "Catalog models",
            &data.models.len().to_string(),
            "Current contract catalog",
            "Catalog"
        ),
        metric(
            "Providers",
            &providers.to_string(),
            "Distinct providers in the current catalog",
            "Catalog"
        ),
        metric(
            "Fresh routes",
            &fresh_routes,
            "Routes with a live heartbeat right now",
            "Live"
        ),
        metric(
            "Models without capacity",
            &unavailable_models,
            "No fresh advertised capacity",
            "Live"
        ),
        if shortage_rows.is_empty() {
            if data.models.is_empty() {
                r##"<tr><td colspan="4">No catalog models are loaded.</td></tr>"##.to_owned()
            } else {
                capacity_window.map_or_else(
                    || r##"<tr><td colspan="4">All catalog models have at least one route advertising accepting capacity.</td></tr>"##.to_owned(),
                    |window| format!(r##"<tr><td colspan="4">{}</td></tr>"##, volatile_text("All catalog models have at least one route advertising accepting capacity.", window, "Capacity evidence expired; refresh to recompute supply exceptions.")),
                )
            }
        } else {
            shortage_rows
        }
    );
    let (status, tone) = if data.models.is_empty() {
        ("Catalog unavailable", "warn")
    } else if unavailable == 0 {
        ("Capacity advertised", "good")
    } else {
        ("Supply exceptions", "warn")
    };
    shell_wide(data, expires, DashboardAppPage::Network, "Explore", "Network health", "Current provider capacity, route availability, market conditions, and supporting evidence.", status, tone, "", &content)
}

fn network_models_page(data: &DashboardData, expires: u64, requested_page: Option<&str>) -> String {
    let page = PageWindow::from_query(data.models.len(), MAX_MODEL_ROWS, requested_page);
    let rows = model_rows(data, page);
    let model_summary = page.status("network models");
    let pagination = pagination_nav(
        page,
        "/mayhem/dashboard/network/models",
        &[],
        "network models",
    );
    let (filter_controls, filter_empty) = shown_rows_filter(
        "network-models",
        "network-models-table",
        "Filter the shown network models by name, capability, or availability",
        page.len(),
        "models",
    );
    let content = format!(
        r##"{}<section class="panel"><header class="panel-head"><div class="panel-title"><h2>Network models</h2><p>Catalog terms paired with fresh advertised route capacity.</p></div>{filter_controls}</header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table" id="network-models-table"><caption class="sr-only">Network models, advertised capacity, capabilities, and price</caption><thead><tr><th>Model</th><th>Advertised capacity</th><th>Capabilities</th><th>Catalog price</th><th>Action</th></tr></thead><tbody>{rows}</tbody></table></div>{filter_empty}</div><footer class="panel-footer"><span>{model_summary}</span>{pagination}</footer></section>"##,
        network_subnav(DashboardProductPage::NetworkModels),
    );
    shell_wide(
        data,
        expires,
        DashboardAppPage::Network,
        "Network analysis",
        "Models",
        "Every catalog model with its current advertised supply.",
        if data.models.is_empty() {
            "Catalog unavailable"
        } else {
            "Catalog loaded"
        },
        if data.models.is_empty() {
            "warn"
        } else {
            "good"
        },
        "",
        &content,
    )
}

fn network_providers_page(
    data: &DashboardData,
    expires: u64,
    requested_page: Option<&str>,
) -> String {
    let total_routes = data
        .models
        .iter()
        .map(|model| model.mayhem.route_candidates.len())
        .sum::<usize>();
    let page = PageWindow::from_query(total_routes, MAX_PROVIDER_ROWS, requested_page);
    let rows = network_provider_rows(data, page);
    let route_summary = page.status("catalog provider routes");
    let pagination = pagination_nav(
        page,
        "/mayhem/dashboard/network/providers",
        &[],
        "catalog provider routes",
    );
    let (filter_controls, filter_empty) = shown_rows_filter(
        "provider",
        "provider-table",
        "Filter the shown routes by provider, model, or state",
        page.len(),
        "routes",
    );
    let content = format!(
        r##"{}<section class="panel"><header class="panel-head"><div class="panel-title"><h2>Provider routes</h2><p>One row per catalog route on this page, enriched with current heartbeat state when present.</p></div>{filter_controls}</header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table" id="provider-table"><caption class="sr-only">Canonical provider routes and current operational evidence</caption><thead><tr><th>Provider / route</th><th>Model</th><th>State</th><th>Capacity</th><th>Performance</th><th>Evidence</th></tr></thead><tbody>{rows}</tbody></table></div>{filter_empty}</div><footer class="panel-footer"><span>{route_summary}</span>{pagination}<span>Raw identifiers are secondary to operational state.</span></footer></section>"##,
        network_subnav(DashboardProductPage::NetworkProviders),
    );
    shell_wide(
        data,
        expires,
        DashboardAppPage::Network,
        "Network analysis",
        "Providers",
        "Compare current advertised capacity, route state, and supporting evidence.",
        if data.accepting_routes() > 0 {
            "Capacity advertised"
        } else {
            "No advertised capacity"
        },
        if data.accepting_routes() > 0 {
            "good"
        } else {
            "warn"
        },
        "",
        &content,
    )
}

fn network_markets_page(
    data: &DashboardData,
    expires: u64,
    requested_page: Option<&str>,
) -> String {
    let total_markets = data
        .models
        .iter()
        .map(|model| model.mayhem.markets.len())
        .sum::<usize>();
    let page = PageWindow::from_query(total_markets, MAX_PROVIDER_ROWS, requested_page);
    let mut index = 0usize;
    let mut rows = String::new();
    for model in data.models.iter() {
        for market in model.mayhem.markets.iter() {
            if index >= page.end {
                break;
            }
            if index >= page.start {
                rows.push_str(&format!(r##"<tr data-filter-row data-filter-text="{} {} T{} {} {}"><td data-export-value="{} / enclave {}" data-sort-value="{} / {}"><span class="table-primary mono">{}</span><span class="table-secondary">{}</span></td><td>T{} &middot; {}</td><td>{} &middot; {}</td><td>{}</td><td>{}</td></tr>"##, html_escape(&model.id), html_escape(&market.enclave_id), market.att_tier, html_escape(&market.quant), html_escape(&market.availability), html_escape(&model.id), html_escape(&market.enclave_id), html_escape(&model.id), html_escape(&market.enclave_id), html_escape(&model.id), html_escape(short_text(&market.enclave_id, 18).as_ref()), market.att_tier, html_escape(&market.quant), count_noun(market.providers_online as u64, "provider"), count_noun(market.route_count as u64, "route"), html_escape(&market.availability), money_html(&dashboard_price(&market.price_ref_au))));
            }
            index += 1;
        }
        if index >= page.end {
            break;
        }
    }
    if rows.is_empty() {
        rows = format!(
            r##"<tr><td colspan="5">{}</td></tr>"##,
            empty_block(
                "No catalog markets",
                "No market records are loaded in this catalog snapshot.",
                None,
            )
        );
    }
    let market_summary = page.status("catalog markets");
    let pagination = pagination_nav(
        page,
        "/mayhem/dashboard/network/markets",
        &[],
        "catalog markets",
    );
    let (filter_controls, filter_empty) = shown_rows_filter(
        "market",
        "market-table",
        "Filter the shown markets by model, tier, quantization, or state",
        page.len(),
        "markets",
    );
    let content = format!(
        r##"{}<section class="panel"><header class="panel-head"><div class="panel-title"><h2>Catalog markets</h2><p>Market structure is contractual; current acceptance still belongs to heartbeat evidence.</p></div>{filter_controls}</header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table" id="market-table"><caption class="sr-only">Catalog markets and reference prices</caption><thead><tr><th>Model / enclave</th><th>Tier / quant</th><th>Catalog supply</th><th>Availability label</th><th>Reference price</th></tr></thead><tbody>{rows}</tbody></table></div>{filter_empty}</div><footer class="panel-footer"><span>{market_summary}</span>{pagination}</footer></section>"##,
        network_subnav(DashboardProductPage::NetworkMarkets),
    );
    shell_wide(
        data,
        expires,
        DashboardAppPage::Network,
        "Network analysis",
        "Markets",
        "How each catalog market is structured: tiers, supply, and reference prices.",
        "Catalog view",
        "",
        "",
        &content,
    )
}

fn network_activity_page(
    data: &DashboardData,
    expires: u64,
    requested_page: Option<&str>,
) -> String {
    let mut entries = data.entries.iter().collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.heartbeat_age_millis.unwrap_or(u64::MAX));
    let page = PageWindow::from_query(entries.len(), MAX_EVIDENCE_ROWS, requested_page);
    let activity_summary = page.status("route observations");
    let pagination = pagination_nav(
        page,
        "/mayhem/dashboard/network/activity",
        &[],
        "route observations",
    );
    let rows = entries.into_iter().skip(page.start).take(page.len()).map(|entry| {
        let state = route_operational_state(entry);
        let queue = heartbeat_value(
            data,
            entry,
            entry
                .heartbeat
                .as_ref()
                .map(|heartbeat| heartbeat.q.engine_backlog.to_string()),
        );
        let age = heartbeat_age(data, entry);
        let explanation = heartbeat_explanation(data, entry, &state.explanation);
        format!(r##"<tr data-filter-row><td data-export-value="{} / room {}"><span class="table-primary mono">{}</span><span class="table-secondary">room {}</span></td><td data-export-value="{}">{}</td><td><span class="status-badge {}">{}</span></td><td>{queue}</td><td><span class="table-primary">{age}</span><span class="table-secondary">{explanation}</span></td></tr>"##, html_escape(&entry.key.provider), html_escape(&entry.key.room_id), html_escape(short_text(&entry.key.provider, 16).as_ref()), html_escape(short_text(&entry.key.room_id, 14).as_ref()), html_escape(&entry.contract.model_id), html_escape(short_text(&entry.contract.model_id, 30).as_ref()), state.tone, html_escape(state.label))
    }).collect::<String>();
    let (filter_controls, filter_empty) = shown_rows_filter(
        "network-activity",
        "network-activity-table",
        "Filter the shown route observations by provider, room, model, or state",
        page.len(),
        "route observations",
    );
    let content = format!(
        r##"{}<div class="notice"><strong>Snapshot, not network history.</strong> The gateway does not persist a network-wide event feed, so this view orders route observations by freshness.</div><section class="panel section-gap"><header class="panel-head"><div class="panel-title"><h2>Route observations</h2><p>Ordered by heartbeat freshness</p></div>{filter_controls}</header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table" id="network-activity-table"><caption class="sr-only">Provider route observations ordered by heartbeat freshness</caption><thead><tr><th>Provider / room</th><th>Model</th><th>State</th><th>Queue</th><th>Freshness</th></tr></thead><tbody>{}</tbody></table></div>{filter_empty}</div><footer class="panel-footer"><span>{activity_summary}</span>{pagination}</footer></section>"##,
        network_subnav(DashboardProductPage::NetworkActivity),
        if rows.is_empty() {
            r##"<tr><td colspan="5">No provider-table entries are available.</td></tr>"##.to_owned()
        } else {
            rows
        }
    );
    shell_wide(
        data,
        expires,
        DashboardAppPage::Network,
        "Network analysis",
        "Route status",
        "Every observed route, ordered by heartbeat freshness.",
        "Snapshot",
        "",
        "",
        &content,
    )
}

fn network_evidence_page(
    data: &DashboardData,
    expires: u64,
    requested_page: Option<&str>,
    requested_probe_page: Option<&str>,
) -> String {
    let route_page = PageWindow::from_query(data.entries.len(), MAX_EVIDENCE_ROWS, requested_page);
    let route_summary = route_page.status("route entries");
    let route_pagination = match requested_probe_page.filter(|value| !value.is_empty()) {
        Some(probe_page) => pagination_nav(
            route_page,
            "/mayhem/dashboard/network/evidence",
            &[("probe_page", probe_page)],
            "route entries",
        ),
        None => pagination_nav(
            route_page,
            "/mayhem/dashboard/network/evidence",
            &[],
            "route entries",
        ),
    };
    let route_rows = data.entries.iter().skip(route_page.start).take(route_page.len()).map(|entry| {
        let state = route_operational_state(entry);
        let evidence = evidence_link(&evidence_href("route", &[("provider", entry.key.provider.as_str()), ("enclave", entry.key.enclave_id.as_str()), ("room", entry.key.room_id.as_str()), ("model", entry.contract.model_id.as_str())]), "Verify", &entry.contract.model_id);
        format!(r##"<tr data-filter-row><td data-export-value="{} / enclave {}"><span class="table-primary mono">{}</span><span class="table-secondary">{}</span></td><td data-export-value="{}">{}</td><td><span class="status-badge {}">{}</span></td><td>{}</td><td>{}</td></tr>"##, html_escape(&entry.key.provider), html_escape(&entry.key.enclave_id), html_escape(short_text(&entry.key.provider, 16).as_ref()), html_escape(short_text(&entry.key.enclave_id, 18).as_ref()), html_escape(&entry.contract.model_id), html_escape(&entry.contract.model_id), state.tone, html_escape(state.label), if entry.attestation_head.is_some() { "Attestation cached" } else { "No cached attestation" }, evidence)
    }).collect::<String>();
    let (route_filter_controls, route_filter_empty) = shown_rows_filter(
        "network-evidence",
        "network-evidence-table",
        "Filter the shown route evidence by provider, enclave, model, or state",
        route_page.len(),
        "route entries",
    );
    let probe_panel = if data.probes.is_empty() {
        String::new()
    } else {
        let probe_page =
            PageWindow::from_query(data.probes.len(), MAX_EVIDENCE_ROWS, requested_probe_page);
        let probe_summary = probe_page.status("probe events");
        let probe_pagination = match requested_page.filter(|value| !value.is_empty()) {
            Some(route_page) => pagination_nav_with_param(
                probe_page,
                "/mayhem/dashboard/network/evidence",
                &[("page", route_page)],
                "probe events",
                "probe_page",
            ),
            None => pagination_nav_with_param(
                probe_page,
                "/mayhem/dashboard/network/evidence",
                &[],
                "probe events",
                "probe_page",
            ),
        };
        let (probe_filter_controls, probe_filter_empty) = shown_rows_filter_scoped(
            "evidence-probes",
            "evidence-probes-table",
            "Filter verification probes on the shown page by probe, enclave, model, provider, method, or result",
            probe_page.len(),
            "probe events",
            Some("probe"),
        );
        let probe_rows = data
            .probes
            .iter()
            .skip(probe_page.start)
            .take(probe_page.len())
            .map(|probe| {
                let evidence = evidence_link(
                    &evidence_href("probe", &[("id", probe.probe_id.as_str())]),
                    "Verify",
                    &probe.probe_id,
                );
                let (result, tone) = if probe.pass {
                    ("Passed", "good")
                } else {
                    ("Failed", "danger")
                };
                format!(
                    r##"<tr data-filter-row data-filter-text="{} {} {} {} {} {result}"><td data-export-value="{} / enclave {}" data-sort-value="{} / {}"><span class="table-primary mono">{}</span><span class="table-secondary">{}</span></td><td data-export-value="{} / provider {}" data-sort-value="{} / {}"><span class="table-primary">{}</span><span class="table-secondary">provider {}</span></td><td data-export-value="{}">{}</td><td data-export-value="{result} / {:.2}% match" data-sort-value="{}"><span class="status-badge {tone}">{result}</span><span class="table-secondary">{:.2}% match</span></td><td>{}</td></tr>"##,
                    html_escape(&probe.probe_id),
                    html_escape(&probe.enclave_id),
                    html_escape(&probe.model_id),
                    html_escape(&probe.provider),
                    html_escape(&probe.verification_method),
                    html_escape(&probe.probe_id),
                    html_escape(&probe.enclave_id),
                    html_escape(&probe.probe_id),
                    html_escape(&probe.enclave_id),
                    html_escape(short_text(&probe.probe_id, 18).as_ref()),
                    html_escape(short_text(&probe.enclave_id, 18).as_ref()),
                    html_escape(&probe.model_id),
                    html_escape(&probe.provider),
                    html_escape(&probe.model_id),
                    html_escape(&probe.provider),
                    html_escape(short_text(&probe.model_id, 30).as_ref()),
                    html_escape(short_text(&probe.provider, 16).as_ref()),
                    html_escape(&probe.verification_method),
                    html_escape(&probe.verification_method),
                    f64::from(probe.match_bps) / 100.0,
                    probe.match_bps,
                    f64::from(probe.match_bps) / 100.0,
                    evidence,
                )
            })
            .collect::<String>();
        format!(
            r##"<section class="panel section-gap"><header class="panel-head"><div class="panel-title"><h2>Verification probes</h2><p>Recorded canary checks with their reported method and result.</p></div>{probe_filter_controls}</header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table" id="evidence-probes-table"><caption class="sr-only">Verification probe evidence</caption><thead><tr><th>Probe / enclave</th><th>Model / provider</th><th>Method</th><th>Result</th><th>Evidence</th></tr></thead><tbody>{probe_rows}</tbody></table></div>{probe_filter_empty}</div><footer class="panel-footer"><span>{probe_summary} Verify opens the exact recorded event.</span>{probe_pagination}</footer></section>"##,
        )
    };
    let content = format!(
        r##"{}<section class="panel"><header class="panel-head"><div class="panel-title"><h2>Route evidence</h2><p>Structured facts and the raw gateway snapshot are loaded only when requested.</p></div>{route_filter_controls}</header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table" id="network-evidence-table"><caption class="sr-only">Provider route evidence</caption><thead><tr><th>Provider / enclave</th><th>Model</th><th>State</th><th>Attestation</th><th>Evidence</th></tr></thead><tbody>{}</tbody></table></div>{route_filter_empty}</div><footer class="panel-footer"><span>{route_summary}</span>{route_pagination}</footer></section>{probe_panel}"##,
        network_subnav(DashboardProductPage::NetworkEvidence),
        if route_rows.is_empty() {
            format!(
                r##"<tr><td colspan="5">{}</td></tr>"##,
                empty_block(
                    "No route evidence in this snapshot",
                    "The gateway provider table contains no route records, so this view does not infer or invent evidence.",
                    None,
                )
            )
        } else {
            route_rows
        },
    );
    shell_wide(
        data,
        expires,
        DashboardAppPage::Network,
        "Network analysis",
        "Evidence",
        "The recorded route facts and verification probes behind every Verify button.",
        "Evidence snapshot",
        "",
        "",
        &content,
    )
}

fn help_page(data: &DashboardData, expires: u64) -> String {
    let provider_start = if data.local_provider_id.is_some() {
        r##"<a class="soft-button" href="/mayhem/dashboard/earn">Open Provider overview</a>"##
    } else {
        r##"<a class="soft-button" href="/mayhem/dashboard/earn">Start provider setup</a>"##
    };
    let content = format!(
        r##"<section class="dashboard-layout help-layout"><div class="stack"><section class="panel"><header class="panel-head"><div class="panel-title"><h2>Get started</h2><p>Choose the path that matches what you want to do.</p></div></header><div class="panel-body"><div class="checklist help-paths"><div class="check-step active"><span class="check-mark" aria-hidden="true">1</span><div class="check-copy"><strong>Use AI</strong><span>Choose a model, send a request, and review its result and receipt.</span><a class="soft-button" href="/mayhem/dashboard/playground">Open Playground</a></div></div><div class="check-step"><span class="check-mark" aria-hidden="true">2</span><div class="check-copy"><strong>Connect an app</strong><span>Copy the Mayhem API address and add it to an OpenAI-compatible application.</span><a class="soft-button" href="/mayhem/dashboard/connect">Open Integrations</a></div></div><div class="check-step"><span class="check-mark" aria-hidden="true">3</span><div class="check-copy"><strong>Provide compute</strong><span>Set up this machine, monitor its serving routes, and review earnings.</span>{provider_start}</div></div></div></div></section><section class="panel"><header class="panel-head"><div class="panel-title"><h2>Common problems</h2><p>Start with the symptom you can see.</p></div></header><div class="help-problems"><details class="help-problem"><summary><span>No models are available</span><small>Check catalog and provider availability</small></summary><div><p>A usable model needs both a catalog entry and a fresh provider advertising capacity. Review availability before retrying.</p><a class="soft-button" href="/mayhem/dashboard/models">Review models</a></div></details><details class="help-problem"><summary><span>A request failed or remains open</span><small>Inspect its latest recorded state</small></summary><div><p>Activity shows the final receipt or latest open record. An open record does not prove that execution is still running.</p><a class="soft-button" href="/mayhem/dashboard/activity">Open Activity</a></div></details><details class="help-problem"><summary><span>I need to add funds</span><small>Check the ledger balance and deposit flow</small></summary><div><p>Billing shows the last-known ledger balance and the commands needed to start or confirm a deposit.</p><a class="soft-button" href="/mayhem/dashboard/wallet">Open Billing</a></div></details><details class="help-problem"><summary><span>My API key is rejected</span><small>Confirm whether authentication is required</small></summary><div><p>Integrations shows the current authentication requirement and explains how to create and use a Mayhem access token.</p><a class="soft-button" href="/mayhem/dashboard/connect#access-tokens">Review access tokens</a></div></details><details class="help-problem"><summary><span>My provider receives no work</span><small>Check heartbeat, capacity, and route state</small></summary><div><p>A configured provider also needs a fresh heartbeat, free slots, and an accepting serving route.</p><a class="soft-button" href="/mayhem/dashboard/earn/machines">Inspect serving routes</a></div></details></div></section><section class="panel"><header class="panel-head"><div class="panel-title"><h2>What dashboard data means</h2><p>Every label has a source and a limit.</p></div></header><div class="panel-body flush"><div class="data-table-wrap"><table class="data-table help-meaning-table"><caption class="sr-only">Dashboard data sources, meanings, and limitations</caption><thead><tr><th>Source</th><th>What it tells you</th><th>What it does not guarantee</th></tr></thead><tbody><tr><th scope="row">Catalog</th><td>Model capabilities and reference terms</td><td>A final price or an available provider</td></tr><tr><th scope="row">Heartbeat</th><td>A provider recently advertised its state and capacity</td><td>That your next request will route</td></tr><tr><th scope="row">Receipt</th><td>The gateway recorded a request and its metering state</td><td>That payment settlement is complete</td></tr><tr><th scope="row">Ledger</th><td>The last-known balance, earnings, holds, and payouts</td><td>That the values remain current after freshness expires</td></tr></tbody></table></div></div><footer class="panel-footer"><span>Refresh expired live data and verify important claims at their source.</span><a href="/mayhem/dashboard/network/evidence">Open network evidence</a></footer></section></div><aside class="stack help-reference"><details class="panel disclosure-panel" open><summary>Essential terms</summary><div class="panel-body"><div class="checklist help-terms"><div class="check-step"><span class="check-mark" aria-hidden="true">A</span><div class="check-copy"><strong>Advertised capacity</strong><span>A provider recently reported that it is accepting work and has free capacity. Routing still checks price, policy, capabilities, reputation, and verification requirements.</span></div></div><div class="check-step"><span class="check-mark" aria-hidden="true">R</span><div class="check-copy"><strong>Final receipt</strong><span>The gateway finished recording the request's usage and outcome. Payment settlement is a separate state.</span></div></div><div class="check-step"><span class="check-mark" aria-hidden="true">E</span><div class="check-copy"><strong>Evidence</strong><span>The exact structured facts and raw gateway snapshot supporting a dashboard claim.</span></div></div></div></div></details><details class="panel disclosure-panel"><summary>Advanced verification</summary><div class="panel-body help-disclosure-copy"><h3>Attestation is a compatibility check</h3><p>A higher tier number does not automatically include every protection from a lower tier. Check the route evidence for the exact property your application requires.</p><a class="soft-button" href="/mayhem/dashboard/network/evidence">Review network evidence</a></div></details><details class="panel disclosure-panel"><summary>Privacy and exports</summary><div class="panel-body help-disclosure-copy"><p>Hide amounts masks dashboard text and redacts money cells from new shown-page CSV exports. It does not change gateway data, transactions, or files you already saved.</p><a class="soft-button" href="/mayhem/dashboard/settings">Review privacy settings</a></div></details></aside></section>"##,
    );
    shell(
        data,
        expires,
        DashboardAppPage::Help,
        "Support",
        "Help",
        "Start using Mayhem, connect an app, provide compute, or troubleshoot a request.",
        "Guidance",
        "",
        "",
        &content,
    )
}

fn settings_page(data: &DashboardData, expires: u64) -> String {
    let (version_status, version_tone) = match data.update_notice.as_ref() {
        Some(notice) if notice.level == "required" => ("Update required", "danger"),
        Some(_) => ("Update available", "warn"),
        None => ("Up to date", "good"),
    };
    let update = data
        .update_notice
        .as_ref()
        .map(|notice| {
            format!(
                "Installed {} / catalog minimum {} / {} affected",
                notice.installed_app_version,
                notice.required_min_app_version,
                notice.affected_model_count
            )
        })
        .unwrap_or_else(|| {
            format!(
                "Mayhem {} · compatible with the current catalog",
                installed_app_version()
            )
        });
    let update_resolution = if data.update_notice.is_some() {
        r##"<div class="field"><span class="field-label">1. Verify and stage on the gateway host</span><pre class="code-block"><code id="mayhem-update-stage-command">mayhem update</code><button class="quiet-button copy-corner js-only" type="button" data-copy data-copy-target="#mayhem-update-stage-command" aria-label="Copy Mayhem update staging command"><span data-copy-label>Copy</span></button></pre><p class="result-summary">Downloads and verifies the release, then stages it. This does not replace the installed binary.</p></div><div class="field-gap" aria-hidden="true"></div><div class="field"><span class="field-label">2. Apply the staged update</span><pre class="code-block"><code id="mayhem-update-apply-command">mayhem update --apply-staged</code><button class="quiet-button copy-corner js-only" type="button" data-copy data-copy-target="#mayhem-update-apply-command" aria-label="Copy Mayhem staged-update apply command"><span data-copy-label>Copy</span></button></pre><p class="result-summary">Run after the required staging delay. Applying replaces the host binary and runs its health check, but it does not restart an already-running gateway service; restart that service, then reload this page.</p></div>"##
    } else {
        ""
    };
    let content = format!(
        r##"<section class="dashboard-layout"><div class="stack"><section class="panel"><header class="panel-head"><div class="panel-title"><h2>Display and attention</h2><p>Preferences are stored in this browser profile.</p></div></header><noscript><div class="panel-body"><p class="notice warn">Display preferences require JavaScript. Gateway session and version facts remain available.</p></div></noscript><div class="settings-list"><div class="settings-row"><div class="settings-copy"><strong>Hide money amounts</strong><span>Replaces monetary text semantically and visually on dashboard pages.</span></div><button class="settings-control soft-button js-only" type="button" data-preference="amounts" role="switch" aria-label="Hide money amounts" aria-checked="false"><span class="switch-track" aria-hidden="true"></span><span data-preference-label>Off</span></button></div><div class="settings-row"><div class="settings-copy"><strong>Reduce motion</strong><span>Disables transitions and animated feedback while preserving state.</span></div><button class="settings-control soft-button js-only" type="button" data-preference="motion" role="switch" aria-label="Reduce motion" aria-checked="false"><span class="switch-track" aria-hidden="true"></span><span data-preference-label>Off</span></button></div><div class="settings-row"><div class="settings-copy"><strong>Compact density</strong><span>Reduces spacing for professional monitoring without hiding explanations.</span></div><button class="settings-control soft-button js-only" type="button" data-preference="density" role="switch" aria-label="Compact density" aria-checked="false"><span class="switch-track" aria-hidden="true"></span><span data-preference-label>Off</span></button></div><div class="settings-row"><div class="settings-copy"><strong>Playground conversation history</strong><span>Prompts and responses are stored only in this browser profile. Credentials are never saved.</span></div><button class="settings-control quiet-button js-only" type="button" data-clear-playground-history>Clear history</button></div></div><footer class="panel-footer"><button class="quiet-button js-only" type="button" data-clear-preferences>Reset preferences</button></footer></section><details class="panel disclosure-panel"><summary><span>Local launch diagnostics</span><span class="status-badge"><span data-local-event-count>0</span>&nbsp;events</span></summary><div class="panel-body"><p class="notice">Optional debugging log for launch issues. Events stay in this browser and never include prompts, responses, credentials, or money amounts.</p><div class="inline-actions section-gap"><button class="soft-button js-only" type="button" data-export-local-events>Export JSON</button><button class="quiet-button js-only" type="button" data-clear-local-events>Clear diagnostics</button></div></div></details></div><aside class="stack"><section class="panel"><header class="panel-head"><div class="panel-title"><h2>Gateway session</h2><p>Access and runtime facts for this gateway</p></div></header><div class="panel-body"><div class="fact-grid"><div class="fact"><span>Authentication</span><strong>{}</strong></div><div class="fact"><span>Active tokens</span><strong>{}</strong></div><div class="fact"><span>Receipt rail</span><strong>{}</strong></div><div class="fact"><span>Provider identity</span><strong>{}</strong></div></div></div></section><section class="panel"><header class="panel-head"><div class="panel-title"><h2>Version</h2><p>From the installed app and catalog requirements</p></div></header><div class="panel-body"><p class="notice {}">{}</p>{update_resolution}</div></section></aside></section>"##,
        if data.requires_auth() {
            "Required"
        } else {
            "Optional"
        },
        data.active_token_count(),
        html_escape(&data.rail.to_ascii_uppercase()),
        data.local_provider_id
            .as_deref()
            .map(|id| html_escape(short_text(id, 20).as_ref()))
            .unwrap_or_else(|| "Not configured".to_owned()),
        version_tone,
        html_escape(&update)
    );
    shell(
        data,
        expires,
        DashboardAppPage::Settings,
        "Application",
        "Settings",
        "Display, motion, and money-visibility preferences, plus this gateway session's facts.",
        version_status,
        version_tone,
        "",
        &content,
    )
}

#[derive(Clone, Debug)]
struct ModelAvailability {
    label: &'static str,
    tone: &'static str,
    explanation: String,
}

fn model_availability(data: &DashboardData, model: &GatewayModel) -> ModelAvailability {
    let mut accepting = 0usize;
    let mut fresh = 0usize;
    let mut capacity = 0usize;
    let mut draining = 0usize;
    for candidate in &model.mayhem.route_candidates {
        let Some(entry) = dashboard_entry_for_route(&data.entries, candidate) else {
            continue;
        };
        match route_operational_state(entry).kind {
            RouteStateKind::Accepting => {
                accepting += 1;
                fresh += 1;
            }
            RouteStateKind::Mixed | RouteStateKind::Blocked | RouteStateKind::Failed => {}
            RouteStateKind::Capacity => {
                capacity += 1;
                fresh += 1;
            }
            RouteStateKind::Draining => {
                draining += 1;
                fresh += 1;
            }
            RouteStateKind::Stale | RouteStateKind::Waiting => {}
        }
    }
    if accepting > 0 {
        ModelAvailability {
            label: "Capacity advertised",
            tone: "good",
            explanation: format!(
                "{} accepting work; {fresh} fresh",
                count_noun(accepting as u64, "route")
            ),
        }
    } else if capacity > 0 {
        ModelAvailability {
            label: "At capacity",
            tone: "warn",
            explanation: format!(
                "{} with no free slot right now",
                count_noun(capacity as u64, "fresh route")
            ),
        }
    } else if draining > 0 {
        ModelAvailability {
            label: "Draining",
            tone: "warn",
            explanation: format!(
                "{} not accepting new work",
                count_noun(draining as u64, "fresh route")
            ),
        }
    } else if !model.mayhem.route_candidates.is_empty() {
        ModelAvailability {
            label: "Telemetry unavailable",
            tone: "warn",
            explanation: format!(
                "{}; no fresh heartbeat",
                count_noun(
                    model.mayhem.route_candidates.len() as u64,
                    "canonical route"
                )
            ),
        }
    } else {
        ModelAvailability {
            label: "No provider route",
            tone: "",
            explanation: "No canonical route in the current catalog".to_owned(),
        }
    }
}

#[derive(Default)]
struct ProviderSlotTotals {
    active: u64,
    max: u64,
    free: u64,
    backlog: u64,
    fresh_routes: usize,
    total_routes: usize,
}

fn provider_slot_totals(
    data: &DashboardData,
    entries: &[&ProviderTableEntry],
) -> ProviderSlotTotals {
    entries.iter().fold(
        ProviderSlotTotals {
            total_routes: entries.len(),
            ..ProviderSlotTotals::default()
        },
        |mut totals, entry| {
            let Some(heartbeat) =
                heartbeat_freshness_window(data, entry).and(entry.heartbeat.as_ref())
            else {
                return totals;
            };
            totals.fresh_routes += 1;
            totals.active = totals
                .active
                .saturating_add(u64::from(heartbeat.slots.active));
            totals.max = totals.max.saturating_add(u64::from(heartbeat.slots.max));
            totals.free = totals
                .free
                .saturating_add(u64::from(heartbeat.q.free_slots));
            totals.backlog = totals
                .backlog
                .saturating_add(u64::from(heartbeat.q.engine_backlog));
            totals
        },
    )
}

fn provider_freshness_window(
    data: &DashboardData,
    entries: &[&ProviderTableEntry],
) -> Option<FreshnessWindow> {
    earliest_freshness_window(data, entries.iter().copied())
}

fn provider_current_value(
    value: String,
    totals: &ProviderSlotTotals,
    window: Option<FreshnessWindow>,
) -> String {
    match (totals.fresh_routes, window) {
        (0, _) | (_, None) => "Unavailable".to_owned(),
        (_, Some(window)) => volatile_text(&value, window, "Unavailable"),
    }
}

fn heartbeat_value(
    data: &DashboardData,
    entry: &ProviderTableEntry,
    value: Option<String>,
) -> String {
    match (value, heartbeat_freshness_window(data, entry)) {
        (Some(value), Some(window)) => volatile_text(&value, window, "Unavailable"),
        _ => "Unavailable".to_owned(),
    }
}

fn heartbeat_explanation(
    data: &DashboardData,
    entry: &ProviderTableEntry,
    explanation: &str,
) -> String {
    heartbeat_freshness_window(data, entry).map_or_else(
        || html_escape(explanation),
        |window| volatile_text(explanation, window, "Heartbeat evidence expired; refresh"),
    )
}

fn heartbeat_age(data: &DashboardData, entry: &ProviderTableEntry) -> String {
    heartbeat_freshness_window(data, entry).map_or_else(
        || {
            if entry.heartbeat.is_some() || entry.heartbeat_age_millis.is_some() {
                "Expired; refresh".to_owned()
            } else {
                "Never received".to_owned()
            }
        },
        |window| volatile_age(window, "Expired; refresh"),
    )
}

#[cfg(test)]
fn provider_metric_basis(totals: &ProviderSlotTotals) -> String {
    match (totals.fresh_routes, totals.total_routes) {
        (0, 0) => "No configured route evidence".to_owned(),
        (0, total) => format!(
            "No fresh heartbeat across {}",
            count_noun(total as u64, "configured route")
        ),
        (fresh, total) if fresh < total => {
            format!("Fresh heartbeat totals from {fresh} of {total} configured routes")
        }
        (fresh, _) => format!(
            "Fresh heartbeat totals from {}",
            count_noun(fresh as u64, "configured route")
        ),
    }
}

fn provider_coverage_notice(
    totals: &ProviderSlotTotals,
    window: Option<FreshnessWindow>,
) -> String {
    match (totals.fresh_routes, totals.total_routes, window) {
        (_, 0, _) => String::new(),
        (0, total, _) => format!(
            r#"<div class="notice warn"><strong>No fresh heartbeat.</strong> {} without a heartbeat in the current freshness window — this usually means the worker is offline. Refresh after it reconnects.</div>"#,
            count_noun(total as u64, "configured route"),
        ),
        (fresh, total, Some(window)) if fresh < total => format!(
            r#"<div class="notice warn"><strong>Partial coverage.</strong> Slot and queue totals cover {fresh} of {total} configured routes; {} excluded. Oldest included heartbeat: {}.</div>"#,
            count_noun(total.saturating_sub(fresh) as u64, "delayed route"),
            relative_time(window),
        ),
        (fresh, _, Some(window)) => {
            if fresh == 1 {
                format!(
                    r#"<p class="result-summary">The configured route has a fresh heartbeat; latest: {}.</p>"#,
                    relative_time(window),
                )
            } else {
                format!(
                    r#"<p class="result-summary">All {fresh} configured routes have fresh heartbeats; oldest: {}.</p>"#,
                    relative_time(window),
                )
            }
        }
        _ => String::new(),
    }
}

fn aggregate_provider_state(entries: &[&ProviderTableEntry]) -> RouteState {
    if entries.is_empty() {
        return RouteState {
            kind: RouteStateKind::Waiting,
            label: "Setup incomplete",
            tone: "warn",
            explanation:
                "No catalog route matches the provider identity configured on this gateway."
                    .to_owned(),
        };
    }
    let states = entries
        .iter()
        .map(|entry| route_operational_state(entry))
        .collect::<Vec<_>>();
    let accepting = states
        .iter()
        .filter(|state| state.kind == RouteStateKind::Accepting)
        .count();
    if accepting == states.len() {
        return RouteState {
            kind: RouteStateKind::Accepting,
            label: "Online and accepting work",
            tone: "good",
            explanation: if accepting == 1 {
                "The configured route is advertising free capacity.".to_owned()
            } else {
                format!("All {accepting} routes advertise free capacity.")
            },
        };
    }
    if accepting > 0 {
        return RouteState {
            kind: RouteStateKind::Mixed,
            label: "Online with route issues",
            tone: "warn",
            explanation: format!(
                "{accepting} of {} routes accept work; attention needed on {}.",
                states.len(),
                count_noun(states.len().saturating_sub(accepting) as u64, "route")
            ),
        };
    }
    let capacity_count = states
        .iter()
        .filter(|state| state.kind == RouteStateKind::Capacity)
        .count();
    let draining_count = states
        .iter()
        .filter(|state| state.kind == RouteStateKind::Draining)
        .count();
    let stale_count = states
        .iter()
        .filter(|state| state.kind == RouteStateKind::Stale)
        .count();
    let waiting_count = states
        .iter()
        .filter(|state| state.kind == RouteStateKind::Waiting)
        .count();
    let distinct_non_accepting = [capacity_count, draining_count, stale_count, waiting_count]
        .into_iter()
        .filter(|count| *count > 0)
        .count();
    if distinct_non_accepting > 1 {
        return RouteState {
            kind: RouteStateKind::Mixed,
            label: "Multiple route issues",
            tone: "warn",
            explanation: format!(
                "No route advertises free accepting capacity: {capacity_count} at capacity, {draining_count} draining, {stale_count} stale, {waiting_count} waiting."
            ),
        };
    }
    if states
        .iter()
        .any(|state| state.kind == RouteStateKind::Capacity)
    {
        return RouteState {
            kind: RouteStateKind::Capacity,
            label: "At capacity",
            tone: "warn",
            explanation:
                "Fresh telemetry is present, but every accepting route reports no free slot."
                    .to_owned(),
        };
    }
    if states
        .iter()
        .any(|state| state.kind == RouteStateKind::Draining)
    {
        return RouteState {
            kind: RouteStateKind::Draining,
            label: "Draining",
            tone: "warn",
            explanation: "Fresh telemetry is present, but routes are not accepting new work."
                .to_owned(),
        };
    }
    if states
        .iter()
        .any(|state| state.kind == RouteStateKind::Stale)
    {
        return RouteState { kind: RouteStateKind::Stale, label: "Telemetry delayed", tone: "warn", explanation: "The last heartbeat is outside the freshness window, so the machine is not presented as online.".to_owned() };
    }
    RouteState {
        kind: RouteStateKind::Waiting,
        label: "Waiting for first heartbeat",
        tone: "warn",
        explanation: "Routes exist, but this gateway has not received a fresh heartbeat for them."
            .to_owned(),
    }
}

fn provider_page_state(
    data: &DashboardData,
    _requested: Option<&str>,
    entries: &[&ProviderTableEntry],
) -> RouteState {
    if data.local_provider_id.is_none() {
        return RouteState {
            kind: RouteStateKind::Waiting,
            label: "Provider identity not configured",
            tone: "warn",
            explanation: "Earn cannot associate routes, capacity, or earnings with this gateway until a provider identity is configured.".to_owned(),
        };
    }
    if data.update_notice.as_ref().is_some_and(|notice| {
        notice.level == "required"
            && data.local_provider_id.is_some()
            && notice.models.iter().any(|affected| {
                entries
                    .iter()
                    .any(|entry| entry.contract.model_id == affected.model_id)
                    || data.provider_load_progress.values().any(|progress| {
                        data.local_provider_id.as_deref() == Some(progress.provider.as_str())
                            && progress.model_id == affected.model_id
                            && progress_is_fresh(progress)
                    })
            })
    }) {
        return RouteState {
            kind: RouteStateKind::Blocked,
            label: "Blocked by update",
            tone: "danger",
            explanation: "Catalog compatibility blocks one or more routes until Mayhem is updated. Existing work is not described as safe or stopped without runtime evidence.".to_owned(),
        };
    }
    if let Some(progress) = data
        .local_provider_id
        .as_deref()
        .and_then(|provider| latest_provider_progress(data, provider))
    {
        if progress_is_failed(progress) {
            let live_routes = entries
                .iter()
                .filter(|entry| entry.heartbeat.is_some())
                .count();
            let accepting_routes = entries
                .iter()
                .filter(|entry| route_operational_state(entry).kind == RouteStateKind::Accepting)
                .count();
            return provider_load_failure_state(progress, live_routes, accepting_routes);
        }
        if entries.is_empty() && progress_is_terminal(progress) {
            return RouteState {
                kind: RouteStateKind::Waiting,
                label: "Prepared; waiting for heartbeat",
                tone: "warn",
                explanation: "Worker preparation completed, but route publication and a fresh heartbeat are still required.".to_owned(),
            };
        }
        if entries.is_empty() {
            let phase = if progress.phase.is_empty() {
                "Preparation"
            } else {
                progress.phase.as_str()
            };
            // Sentence-case the reported phase so the page summary reads as prose.
            let phase = {
                let mut chars = phase.chars();
                chars.next().map_or_else(String::new, |first| {
                    first.to_uppercase().collect::<String>() + chars.as_str()
                })
            };
            return RouteState {
                kind: RouteStateKind::Waiting,
                label: "Preparing a model",
                tone: "",
                explanation: provider_progress_percent(progress).map_or_else(
                    || format!("{phase} progress is not reported; route publication and a fresh heartbeat are still pending."),
                    |percent| format!("{phase} is {percent}% complete; route publication and a fresh heartbeat are still pending."),
                ),
            };
        }
    }
    aggregate_provider_state(entries)
}

fn latest_provider_progress<'a>(
    data: &'a DashboardData,
    provider: &str,
) -> Option<&'a DashboardProviderLoadProgress> {
    data.provider_load_progress
        .values()
        .filter(|progress| progress.provider == provider && progress_is_fresh(progress))
        .max_by_key(|progress| progress.updated_at_ms)
}

fn progress_is_fresh(progress: &DashboardProviderLoadProgress) -> bool {
    progress.updated_at_ms > 0
        && now_millis_u64().saturating_sub(progress.updated_at_ms)
            <= DASHBOARD_PROVIDER_PROGRESS_ONLY_TTL_MS
}

fn progress_is_terminal(progress: &DashboardProviderLoadProgress) -> bool {
    matches!(
        progress.status.to_ascii_lowercase().as_str(),
        "complete" | "serving" | "joined"
    )
}

fn progress_is_failed(progress: &DashboardProviderLoadProgress) -> bool {
    progress.status.eq_ignore_ascii_case("error") || progress.status.eq_ignore_ascii_case("failed")
}

fn provider_load_failure_state(
    progress: &DashboardProviderLoadProgress,
    live_routes: usize,
    accepting_routes: usize,
) -> RouteState {
    let stage = if progress.label.is_empty() {
        if progress.phase.is_empty() {
            "Preparation"
        } else {
            progress.phase.as_str()
        }
    } else {
        progress.label.as_str()
    };
    let model = if progress.model_id.is_empty() {
        "Unknown model"
    } else {
        progress.model_id.as_str()
    };
    let reason = progress
        .error
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("No failure reason reported");
    // Pair the verbatim error with a plain-language next step so the page
    // never ends at a raw protocol string.
    let advice = if reason.contains("signature") || reason.contains("checksum") {
        "The downloaded model files don't match their signed checksum — re-run preparation to re-download, or choose another model."
    } else if reason.contains("space") || reason.contains("disk") {
        "The machine ran out of space — free up disk, then re-run preparation."
    } else {
        "Re-run preparation to retry, or choose another model."
    };
    let failure = format!("{model}: {stage} failed: {reason}. {advice}");
    if live_routes > 0 {
        let (label, route_summary) = if accepting_routes > 0 {
            (
                "Online with preparation issue",
                format!(
                    "{} still advertising capacity",
                    count_noun(accepting_routes as u64, "route")
                ),
            )
        } else {
            (
                "Live routes with preparation issue",
                format!(
                    "{} still reporting fresh telemetry",
                    count_noun(live_routes as u64, "route")
                ),
            )
        };
        RouteState {
            kind: RouteStateKind::Mixed,
            label,
            tone: "warn",
            explanation: format!("{route_summary}; {failure}"),
        }
    } else {
        RouteState {
            kind: RouteStateKind::Failed,
            label: "Setup blocked by model failure",
            tone: "danger",
            explanation: failure,
        }
    }
}

fn provider_progress_percent(progress: &DashboardProviderLoadProgress) -> Option<u64> {
    progress
        .percent
        .map(|percent| percent.min(100))
        .or_else(|| match (progress.position, progress.total) {
            (Some(position), Some(total)) if total > 0 => {
                Some((position.saturating_mul(100) / total).min(100))
            }
            _ => None,
        })
}

fn provider_progress_notice(data: &DashboardData, terminal_action: Option<(&str, &str)>) -> String {
    let Some(provider) = data.local_provider_id.as_deref() else {
        return String::new();
    };
    let Some(progress) = latest_provider_progress(data, provider) else {
        return String::new();
    };
    let Some(window) = progress_freshness_window(progress) else {
        return String::new();
    };
    if progress_is_terminal(progress) {
        if data.provider_entries(None).is_empty() {
            return volatile_attention(
                "warn",
                "!",
                "Model prepared; route not confirmed",
                "Preparation completed, but route publication and a fresh heartbeat are still pending.",
                terminal_action,
                window,
                "Preparation snapshot expired",
            );
        }
        return String::new();
    }
    let phase = if progress.phase.is_empty() {
        "Preparation"
    } else {
        progress.phase.as_str()
    };
    if progress_is_failed(progress) {
        let stage = if progress.label.is_empty() {
            phase
        } else {
            progress.label.as_str()
        };
        let reason = progress
            .error
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("No failure reason reported");
        let model = if progress.model_id.is_empty() {
            "Unknown model"
        } else {
            progress.model_id.as_str()
        };
        return volatile_attention(
            "danger",
            "!",
            "Model preparation failed",
            &format!("{model}: {stage} ({phase}) failed: {reason}"),
            None,
            window,
            "Preparation snapshot expired",
        );
    }
    let (progress_text, progress_markup) = match provider_progress_percent(progress) {
        Some(percent) => (
            format!("{percent}%"),
            format!(
                r##"<progress max="100" value="{percent}" aria-label="{} progress">{percent}%</progress>"##,
                html_escape(phase)
            ),
        ),
        None => (
            "progress not reported".to_owned(),
            format!(
                r##"<progress max="100" aria-label="{} progress not reported">Progress not reported</progress>"##,
                html_escape(phase)
            ),
        ),
    };
    format!(
        r##"<section class="attention-card" role="status"><span class="attention-icon" aria-hidden="true">...</span><div class="attention-copy"><strong data-volatile-status data-observed-at-ms="{}" data-volatile-expires-at-ms="{}" data-expired-text="Preparation snapshot expired">Preparing model</strong><p>{} &middot; {} {} &middot; observed {}</p>{progress_markup}</div></section>"##,
        window.observed_at_millis,
        window.expires_at_millis,
        html_escape(if progress.label.is_empty() {
            "Provider runtime"
        } else {
            progress.label.as_str()
        }),
        html_escape(phase),
        html_escape(&progress_text),
        relative_time(window),
    )
}

fn provider_recovery_panel(data: &DashboardData, state: &RouteState) -> String {
    let preparation_failure = state.kind == RouteStateKind::Failed
        || (state.kind == RouteStateKind::Mixed
            && matches!(
                state.label,
                "Online with preparation issue" | "Live routes with preparation issue"
            ));
    let offline_or_unpublished = state.kind == RouteStateKind::Stale
        || (state.kind == RouteStateKind::Waiting
            && !matches!(
                state.label,
                "Provider identity not configured" | "Preparing a model"
            ));
    if !preparation_failure && !offline_or_unpublished {
        return String::new();
    }

    let (title, observed, host_action) = if preparation_failure {
        (
            "Recover model preparation",
            "The failed stage and its recorded reason are shown above. Existing healthy routes remain separate from this failure.",
            "On the gateway host, correct the reported preparation problem, then rerun the same mayhem provider start command. This page cannot retry preparation or restart a worker.",
        )
    } else if state.kind == RouteStateKind::Stale {
        (
            "Restore fresh telemetry",
            state.explanation.as_str(),
            "Start or reconnect the provider worker on the gateway host, then wait for a fresh heartbeat. This page cannot restart the worker.",
        )
    } else if state.label == "Prepared; waiting for heartbeat" {
        (
            "Finish route activation",
            state.explanation.as_str(),
            "Keep the provider process running on the gateway host until it publishes the route and sends a fresh heartbeat. This page cannot publish a route.",
        )
    } else {
        (
            "Restore the provider route",
            state.explanation.as_str(),
            "Run the intended mayhem provider start command on the gateway host, or reconnect the existing worker. This page cannot publish a route or start a worker.",
        )
    };
    let badge = if preparation_failure || state.label == "Prepared; waiting for heartbeat" {
        provider_progress_freshness_window(data).map_or_else(
            || {
                format!(
                    r#"<span class="status-badge {}">{}</span>"#,
                    state.tone,
                    html_escape(state.label)
                )
            },
            |window| volatile_status_badge(state.label, state.tone, window, "Refresh to reconfirm"),
        )
    } else {
        format!(
            r#"<span class="status-badge {}">{}</span>"#,
            state.tone,
            html_escape(state.label)
        )
    };

    format!(
        r##"<section class="panel" aria-labelledby="provider-recovery-title"><header class="panel-head"><div class="panel-title"><h2 id="provider-recovery-title">{}</h2><p>Three finite checks; this dashboard remains read-only.</p></div>{badge}</header><div class="panel-body"><div class="checklist"><div class="check-step done"><span class="check-mark" aria-hidden="true">&#10003;</span><div class="check-copy"><strong>1. Read the current snapshot</strong><span>{}</span></div></div><div class="check-step active"><span class="check-mark" aria-hidden="true">2</span><div class="check-copy"><strong>Act on the gateway host</strong><span>{}</span></div></div><div class="check-step"><span class="check-mark" aria-hidden="true">3</span><div class="check-copy"><strong>Confirm new evidence</strong><span>After the host reports progress or reconnects, refresh this page and verify route publication plus a fresh heartbeat.</span></div></div></div></div><footer class="panel-footer"><span>Refresh reads a new gateway snapshot; it does not change provider state.</span><a class="soft-button" href="/mayhem/dashboard/earn/machines" aria-label="Refresh provider machine snapshot">Refresh snapshot</a></footer></section>"##,
        html_escape(title),
        html_escape(observed),
        html_escape(host_action),
    )
}

fn provider_reliability_range(entries: &[&ProviderTableEntry]) -> String {
    entries
        .iter()
        .map(|entry| entry.contract.reputation.clamp(0.0, 1.0))
        .reduce(f64::min)
        .map(|value| format!("{:.2}%", value * 100.0))
        .unwrap_or_else(|| "Unavailable".to_owned())
}

struct ClaimableView {
    value: String,
    freshness: String,
    confirmed: bool,
    basis: &'static str,
}

fn provider_claimable(data: &DashboardData, provider_id: Option<&str>) -> ClaimableView {
    let Some(provider_id) = provider_id else {
        return ClaimableView {
            value: "Unavailable".to_owned(),
            freshness: "Provider identity not configured".to_owned(),
            confirmed: false,
            basis: "No provider-scoped ledger record",
        };
    };
    let mut total = 0_u128;
    let mut matched = 0usize;
    let mut malformed = false;
    for entry in &data.earnings.entries {
        if entry.get("provider").and_then(Value::as_str) != Some(provider_id)
            || entry.get("rail").and_then(Value::as_str) != Some(data.rail.as_str())
        {
            continue;
        }
        let Some(claimable) = entry.get("claimable_au").and_then(value_as_money_au) else {
            malformed = true;
            continue;
        };
        total = total.saturating_add(claimable);
        matched += 1;
    }
    ClaimableView {
        value: if matched == 0 || malformed {
            "Unavailable".to_owned()
        } else {
            format_au_usd(total)
        },
        freshness: if malformed {
            format!(
                "Malformed matching ledger record; {}",
                earnings_refresh_label(&data.earnings)
            )
        } else {
            earnings_refresh_label(&data.earnings)
        },
        confirmed: matched > 0 && !malformed,
        basis: if malformed {
            "Matching ledger record could not be read"
        } else if matched == 0 {
            "No confirmed matching ledger record"
        } else {
            "Last confirmed ledger snapshot"
        },
    }
}

fn earnings_refresh_label(snapshot: &GatewayProviderEarningsSnapshot) -> String {
    if let Some(error) = snapshot.last_error.as_deref() {
        return match snapshot.refreshed_at_seconds {
            Some(timestamp) => format!(
                "Refresh delayed; last successful snapshot {} ago: {}",
                format_elapsed_since(timestamp),
                short_text(error, 80)
            ),
            None => format!(
                "Refresh failed before a snapshot was available: {}",
                short_text(error, 80)
            ),
        };
    }
    snapshot
        .refreshed_at_seconds
        .map(|timestamp| {
            let age = now_secs().saturating_sub(timestamp);
            if age <= EARNINGS_SNAPSHOT_FRESH_SECONDS {
                format!("Refreshed {} ago", format_elapsed_since(timestamp))
            } else {
                format!(
                    "Last refreshed {} ago; refresh to reconfirm",
                    format_elapsed_since(timestamp)
                )
            }
        })
        .unwrap_or_else(|| "Never refreshed".to_owned())
}

fn earnings_snapshot_is_fresh(snapshot: &GatewayProviderEarningsSnapshot) -> bool {
    snapshot.last_error.is_none()
        && snapshot.refreshed_at_seconds.is_some_and(|timestamp| {
            now_secs().saturating_sub(timestamp) <= EARNINGS_SNAPSHOT_FRESH_SECONDS
        })
}

fn provider_earning_rows(data: &DashboardData, provider_id: Option<&str>) -> String {
    let Some(provider_id) = provider_id else {
        return r##"<tr><td colspan="7">Configure a gateway provider identity before showing earnings.</td></tr>"##.to_owned();
    };
    let rows = data.earnings.entries.iter().filter(|entry| entry.get("provider").and_then(Value::as_str) == Some(provider_id)).map(|entry| {
        let money = |name: &str| entry.get(name).and_then(value_as_money_au).map(format_au_usd).unwrap_or_else(|| "Unavailable".to_owned());
        let epoch = entry.get("updated_epoch").and_then(Value::as_u64).map(|value| value.to_string()).unwrap_or_else(|| "Unavailable".to_owned());
        let rail = entry.get("rail").and_then(Value::as_str).unwrap_or("unknown");
        let evidence = evidence_link(&evidence_href("earning", &[("rail", rail)]), "Verify", rail);
        format!(r##"<tr><td><span class="table-primary">{}</span></td><td data-money><span class="money-value">{}</span></td><td data-money><span class="money-value">{}</span></td><td data-money><span class="money-value">{}</span></td><td data-money><span class="money-value">{}</span></td><td>{}</td><td>{}</td></tr>"##, html_escape(&rail.to_ascii_uppercase()), html_escape(&money("total_au")), html_escape(&money("held_au")), html_escape(&money("claimable_au")), html_escape(&money("paid_cum_au")), html_escape(&epoch), evidence)
    }).collect::<String>();
    if rows.is_empty() {
        r##"<tr><td colspan="7">No canonical earnings record matches this provider identity.</td></tr>"##.to_owned()
    } else {
        rows
    }
}

fn provider_identity_attention(data: &DashboardData, requested: Option<&str>) -> String {
    if let Some(provider) = data.local_provider_id.as_deref() {
        return format!(
            r##"<div class="provider-scope result-summary"><strong>Provider identity:</strong> <span class="mono">{}</span> &mdash; configured on this gateway.</div>"##,
            html_escape(short_text(provider, 12).as_ref())
        );
    }
    if let Some(provider) = requested {
        return format!(
            r##"<div class="notice warn"><strong>Provider query ignored:</strong> <span class="mono">{}</span> came from the URL, not the provider identity configured on this gateway. Earn remains unscoped; inspect public routes under Network.</div>"##,
            html_escape(short_text(provider, 24).as_ref())
        );
    }
    attention("warn", "!", "Provider identity is not configured", "Configure the wallet/provider identity on this gateway before Earn can associate routes or earnings records with this host.", None)
}

fn provider_action_center(state: &RouteState) -> String {
    if state.kind == RouteStateKind::Waiting && state.label == "Provider identity not configured" {
        return r##"<section class="panel"><header class="panel-head"><div class="panel-title"><h2>Start provider setup</h2><p>One safe host-side next step</p></div></header><div class="panel-body"><div class="notice warn"><strong>Choose the provider start command for this host.</strong><br>Open the CLI help on the gateway host before selecting a model, engine, and hardware options.</div><div class="field-gap" aria-hidden="true"></div><pre class="code-block"><code id="provider-start-help-command">mayhem provider start --help</code><button class="quiet-button copy-corner js-only" type="button" data-copy data-copy-target="#provider-start-help-command" aria-label="Copy provider start help command"><span data-copy-label>Copy</span></button></pre></div><footer class="panel-footer"><span>This dashboard remains read-only and does not start a worker.</span></footer></section>"##.to_owned();
    }
    if matches!(
        state.kind,
        RouteStateKind::Accepting | RouteStateKind::Blocked | RouteStateKind::Failed
    ) || (state.kind == RouteStateKind::Waiting
        && matches!(
            state.label,
            "Preparing a model" | "Prepared; waiting for heartbeat"
        ))
    {
        // The page header or shared update/progress surface owns these actions.
        return String::new();
    }
    let (title, copy, href, label) = match state.kind {
        RouteStateKind::Accepting => unreachable!("accepting routes do not need an action center"),
        RouteStateKind::Mixed => ("Some routes need attention", "At least one route accepts work, but the provider should not be treated as fully healthy until sibling route issues are reviewed.", "/mayhem/dashboard/earn/machines", "Inspect routes"),
        RouteStateKind::Blocked => unreachable!("blocked routes use the shared update attention"),
        RouteStateKind::Failed => unreachable!("failed preparation uses the page header action"),
        RouteStateKind::Capacity => ("Capacity in use", "Inspect slots and queue before changing limits. The dashboard does not stop work automatically.", "/mayhem/dashboard/earn/machines", "Inspect capacity"),
        RouteStateKind::Draining => ("Routes are draining", "New work is not accepted. Existing work may still be completing.", "/mayhem/dashboard/earn/machines", "Inspect routes"),
        RouteStateKind::Stale => ("Telemetry needs attention", "The gateway will not keep a green state after the heartbeat freshness window expires.", "/mayhem/dashboard/earn/machines", "Inspect freshness"),
        RouteStateKind::Waiting if state.label == "Provider identity not configured" => ("Configure provider identity", "Run mayhem provider start on the gateway host so Earn can scope routes and earnings to that identity.", "", "Host action required"),
        RouteStateKind::Waiting if state.label == "Waiting for first heartbeat" => ("Restore the worker heartbeat", "The route is published, but this gateway has no fresh worker heartbeat. Start or reconnect the provider worker before treating it as available.", "/mayhem/dashboard/earn/machines", "Inspect route"),
        RouteStateKind::Waiting => ("Publish a provider route", "Run mayhem provider start on the gateway host, then wait for route publication and a fresh heartbeat.", "", "Host action required"),
    };
    let action = if href.is_empty() {
        format!(r##"<span>{}</span>"##, html_escape(label))
    } else {
        format!(
            r##"<a href="{}">{}</a>"##,
            html_escape(href),
            html_escape(label)
        )
    };
    format!(
        r##"<section class="panel"><header class="panel-head"><div class="panel-title"><h2>Action center</h2><p>One highest-value next step</p></div></header><div class="panel-body"><div class="notice {}"><strong>{}</strong><br>{}</div></div><footer class="panel-footer"><span>{}</span>{action}</footer></section>"##,
        state.tone,
        html_escape(title),
        html_escape(copy),
        html_escape(state.label)
    )
}

fn provider_route_rows(
    data: &DashboardData,
    entries: &[&ProviderTableEntry],
    page: PageWindow,
) -> String {
    if entries.is_empty() {
        return format!(
            r##"<tr><td colspan="5">{}</td></tr>"##,
            empty_block(
                "No serving routes yet",
                "Configure a provider identity and publish a route; fresh heartbeat capacity will appear here after it is observed.",
                None,
            )
        );
    }
    entries.iter().skip(page.start).take(page.len()).map(|entry| {
        let state = route_operational_state(entry);
        let heartbeat = entry.heartbeat.as_ref();
        let slots = heartbeat_value(data, entry, heartbeat.map(|value| format!("{} / {} active · {} free", value.slots.active, value.slots.max, value.q.free_slots)));
        let queue = heartbeat_value(data, entry, heartbeat.map(|value| format!("{} queued · {}ms wait", value.q.engine_backlog, value.q.est_wait_ms)));
        let perf = heartbeat_value(data, entry, heartbeat.map(|value| value.perf.tok_s.map(|tok_s| format!("{}ms TTFT · {tok_s:.1} tok/s", value.perf.ttft_ms)).unwrap_or_else(|| format!("{}ms TTFT", value.perf.ttft_ms))));
        let explanation = heartbeat_explanation(data, entry, &state.explanation);
        format!(r##"<tr data-filter-row data-filter-text="{} {} {} {}"><td data-export-value="{} / room {}" data-sort-value="{} / {}"><span class="table-primary mono">{}</span><span class="table-secondary">room {}</span></td><td data-export-value="{} / {}"><span class="status-badge {}">{}</span><span class="table-secondary">{}</span></td><td>{slots}</td><td>{queue}</td><td>{perf}</td></tr>"##, html_escape(&entry.contract.model_id), html_escape(&entry.key.room_id), html_escape(state.label), html_escape(&state.explanation), html_escape(&entry.contract.model_id), html_escape(&entry.key.room_id), html_escape(&entry.contract.model_id), html_escape(&entry.key.room_id), html_escape(short_text(&entry.contract.model_id, 30).as_ref()), html_escape(short_text(&entry.key.room_id, 12).as_ref()), html_escape(state.label), html_escape(&state.explanation), state.tone, html_escape(state.label), explanation)
    }).collect()
}

fn provider_machine_rows(
    data: &DashboardData,
    entries: &[&ProviderTableEntry],
    page: PageWindow,
) -> String {
    if entries.is_empty() {
        return format!(
            r##"<tr><td colspan="6">{}</td></tr>"##,
            empty_block(
                "No machine routes yet",
                "No catalog route currently matches this gateway identity. Start or reconnect a worker on the gateway host, then refresh the snapshot.",
                None,
            )
        );
    }
    entries.iter().skip(page.start).take(page.len()).map(|entry| {
        let state = route_operational_state(entry);
        let heartbeat = entry.heartbeat.as_ref();
        let capacity = heartbeat_value(data, entry, heartbeat.map(|value| format!("{} / {} active · {} free", value.slots.active, value.slots.max, value.q.free_slots)));
        let freshness = heartbeat_age(data, entry);
        let explanation = heartbeat_explanation(data, entry, &state.explanation);
        let evidence = evidence_link(&evidence_href("route", &[("provider", entry.key.provider.as_str()), ("enclave", entry.key.enclave_id.as_str()), ("room", entry.key.room_id.as_str()), ("model", entry.contract.model_id.as_str())]), "Verify", &entry.contract.model_id);
        format!(r##"<tr data-filter-row data-filter-text="{} {} {} {} {}"><td data-export-value="{} / room {}" data-sort-value="{} / {}"><span class="table-primary mono">{}</span><span class="table-secondary">room {}</span></td><td data-export-value="{}">{}</td><td data-export-value="{} / {}"><span class="status-badge {}">{}</span><span class="table-secondary">{}</span></td><td>{capacity}</td><td data-sort-value="{}">{freshness}</td><td>{}</td></tr>"##, html_escape(&entry.key.enclave_id), html_escape(&entry.key.room_id), html_escape(&entry.contract.model_id), html_escape(state.label), html_escape(&state.explanation), html_escape(&entry.key.enclave_id), html_escape(&entry.key.room_id), html_escape(&entry.key.enclave_id), html_escape(&entry.key.room_id), html_escape(short_text(&entry.key.enclave_id, 18).as_ref()), html_escape(short_text(&entry.key.room_id, 12).as_ref()), html_escape(&entry.contract.model_id), html_escape(&entry.contract.model_id), html_escape(state.label), html_escape(&state.explanation), state.tone, html_escape(state.label), explanation, entry.heartbeat_age_millis.unwrap_or(u64::MAX), evidence)
    }).collect()
}

fn network_provider_rows(data: &DashboardData, page: PageWindow) -> String {
    let mut rows = String::new();
    let mut index = 0usize;
    for model in data.models.iter() {
        for candidate in &model.mayhem.route_candidates {
            if index >= page.end {
                break;
            }
            if index < page.start {
                index += 1;
                continue;
            }
            let entry = dashboard_entry_for_route(&data.entries, candidate);
            let state = entry.map(route_operational_state).unwrap_or(RouteState {
                kind: RouteStateKind::Waiting,
                label: "No provider-table entry",
                tone: "",
                explanation: "The canonical route is not present in the current provider table."
                    .to_owned(),
            });
            let capacity = entry.map_or_else(
                || "Unavailable".to_owned(),
                |entry| {
                    heartbeat_value(
                        data,
                        entry,
                        entry.heartbeat.as_ref().map(|heartbeat| {
                            format!(
                                "{} / {} active · {} free",
                                heartbeat.slots.active, heartbeat.slots.max, heartbeat.q.free_slots
                            )
                        }),
                    )
                },
            );
            let perf = entry.map_or_else(
                || "Unavailable".to_owned(),
                |entry| {
                    heartbeat_value(
                        data,
                        entry,
                        entry.heartbeat.as_ref().map(|heartbeat| {
                            heartbeat
                                .perf
                                .tok_s
                                .map(|tok_s| {
                                    format!("{}ms · {tok_s:.1} tok/s", heartbeat.perf.ttft_ms)
                                })
                                .unwrap_or_else(|| format!("{}ms TTFT", heartbeat.perf.ttft_ms))
                        }),
                    )
                },
            );
            let explanation = entry.map_or_else(
                || html_escape(&state.explanation),
                |entry| heartbeat_explanation(data, entry, &state.explanation),
            );
            let evidence = evidence_link(
                &evidence_href(
                    "route",
                    &[
                        ("provider", candidate.provider.as_str()),
                        ("enclave", candidate.enclave_id.as_str()),
                        ("room", candidate.room_id.as_str()),
                        ("model", model.id.as_str()),
                    ],
                ),
                "Verify",
                &model.id,
            );
            rows.push_str(&format!(r##"<tr data-filter-row data-filter-text="{} {} {} {} {}"><td data-export-value="{} / room {} / enclave {}"><span class="table-primary mono">{}</span><span class="table-secondary">room {} &middot; enclave {}</span></td><td data-export-value="{}">{}</td><td><span class="status-badge {}">{}</span><span class="table-secondary">{}</span></td><td>{capacity}</td><td>{perf}</td><td>{}</td></tr>"##, html_escape(&candidate.provider), html_escape(&candidate.room_id), html_escape(&candidate.enclave_id), html_escape(&model.id), html_escape(state.label), html_escape(&candidate.provider), html_escape(&candidate.room_id), html_escape(&candidate.enclave_id), html_escape(short_text(&candidate.provider, 16).as_ref()), html_escape(short_text(&candidate.room_id, 12).as_ref()), html_escape(short_text(&candidate.enclave_id, 12).as_ref()), html_escape(&model.id), html_escape(short_text(&model.id, 30).as_ref()), state.tone, html_escape(state.label), explanation, evidence));
            index += 1;
        }
        if index >= page.end {
            break;
        }
    }
    if rows.is_empty() {
        format!(
            r##"<tr><td colspan="6">{}</td></tr>"##,
            empty_block(
                "No provider routes",
                "No canonical provider routes are loaded in this catalog snapshot.",
                None,
            )
        )
    } else {
        rows
    }
}

fn earn_subnav(active: DashboardProductPage) -> String {
    let links = [
        (
            DashboardProductPage::Earn,
            "Overview",
            "/mayhem/dashboard/earn",
        ),
        (
            DashboardProductPage::EarnJobs,
            "Jobs",
            "/mayhem/dashboard/earn/jobs",
        ),
        (
            DashboardProductPage::EarnEarnings,
            "Earnings",
            "/mayhem/dashboard/earn/earnings",
        ),
        (
            DashboardProductPage::EarnMachines,
            "Machines",
            "/mayhem/dashboard/earn/machines",
        ),
    ];
    let advanced_open = matches!(
        active,
        DashboardProductPage::EarnOpportunities | DashboardProductPage::EarnReliability
    );
    format!(
        r##"{}<details class="subnav-advanced"{}><summary>Advanced provider analysis</summary><div><a href="/mayhem/dashboard/earn/opportunities"{}>Model opportunities</a><a href="/mayhem/dashboard/earn/reliability"{}>Reliability</a></div></details>"##,
        contextual_nav("Earn", active, &links),
        if advanced_open { " open" } else { "" },
        if active == DashboardProductPage::EarnOpportunities {
            r#" aria-current="page""#
        } else {
            ""
        },
        if active == DashboardProductPage::EarnReliability {
            r#" aria-current="page""#
        } else {
            ""
        },
    )
}

fn network_subnav(active: DashboardProductPage) -> String {
    let links = [
        (
            DashboardProductPage::Network,
            "Overview",
            "/mayhem/dashboard/network",
        ),
        (
            DashboardProductPage::NetworkModels,
            "Models",
            "/mayhem/dashboard/network/models",
        ),
        (
            DashboardProductPage::NetworkProviders,
            "Providers",
            "/mayhem/dashboard/network/providers",
        ),
        (
            DashboardProductPage::NetworkMarkets,
            "Markets",
            "/mayhem/dashboard/network/markets",
        ),
        (
            DashboardProductPage::NetworkActivity,
            "Routes",
            "/mayhem/dashboard/network/activity",
        ),
        (
            DashboardProductPage::NetworkEvidence,
            "Evidence",
            "/mayhem/dashboard/network/evidence",
        ),
    ];
    contextual_nav("Network", active, &links)
}

fn contextual_nav<const N: usize>(
    label: &str,
    active: DashboardProductPage,
    links: &[(DashboardProductPage, &str, &str); N],
) -> String {
    let links = links
        .iter()
        .map(|(page, text, href)| {
            format!(
                r##"<a href="{href}"{}>{}</a>"##,
                if *page == active {
                    r##" aria-current="page""##
                } else {
                    ""
                },
                html_escape(text)
            )
        })
        .collect::<String>();
    format!(
        r##"<nav class="subnav" aria-label="{} sections">{links}</nav>"##,
        html_escape(label)
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PageWindow {
    current: usize,
    page_count: usize,
    start: usize,
    end: usize,
    total: usize,
}

impl PageWindow {
    fn from_query(total: usize, page_size: usize, requested: Option<&str>) -> Self {
        assert!(page_size > 0, "dashboard page size must be nonzero");
        let page_count = total.div_ceil(page_size).max(1);
        let current = requested
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|page| *page > 0)
            .unwrap_or(1)
            .min(page_count);
        let start = current.saturating_sub(1).saturating_mul(page_size);
        let end = start.saturating_add(page_size).min(total);
        Self {
            current,
            page_count,
            start,
            end,
            total,
        }
    }

    fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    fn status(self, rows_label: &str) -> String {
        if self.total == 0 {
            return format!("Showing 0 {rows_label}. Page 1 of 1.");
        }
        format!(
            "Showing rows {}&ndash;{} of {} {rows_label}. Page {} of {}.",
            self.start + 1,
            self.end,
            self.total,
            self.current,
            self.page_count,
        )
    }
}

fn pagination_href_with_param(
    path: &str,
    page: usize,
    preserved_query: &[(&str, &str)],
    page_param: &str,
) -> String {
    let mut href = path.to_owned();
    let mut first = true;
    for (key, value) in preserved_query
        .iter()
        .copied()
        .filter(|(key, value)| !value.is_empty() && *key != page_param)
        .chain(std::iter::once((page_param, "")))
    {
        href.push_str(if first { "?" } else { "&amp;" });
        first = false;
        href.push_str(&html_escape(key));
        href.push('=');
        if key == page_param {
            href.push_str(&page.to_string());
        } else {
            href.push_str(&dashboard_url_encode(value));
        }
    }
    href
}

fn pagination_nav(
    window: PageWindow,
    path: &str,
    preserved_query: &[(&str, &str)],
    rows_label: &str,
) -> String {
    pagination_nav_with_param(window, path, preserved_query, rows_label, "page")
}

fn pagination_nav_with_param(
    window: PageWindow,
    path: &str,
    preserved_query: &[(&str, &str)],
    rows_label: &str,
    page_param: &str,
) -> String {
    if window.page_count <= 1 {
        return String::new();
    }
    let previous = if window.current > 1 {
        format!(
            r##"<a class="quiet-button" rel="prev" href="{}" aria-label="Previous page of {}">Previous</a>"##,
            pagination_href_with_param(path, window.current - 1, preserved_query, page_param,),
            html_escape(rows_label),
        )
    } else {
        r##"<span class="quiet-button pagination-disabled" aria-disabled="true">Previous</span>"##
            .to_owned()
    };
    let next = if window.current < window.page_count {
        format!(
            r##"<a class="quiet-button" rel="next" href="{}" aria-label="Next page of {}">Next</a>"##,
            pagination_href_with_param(path, window.current + 1, preserved_query, page_param,),
            html_escape(rows_label),
        )
    } else {
        r##"<span class="quiet-button pagination-disabled" aria-disabled="true">Next</span>"##
            .to_owned()
    };
    format!(
        r##"<nav class="pagination" aria-label="{} pagination">{previous}<span class="pagination-page" aria-current="page">Page {} of {}</span>{next}</nav>"##,
        html_escape(rows_label),
        window.current,
        window.page_count,
    )
}

fn pagination_nav_with_optional_provider(
    window: PageWindow,
    path: &str,
    provider: Option<&str>,
    rows_label: &str,
) -> String {
    match provider.filter(|provider| !provider.is_empty()) {
        Some(provider) => pagination_nav(window, path, &[("provider", provider)], rows_label),
        None => pagination_nav(window, path, &[], rows_label),
    }
}

fn bounded_rows_summary(total: usize, limit: usize, rows_label: &str) -> String {
    let shown = total.min(limit);
    if total > limit {
        format!("Showing first {shown} of {total} {rows_label}.")
    } else {
        format!("Showing {shown} {rows_label}.")
    }
}

fn shown_rows_filter(
    prefix: &str,
    table_id: &str,
    aria_label: &str,
    shown: usize,
    noun: &str,
) -> (String, String) {
    shown_rows_filter_scoped(prefix, table_id, aria_label, shown, noun, None)
}

fn shown_rows_filter_scoped(
    prefix: &str,
    table_id: &str,
    aria_label: &str,
    shown: usize,
    noun: &str,
    query_prefix: Option<&str>,
) -> (String, String) {
    if shown == 0 {
        return (String::new(), String::new());
    }
    let input_id = format!("{prefix}-filter");
    let count_id = format!("{prefix}-count");
    let empty_id = format!("{prefix}-empty");
    let query_scope = query_prefix
        .filter(|value| !value.is_empty())
        .map(|value| format!(r##" data-table-query-prefix="{}""##, html_escape(value)))
        .unwrap_or_default();
    let controls = format!(
        r##"<div class="panel-actions"><label class="field-label js-only" for="{}">Filter shown page</label><input class="search-field js-only" id="{}" type="search" aria-label="{}" data-table-filter="#{}" data-filter-count="#{}" data-filter-empty="#{}"{query_scope}><span class="result-summary" id="{}">{} shown rows</span></div>"##,
        html_escape(&input_id),
        html_escape(&input_id),
        html_escape(aria_label),
        html_escape(table_id),
        html_escape(&count_id),
        html_escape(&empty_id),
        html_escape(&count_id),
        shown,
    );
    let empty = format!(
        r##"<div id="{}" hidden>{}</div>"##,
        html_escape(&empty_id),
        empty_block(
            &format!("No matching shown {noun}"),
            "Clear or change the search. Rows on other pages are not searched.",
            None,
        )
    );
    (controls, empty)
}

fn metric(label: &str, value_html: &str, meta: &str, source: &str) -> String {
    metric_with_meta_html(label, value_html, &html_escape(meta), source)
}

// Status facts render as a badge row, never as display-type metric values;
// the value slot is reserved for numbers and amounts.
fn metric_status(label: &str, badge_html: &str, meta: &str, source: &str) -> String {
    format!(
        r##"<article class="metric"><div class="metric-top"><span class="metric-label">{}</span><span class="metric-state">{}</span></div><div class="metric-status">{badge_html}</div><p class="metric-meta">{}</p></article>"##,
        html_escape(label),
        html_escape(source),
        html_escape(meta),
    )
}

fn status_badge(label: &str, tone: &str) -> String {
    format!(
        r#"<span class="status-badge {}">{}</span>"#,
        html_escape(tone),
        html_escape(label),
    )
}

fn metric_with_meta_html(label: &str, value_html: &str, meta_html: &str, source: &str) -> String {
    format!(
        r##"<article class="metric"><div class="metric-top"><span class="metric-label">{}</span><span class="metric-state">{}</span></div><div class="metric-value">{value_html}</div><p class="metric-meta">{meta_html}</p></article>"##,
        html_escape(label),
        html_escape(source),
    )
}

fn money_html(value: &str) -> String {
    format!(
        r##"<span data-money><span class="money-value mono">{}</span></span>"##,
        html_escape(value)
    )
}

fn privacy_amount_text(value: &str, amount: Option<&str>) -> String {
    let Some(amount) = amount else {
        return html_escape(value);
    };
    let Some(offset) = value.find(amount) else {
        return html_escape(value);
    };
    let suffix_offset = offset + amount.len();
    format!(
        r##"{}<span data-money><span class="money-value">{}</span></span>{}"##,
        html_escape(&value[..offset]),
        html_escape(amount),
        html_escape(&value[suffix_offset..]),
    )
}

fn attention(
    tone: &str,
    icon: &str,
    title: &str,
    copy: &str,
    action: Option<(&str, &str)>,
) -> String {
    let action = action
        .map(|(label, href)| {
            format!(
                r##"<a class="soft-button" href="{}">{}</a>"##,
                html_escape(href),
                html_escape(label)
            )
        })
        .unwrap_or_default();
    format!(
        r##"<section class="attention-card {tone}" role="status"><span class="attention-icon" aria-hidden="true">{}</span><div class="attention-copy"><strong>{}</strong><p>{}</p></div>{action}</section>"##,
        html_escape(icon),
        html_escape(title),
        html_escape(copy)
    )
}

fn volatile_attention(
    tone: &str,
    icon: &str,
    title: &str,
    copy: &str,
    action: Option<(&str, &str)>,
    window: FreshnessWindow,
    expired_title: &str,
) -> String {
    let action = action
        .map(|(label, href)| {
            format!(
                r##"<a class="soft-button" href="{}">{}</a>"##,
                html_escape(href),
                html_escape(label)
            )
        })
        .unwrap_or_default();
    format!(
        r##"<section class="attention-card {tone}" role="status"><span class="attention-icon" aria-hidden="true">{}</span><div class="attention-copy"><strong data-volatile-status data-observed-at-ms="{}" data-volatile-expires-at-ms="{}" data-expired-text="{}">{}</strong><p>{} &middot; observed {}</p></div>{action}</section>"##,
        html_escape(icon),
        window.observed_at_millis,
        window.expires_at_millis,
        html_escape(expired_title),
        html_escape(title),
        html_escape(copy),
        relative_time(window),
    )
}

fn empty_block(title: &str, copy: &str, action: Option<(&str, &str)>) -> String {
    empty_block_with_heading(title, copy, action, false)
}

fn page_empty_block(title: &str, copy: &str, action: Option<(&str, &str)>) -> String {
    empty_block_with_heading(title, copy, action, true)
}

fn empty_block_with_heading(
    title: &str,
    copy: &str,
    action: Option<(&str, &str)>,
    page_level: bool,
) -> String {
    let action = action
        .map(|(label, href)| {
            format!(
                r##"<a class="primary-button" href="{}">{}</a>"##,
                html_escape(href),
                html_escape(label)
            )
        })
        .unwrap_or_default();
    let heading = if page_level {
        format!(r##"<h2>{}</h2>"##, html_escape(title))
    } else {
        format!(r##"<h3>{}</h3>"##, html_escape(title))
    };
    format!(
        r##"<div class="empty-block"><div class="empty-block-inner"><div class="empty-symbol" aria-hidden="true">&mdash;</div>{heading}<p>{}</p>{action}</div></div>"##,
        html_escape(copy)
    )
}

// Every countable noun in dashboard copy is regular, so simple "s" suffixing
// keeps "1 route" / "2 routes" out of "route(s)" territory.
fn count_noun(count: u64, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

fn timestamp_seconds(value: u64) -> u64 {
    if value > 100_000_000_000 {
        value / 1_000
    } else {
        value
    }
}

fn format_millis_age(value: u64) -> String {
    if value < 1_000 {
        format!("{value}ms")
    } else if value < 60_000 {
        format!("{}s", value / 1_000)
    } else if value < 3_600_000 {
        format!("{} min", value / 60_000)
    } else if value < 86_400_000 {
        format!("{}h", value / 3_600_000)
    } else if value < 14 * 86_400_000 {
        format!("{}d", value / 86_400_000)
    } else {
        format!("{}w", value / (7 * 86_400_000))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn earnings_fixture(provider: &str, amount: MoneyAu) -> Value {
        json!({
            "provider": provider,
            "rail": "fiat",
            "denom": "au_usd",
            "total_au": amount.to_string(),
            "held_au": "0",
            "claimable_au": amount.to_string(),
            "paid_cum_au": "0",
            "updated_at": now_secs(),
        })
    }

    fn activity_receipt_fixture(
        session_id: &str,
        final_receipt: bool,
        timestamp: u64,
    ) -> StoredReceipt {
        let voucher = SpendVoucher {
            body: SpendVoucherBody {
                session_id: session_id.to_owned(),
                billing_id: format!("billing-{session_id}"),
                billing_attempt: 0,
                billing_prior_usage: ReceiptUsage::default(),
                billing_prior_au_owed_cum: 0,
                rail: "fiat".to_owned(),
                enclave_id: "enclave-test".to_owned(),
                price_ver: 1,
                locked_rate_map: Vec::new(),
                locked_per_req_au: 0,
                locked_min_session_au: 0,
                served_ctx: 4_096,
                required_modalities: Vec::new(),
                required_specialities: BTreeMap::new(),
                ctx_bracket: None,
                ctx_bracket_table_ver: None,
                max_spend_au: AU_PER_USD,
                checkpoint_every: CheckpointPolicy {
                    tokens: 256,
                    ms: 750,
                },
            },
            user_sig: "voucher-signature".to_owned(),
        };
        let body = ReceiptBody {
            schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
            session_id: session_id.to_owned(),
            billing_id: format!("billing-{session_id}"),
            billing_attempt: 0,
            billing_prior_usage: ReceiptUsage::default(),
            billing_prior_au_owed_cum: 0,
            seq: if final_receipt { 2 } else { 1 },
            final_receipt,
            rail: "fiat".to_owned(),
            user: "user-test".to_owned(),
            provider: "provider-test".to_owned(),
            enclave_id: "enclave-test".to_owned(),
            model_id: "model-test".to_owned(),
            price_ver: 1,
            locked_rate_map: Vec::new(),
            locked_per_req_au: 0,
            locked_min_session_au: 0,
            served_ctx: 4_096,
            ctx_bracket: None,
            ctx_bracket_table_ver: None,
            rules_ver: 1,
            usage: ReceiptUsage::text(12, 4),
            usage_attribution: BTreeMap::new(),
            au_owed_cum: AU_PER_USD / 4,
            prompt_hash: "prompt-hash".to_owned(),
            ts: timestamp,
        };
        StoredReceipt {
            rail: "fiat".to_owned(),
            receipt: SessionReceipt {
                body,
                enclave_sig: "enclave-signature".to_owned(),
                enclave_pubkey: "enclave-key".to_owned(),
                user_sig: "user-signature".to_owned(),
            },
            receipt_ack: ReceiptAck {
                session_id: session_id.to_owned(),
                seq: if final_receipt { 2 } else { 1 },
                user_sig: "ack-signature".to_owned(),
            },
            voucher,
            access_token: None,
        }
    }

    #[test]
    fn product_paths_are_explicit_and_unknown_paths_stay_closed() {
        let paths = [
            "playground",
            "models",
            "activity",
            "wallet",
            "connect",
            "earn",
            "earn/jobs",
            "earn/machines",
            "earn/opportunities",
            "earn/earnings",
            "earn/reliability",
            "network",
            "network/models",
            "network/providers",
            "network/markets",
            "network/activity",
            "network/evidence",
            "help",
            "settings",
        ];
        for path in paths {
            assert!(DashboardProductPage::from_path(path).is_some(), "{path}");
        }
        assert_eq!(DashboardProductPage::from_path(""), None);
        assert_eq!(DashboardProductPage::from_path("admin"), None);
        assert_eq!(DashboardProductPage::from_path("earn/disputes"), None);
        assert_eq!(DashboardProductPage::from_path("earn/private"), None);
    }

    #[test]
    fn dashboard_receipt_and_recovery_history_survives_gateway_restart() {
        let dir = tempfile::tempdir().expect("history tempdir");
        let path = dir.path().join("dashboard-history.json");
        let state = GatewayState::from_models(Vec::new()).with_dashboard_history_path(&path);
        state
            .record_workbench_receipt(activity_receipt_fixture(
                "persistent-session",
                true,
                now_secs(),
            ))
            .expect("persist receipt");
        state.pause_session(PausedSession {
            session_id: "paused-session".to_owned(),
            reason: "provider recovery pending".to_owned(),
        });
        drop(state);

        let restored = GatewayState::from_models(Vec::new()).with_dashboard_history_path(&path);
        assert_eq!(restored.receipts().len(), 1);
        assert_eq!(
            restored.receipts()[0].receipt.body.session_id,
            "persistent-session"
        );
        assert_eq!(restored.paused_sessions().len(), 1);
        assert_eq!(restored.paused_sessions()[0].session_id, "paused-session");
        let stored = fs::read_to_string(path).expect("read persisted history");
        assert!(!stored.contains("\"prompt\":"));
        assert!(!stored.contains("\"response\":"));
    }

    #[test]
    fn timestamps_accept_seconds_or_milliseconds_without_inventing_age() {
        assert_eq!(timestamp_seconds(1_725_000_000), 1_725_000_000);
        assert_eq!(timestamp_seconds(1_725_000_000_123), 1_725_000_000);
    }

    #[test]
    fn provider_capacity_values_require_fresh_evidence_and_disclose_partial_coverage() {
        let state = GatewayState::from_models(Vec::new())
            .with_local_provider_id("provider-a")
            .with_provider_heartbeat_ttl_millis(4_321);
        let data = DashboardData::from_state(&state);
        assert_eq!(data.provider_heartbeat_ttl_millis, 4_321);

        let html = earn_overview_page(&data, 60, None, None);
        assert!(html.contains("Live route metrics"));
        assert!(html.contains("No routes yet"));
        assert!(!html.contains(">0 / 0<"));

        let partial = ProviderSlotTotals {
            active: 2,
            max: 4,
            free: 2,
            backlog: 1,
            fresh_routes: 1,
            total_routes: 3,
        };
        let window = FreshnessWindow {
            observed_at_millis: 1_000,
            expires_at_millis: 5_321,
        };
        let notice = provider_coverage_notice(&partial, Some(window));
        assert!(notice.contains("Partial coverage"));
        assert!(notice.contains("1 of 3 configured routes"));
        assert!(notice.contains("2 delayed routes excluded"));
        assert!(notice.contains(r#"data-relative-time data-observed-at-ms="1000""#));
        assert_eq!(
            provider_metric_basis(&partial),
            "Fresh heartbeat totals from 1 of 3 configured routes"
        );

        let value = volatile_text("2 / 4", window, "Unavailable");
        assert!(value.contains("data-volatile-value"));
        assert!(value.contains(r#"data-observed-at-ms="1000""#));
        assert!(value.contains(r#"data-volatile-expires-at-ms="5321""#));
        assert!(value.contains(r#"data-expired-text="Unavailable""#));

        let badge = mark_volatile_capacity_badges(
            r#"<span class="status-badge good">Capacity advertised</span>"#,
            Some(window),
        );
        assert!(badge.contains("data-volatile-status"));
        assert!(badge.contains("Refresh to reconfirm"));
    }

    #[test]
    fn activity_prioritizes_state_backed_incomplete_records_and_recovery() {
        let mut data = DashboardData::from_state(&GatewayState::from_models(Vec::new()));
        data.receipts = vec![
            activity_receipt_fixture("session-final", false, now_secs().saturating_sub(90)),
            activity_receipt_fixture("session-final", true, now_secs().saturating_sub(60)),
            activity_receipt_fixture("session-incomplete", false, now_secs().saturating_sub(30)),
        ];
        data.receipt_checkpoint_count = 3;
        data.paused_sessions.push(PausedSession {
            session_id: "session-paused".to_owned(),
            reason: "receipt co-signing stopped".to_owned(),
        });

        let html = activity_page(&data, 60, None);
        let incomplete = html.find("session-incomplete").expect("incomplete receipt");
        let paused = html.find("session-paused").expect("pause record");
        let final_receipt = html.find("session-final").expect("final receipt");
        assert!(incomplete < final_receipt);
        assert!(paused < final_receipt);
        assert_eq!(data.incomplete_session_count(), 2);
        let prioritized = prioritized_activity_records(&data);
        assert_eq!(
            prioritized
                .iter()
                .filter(|record| matches!(
                    record,
                    ActivityRecord::Receipt(receipt)
                        if receipt.receipt.body.session_id == "session-final"
                ))
                .count(),
            1
        );
        assert!(html.contains("Open records to review"));
        assert!(html.contains(r##"href="#incomplete-activity">Review open records"##));
        assert!(html.contains("does not mean work is still running"));
        assert!(!html.contains("Execution state unknown"));
        assert!(!html.contains("Current execution state unknown &middot; not a payout state"));
    }

    #[test]
    fn evidence_payloads_explain_model_sources_and_receipt_metering_context() {
        let state = GatewayState::fixture();
        let model_id = state.models_snapshot()[0].id.clone();
        let model_payload = dashboard_evidence_payload(
            &state,
            &DashboardQuery {
                kind: Some("model".to_owned()),
                id: Some(model_id),
                ..DashboardQuery::default()
            },
        )
        .expect("model evidence");
        assert!(model_payload.get("generated_at_millis").is_some());
        assert!(model_payload
            .get("interpretation")
            .and_then(Value::as_str)
            .is_some_and(|value| value.contains("point-in-time")));
        assert!(model_payload["raw"].get("catalog_model").is_some());
        assert_eq!(
            model_payload["raw"].get("id").and_then(Value::as_str),
            model_payload["raw"]["catalog_model"]
                .get("id")
                .and_then(Value::as_str)
        );
        assert!(model_payload["raw"]
            .get("matching_route_sources")
            .is_some_and(Value::is_array));
        assert!(model_payload["raw"].get("heartbeat_ttl_millis").is_some());

        let receipt = activity_receipt_fixture("receipt-evidence", true, now_secs());
        state
            .record_workbench_receipt(receipt)
            .expect("fixture receipt can be retained");
        let receipt_payload = dashboard_evidence_payload(
            &state,
            &DashboardQuery {
                kind: Some("receipt".to_owned()),
                id: Some("receipt-evidence".to_owned()),
                ..DashboardQuery::default()
            },
        )
        .expect("receipt evidence");
        let facts = receipt_payload["facts"].as_array().expect("receipt facts");
        let fact_value = |label: &str| {
            facts.iter().find_map(|fact| {
                (fact["label"].as_str() == Some(label))
                    .then(|| fact["value"].as_str().unwrap_or_default())
            })
        };
        assert_eq!(fact_value("Final cumulative charge"), Some("$0.25"));
        assert!(fact_value("Usage context").is_some_and(|value| value.contains("input_token: 12")));
        assert!(fact_value("Usage context").is_some_and(|value| value.contains("output_token: 4")));
        assert!(receipt_payload
            .get("interpretation")
            .and_then(Value::as_str)
            .is_some_and(|value| value.contains("not a payout or settlement record")));
    }

    #[test]
    fn provider_progress_is_reported_or_derived_without_inventing_zero() {
        let mut progress = DashboardProviderLoadProgress::default();
        assert_eq!(provider_progress_percent(&progress), None);

        progress.position = Some(1);
        progress.total = Some(4);
        assert_eq!(provider_progress_percent(&progress), Some(25));

        progress.percent = Some(68);
        assert_eq!(provider_progress_percent(&progress), Some(68));

        progress.percent = Some(140);
        assert_eq!(provider_progress_percent(&progress), Some(100));
    }

    #[test]
    fn active_probation_uses_only_the_reported_successful_session_denominator() {
        let probation = ProviderProbation {
            active: true,
            since_seconds: 1_725_000_000,
            successful_sessions: 7,
            required_successful_sessions: 25,
            required_seconds: 7 * 24 * 60 * 60,
            caps: crate::ProbationCaps::default(),
        };

        let html = provider_probation_progress(Some(&probation));
        assert!(html.contains("Probation active"));
        assert!(html.contains("7 / 25 successful sessions"));
        assert!(html.contains(
            r#"<progress max="25" value="7" aria-label="Probation successful-session requirement: 7 of 25">"#
        ));
        assert!(html.contains("18 successful sessions remaining"));
        assert!(html.contains("elapsed-time condition remains separate"));

        let mut zero_denominator = probation.clone();
        zero_denominator.required_successful_sessions = 0;
        let html = provider_probation_progress(Some(&zero_denominator));
        assert!(!html.contains("<progress"));
        assert!(html.contains("no percentage is shown"));
    }

    #[test]
    fn machines_surface_current_provider_failure_and_reason() {
        let state = GatewayState::from_models(Vec::new()).with_local_provider_id("provider-a");
        let mut data = DashboardData::from_state(&state);
        data.provider_load_progress.insert(
            ("provider-a".to_owned(), "model-a".to_owned()),
            DashboardProviderLoadProgress {
                provider: "provider-a".to_owned(),
                model_id: "model-a".to_owned(),
                label: "verify catalog artifact".to_owned(),
                phase: "verify".to_owned(),
                status: "error".to_owned(),
                error: Some("artifact signature mismatch".to_owned()),
                updated_at_ms: now_millis_u64(),
                ..DashboardProviderLoadProgress::default()
            },
        );

        let html = earn_machines_page(&data, 60, None, None);
        assert!(html.contains("Setup blocked by model failure"));
        assert!(html.contains("Model preparation failed"));
        assert!(html.contains(
            "model-a: verify catalog artifact (verify) failed: artifact signature mismatch"
        ));
        assert_eq!(html.matches("artifact signature mismatch").count(), 1);
        assert!(html.contains("Recover model preparation"));
        assert!(html.contains("rerun the same mayhem provider start command"));
        assert!(html.contains("This page cannot retry preparation or restart a worker."));
        assert!(html.contains(
            r#"href="/mayhem/dashboard/earn/machines" aria-label="Refresh provider machine snapshot">Refresh snapshot"#
        ));

        let overview = earn_overview_page(&data, 60, None, None);
        assert!(overview.contains("Setup blocked by model failure"));
        assert!(overview
            .contains("model-a: verify catalog artifact failed: artifact signature mismatch"));
        assert_eq!(overview.matches("artifact signature mismatch").count(), 1);
        assert!(!overview.contains("<h2>Action center</h2>"));
    }

    #[test]
    fn failed_model_load_does_not_hide_healthy_provider_capacity() {
        let progress = DashboardProviderLoadProgress {
            model_id: "model-b".to_owned(),
            label: "load model weights".to_owned(),
            phase: "load".to_owned(),
            status: "error".to_owned(),
            error: Some("out of memory".to_owned()),
            ..DashboardProviderLoadProgress::default()
        };

        let state = provider_load_failure_state(&progress, 2, 1);
        assert_eq!(state.kind, RouteStateKind::Mixed);
        assert_eq!(state.label, "Online with preparation issue");
        assert_eq!(state.tone, "warn");
        assert!(state
            .explanation
            .contains("1 route still advertising capacity"));
        assert!(state
            .explanation
            .contains("model-b: load model weights failed: out of memory"));
    }

    #[test]
    fn machines_render_unreported_progress_as_indeterminate() {
        let state = GatewayState::from_models(Vec::new()).with_local_provider_id("provider-a");
        let mut data = DashboardData::from_state(&state);
        data.provider_load_progress.insert(
            ("provider-a".to_owned(), "model-a".to_owned()),
            DashboardProviderLoadProgress {
                provider: "provider-a".to_owned(),
                model_id: "model-a".to_owned(),
                label: "catalog artifact".to_owned(),
                phase: "download".to_owned(),
                status: "loading".to_owned(),
                updated_at_ms: now_millis_u64(),
                ..DashboardProviderLoadProgress::default()
            },
        );

        let html = earn_machines_page(&data, 60, None, None);
        assert!(html.contains("download progress not reported"));
        assert!(
            html.contains("<progress max=\"100\" aria-label=\"download progress not reported\">")
        );
        assert!(!html.contains("download 0%"));
    }

    #[test]
    fn unscoped_earn_metrics_and_empty_model_fit_are_explicit() {
        let state = GatewayState::from_models(Vec::new());
        let query = DashboardQuery::default();
        let home = render_dashboard_product_page(
            &state,
            60,
            "http://127.0.0.1:11435",
            &query,
            DashboardProductPage::Home,
        );
        assert!(home.contains("Final receipts"));
        assert!(!home.contains("Successful requests"));

        let earn = render_dashboard_product_page(
            &state,
            60,
            "http://127.0.0.1:11435",
            &query,
            DashboardProductPage::Earn,
        );
        assert!(earn.contains(
            "Live route metrics</span><span class=\"metric-state\">Fresh heartbeat</span></div><div class=\"metric-status\">"
        ));
        assert!(!earn.contains("<strong>Provider identity is not configured</strong>"));
        assert!(!earn.contains("<h2>Action center</h2>"));

        let opportunities = render_dashboard_product_page(
            &state,
            60,
            "http://127.0.0.1:11435",
            &query,
            DashboardProductPage::EarnOpportunities,
        );
        assert!(opportunities.contains("No catalog models available"));
        assert!(opportunities
            .contains("Catalog models, gateway-host compatibility, and advertised supply"));
    }

    #[test]
    fn settings_update_state_offers_a_copyable_resolution() {
        let state = GatewayState::from_models(Vec::new());
        let mut data = DashboardData::from_state(&state);
        data.update_notice = Some(GatewayUpdateNotice {
            level: "required".to_owned(),
            installed_app_version: "1.0.0".to_owned(),
            required_min_app_version: "2.0.0".to_owned(),
            affected_model_count: 1,
            hidden_model_count: 0,
            message: "Update required".to_owned(),
            models: Vec::new(),
        });

        let html = settings_page(&data, 60);
        assert!(html.contains("<code id=\"mayhem-update-stage-command\">mayhem update</code>"));
        assert!(html.contains("data-copy-target=\"#mayhem-update-stage-command\""));
        assert!(html.contains(
            "<code id=\"mayhem-update-apply-command\">mayhem update --apply-staged</code>"
        ));
        assert!(html.contains("data-copy-target=\"#mayhem-update-apply-command\""));
        assert!(html.contains("does not replace the installed binary"));
        assert!(html.contains("does not restart an already-running gateway service"));
    }

    #[test]
    fn wallet_funding_guides_are_runnable_and_explicit_about_submission() {
        let fiat = wallet_funding_guide("fiat");
        assert_eq!(fiat.command, "mayhem pay stripe --amount 10");
        assert!(fiat.help.contains("review"));

        let tap = wallet_funding_guide("tap");
        assert_eq!(tap.command, "mayhem pay tap --amount-tap 10");
        assert!(tap.help.contains("dry run"));
        assert!(tap.help.contains("--confirm"));

        let tnk = wallet_funding_guide("tnk");
        assert_eq!(tnk.command, "mayhem pay tnk --amount 10");
        assert!(tnk.help.contains("without submitting"));

        assert_eq!(wallet_funding_guide("unknown").command, "mayhem payments");
        assert_eq!(
            wallet_deposit_status_command("fiat"),
            "mayhem deposit status --rail fiat"
        );
        assert_eq!(
            wallet_deposit_status_command("tap"),
            "mayhem deposit status --rail tap"
        );
        assert_eq!(
            wallet_deposit_status_command("tnk"),
            "mayhem deposit status --rail tnk"
        );
        assert_eq!(
            wallet_deposit_status_command("unknown"),
            "mayhem deposit status --help"
        );

        let data = DashboardData::from_state(&GatewayState::from_models(Vec::new()));
        let wallet = wallet_page(&data, 60);
        assert!(wallet.contains("data-hide-amounts"));
        assert!(wallet.contains(
            r#"Start a <span data-money><span class="money-value">$10</span></span> Stripe checkout"#
        ));
        assert!(wallet.contains(
            r#"<code id="wallet-funding-command">mayhem pay stripe --amount <span data-money><span class="money-value">10</span></span></code>"#
        ));
    }

    #[test]
    fn table_regions_and_first_value_steps_expose_keyboard_and_screen_reader_state() {
        let sample = r#"<div class="data-table-wrap"><table><caption class="sr-only">Example records</caption><tbody></tbody></table></div>"#;
        let enhanced = keyboard_accessible_table_regions(sample);
        assert!(enhanced.contains(
            r#"class="data-table-wrap" role="region" tabindex="0" aria-label="Example records. Scroll horizontally to view all columns.""#
        ));

        let data = DashboardData::from_state(&GatewayState::from_models(Vec::new()));
        let checklist = activation_panel(&data);
        assert!(checklist.contains(r#"<ol class="checklist">"#));
        assert!(checklist.contains(r#"<li class="check-step done">"#));
        assert!(checklist.contains(r#"<span class="sr-only" data-check-state>Complete: </span>"#));
        assert!(checklist.contains(r#"<span class="sr-only" data-check-state>Current: </span>"#));
        assert!(
            checklist.contains(r#"<span class="sr-only" data-check-state>Not started: </span>"#)
        );
    }

    #[test]
    fn playground_no_script_explanation_precedes_and_hides_enhanced_fields() {
        let data = DashboardData::from_state(&GatewayState::fixture());
        let html = playground_page(&data, 60, None);
        let warning = html.find("<noscript>").expect("no-JavaScript warning");
        let enhanced = html
            .find(r#"<div class="playground-interactive js-only">"#)
            .expect("enhanced Playground wrapper");
        let form = html.find("data-playground-form").expect("Playground form");
        assert!(warning < enhanced);
        assert!(enhanced < form);
        assert!(html.contains("before entering a prompt or access token"));
    }

    #[test]
    fn playground_request_controls_match_gateway_routing_contracts() {
        let data = DashboardData::from_state(&GatewayState::fixture());
        let html = playground_page(&data, 60, None);

        assert!(html.contains("data-playground-max-tokens"));
        assert!(html.contains(r#"min="64" max="4096""#));
        assert!(html.contains("data-playground-max-price"));
        assert!(html.contains(r#"data-price-mode="rate""#));
        assert!(html.contains("data-money-input"));
        assert!(html.contains("Route price ceiling"));
        assert!(html.contains("data-playground-min-att-tier"));
        assert!(html.contains(r#"<option value="4">T4 only"#));
        assert!(html.contains("Numeric identity tier does not promise confidential compute."));
        assert!(html.contains("data-playground-request-summary"));
        assert!(html.contains("drafts and history stay in this browser tab"));
        assert!(html.contains("access tokens are never saved"));
        assert!(html.contains("data-playground-reset-draft"));
    }

    #[test]
    fn playground_image_sizes_follow_the_selected_models_signed_constraints() {
        let mut model = GatewayState::fixture().models_snapshot()[0].clone();
        model.id = "test/signed-image".to_owned();
        model.mayhem.model_class = "image-generation".to_owned();
        model.mayhem.caps.image = true;
        model.mayhem.caps.max_image_width = Some(2_048);
        model.mayhem.caps.max_image_height = Some(2_048);
        model.mayhem.caps.output_modality = Some("image".to_owned());
        model.mayhem.caps.output_modalities = vec!["image".to_owned()];
        let mut contract = mayhem_proto::endpoint_family_contract_template(
            mayhem_proto::ENDPOINT_OPENAI_IMAGE_GENERATIONS,
        )
        .expect("OpenAI image endpoint contract");
        for path in ["width", "height"] {
            let spec = contract.request_attribute_specs.get_mut(path).unwrap();
            spec.minimum = Some(576.0);
            spec.maximum = Some(2_048.0);
            spec.multiple_of = Some(16.0);
        }
        model.mayhem.adapter.endpoint_families = vec![contract.clone()];

        let config = playground_image_request_config(&model).expect("image request config");
        assert_eq!(config.dimension_mode, "size");
        assert_eq!(config.sizes["1:1"], "576x576");
        assert_eq!(config.sizes["4:3"], "768x576");
        assert_eq!(config.sizes["3:4"], "576x768");
        assert_eq!(config.sizes["16:9"], "1024x576");
        assert!(!config.sizes.values().any(|size| size == "512x512"));

        let data = DashboardData::from_state(&GatewayState::from_models(vec![model.clone()]));
        let html = playground_page(&data, 60, Some(&model.id));
        assert!(html.contains(r#"data-image-dimension-mode="size""#));
        assert!(html.contains(r#"&quot;1:1&quot;:&quot;576x576&quot;"#));
        assert!(html.contains("data-playground-image-size"));

        contract.request_attributes.retain(|path| path != "size");
        contract.request_attribute_specs.remove("size");
        model.mayhem.adapter.endpoint_families = vec![contract];
        let dimensions =
            playground_image_request_config(&model).expect("width-height image request config");
        assert_eq!(dimensions.dimension_mode, "width-height");
        assert_eq!(dimensions.sizes["1:1"], "576x576");
    }

    #[test]
    fn playground_marks_fixed_only_price_schedules_without_reclassifying_mixed_rates() {
        let mut fixed_model = GatewayState::fixture().models_snapshot()[0].clone();
        fixed_model.mayhem.price_ref_au.rate_map.clear();
        fixed_model.mayhem.price_ref_au.per_req_au = AU_PER_USD;
        fixed_model.mayhem.price_ref_au.min_session_au = 2 * AU_PER_USD;
        let fixed_state = GatewayState::from_models(vec![fixed_model]);
        let fixed_html = playground_page(&DashboardData::from_state(&fixed_state), 60, None);
        assert!(fixed_html.contains(r#"data-price-mode="fixed""#));

        let mut mixed_model = GatewayState::fixture().models_snapshot()[0].clone();
        mixed_model.mayhem.price_ref_au.per_req_au = AU_PER_USD;
        mixed_model.mayhem.price_ref_au.min_session_au = 2 * AU_PER_USD;
        assert!(!mixed_model.mayhem.price_ref_au.rate_map.is_empty());
        let mixed_state = GatewayState::from_models(vec![mixed_model]);
        let mixed_html = playground_page(&DashboardData::from_state(&mixed_state), 60, None);
        assert!(mixed_html.contains(r#"data-price-mode="rate""#));
        assert!(!mixed_html.contains(r#"data-price-mode="fixed""#));
    }

    #[test]
    fn playground_direct_empty_states_continue_the_page_heading_hierarchy() {
        let mut credential_required = DashboardData::from_state(&GatewayState::fixture());
        credential_required.access = json!({
            "require_auth": true,
            "active_token_count": 0,
        });
        let auth_html = playground_page(&credential_required, 60, None);
        assert!(auth_html.contains("<h1>Playground</h1>"));
        assert!(auth_html.contains("<h2>Create an access token first</h2>"));
        assert!(!auth_html.contains("<h3>Create an access token first</h3>"));

        let no_models = DashboardData::from_state(&GatewayState::from_models(Vec::new()));
        let empty_html = playground_page(&no_models, 60, None);
        assert!(empty_html.contains("<h1>Playground</h1>"));
        assert!(empty_html.contains("<h2>No compatible models available</h2>"));
        assert!(!empty_html.contains("<h3>No compatible models available</h3>"));
    }

    #[test]
    fn adaptive_home_secondary_panel_scenarios_use_completed_work_and_provider_state() {
        let scenarios = [
            (
                "non-final-only history still has zero completed requests",
                0,
                None,
                false,
                false,
                HomeSecondaryPanel::FirstValue,
            ),
            (
                "completed request with no provider role",
                1,
                None,
                false,
                false,
                HomeSecondaryPanel::None,
            ),
            (
                "active configured provider before a first request",
                0,
                Some(RouteStateKind::Accepting),
                false,
                true,
                HomeSecondaryPanel::Provider,
            ),
            (
                "provider state needs attention before a first request",
                0,
                Some(RouteStateKind::Stale),
                false,
                true,
                HomeSecondaryPanel::Provider,
            ),
            (
                "provider preparation needs attention before route evidence",
                0,
                None,
                true,
                false,
                HomeSecondaryPanel::Provider,
            ),
        ];

        for (name, completed, state, progress_attention, evidence, expected) in scenarios {
            assert_eq!(
                choose_home_secondary_panel(completed, state, progress_attention, evidence),
                expected,
                "{name}"
            );
        }

        let user_only = DashboardData::from_state(&GatewayState::from_models(Vec::new()));
        let user_html = home_page(&user_only, 60);
        assert!(user_html.contains("<h2>Getting started</h2>"));

        let configured = GatewayState::from_models(Vec::new()).with_local_provider_id("provider-a");
        let provider_data = DashboardData::from_state(&configured);
        assert_eq!(provider_data.completed_requests(), 0);
        let provider_html = home_page(&provider_data, 60);
        assert!(provider_html.contains("<h2>Your provider</h2>"));
        assert!(!provider_html.contains("<h2>Getting started</h2>"));
    }

    #[test]
    fn provider_progress_and_failure_remain_visible_on_adaptive_home() {
        let state = GatewayState::from_models(Vec::new()).with_local_provider_id("provider-a");
        let mut data = DashboardData::from_state(&state);
        data.provider_load_progress.insert(
            ("provider-a".to_owned(), "model-a".to_owned()),
            DashboardProviderLoadProgress {
                provider: "provider-a".to_owned(),
                model_id: "model-a".to_owned(),
                label: "verify catalog artifact".to_owned(),
                phase: "verify".to_owned(),
                status: "error".to_owned(),
                error: Some("artifact signature mismatch".to_owned()),
                updated_at_ms: now_millis_u64(),
                ..DashboardProviderLoadProgress::default()
            },
        );

        let html = home_page(&data, 60);
        assert!(html.contains("Your provider"));
        assert!(html.contains("Setup blocked by model failure"));
        assert!(html.contains("artifact signature mismatch"));
        assert!(!html.contains("<h2>Getting started</h2>"));
    }

    #[test]
    fn model_fit_keeps_its_evidence_limit_in_one_visible_consequence_statement() {
        let data = DashboardData::from_state(&GatewayState::fixture());
        let html = earn_opportunities_page(&data, 60, None, None);

        assert!(html.contains("<strong>Gateway-host evidence only.</strong>"));
        assert_eq!(html.matches("remote worker").count(), 1);
        assert_eq!(html.matches("demand, revenue, or earnings").count(), 1);
        assert!(html.contains("Catalog compatibility beside current advertised supply."));
        assert!(html.contains(
            "Which catalog models this machine can run, next to what the network currently supplies."
        ));
        assert!(!html.contains("Use this as a host compatibility reference"));
        assert!(!html.contains("without inventing demand or revenue signals"));
    }

    #[test]
    fn model_rows_disclose_compact_capability_and_price_summaries() {
        let mut model = GatewayState::fixture().models_snapshot()[0].clone();
        let model_id = model.id.clone();
        let readable_price = dashboard_model_price(&model);
        assert!(readable_price.contains(" / 1M input tokens"));
        assert!(!readable_price.contains("input_token"));
        model.mayhem.caps.output_modalities = vec![
            "audio".to_owned(),
            "embedding".to_owned(),
            "image".to_owned(),
            "text".to_owned(),
            "video".to_owned(),
            "vision".to_owned(),
        ];
        let base_rate = model.mayhem.price_ref_au.rate_map[0].clone();
        model.mayhem.price_ref_au.rate_map = vec![base_rate; 5];
        let omitted_abilities = dashboard_model_abilities(&model)
            .into_iter()
            .filter(|value| !value.starts_with("api:"))
            .count()
            .saturating_sub(4);
        let exported_abilities = dashboard_model_abilities(&model)
            .into_iter()
            .filter(|value| !value.starts_with("api:"))
            .collect::<Vec<_>>()
            .join(" / ");
        assert!(omitted_abilities > 0);
        let state = GatewayState::from_models(vec![model]);
        let data = DashboardData::from_state(&state);
        let html = models_page(&data, 60, None);

        assert!(html.contains(&format!(
            r#"data-collapsed-label="+{omitted_abilities} more""#
        )));
        assert!(html.contains("+2 other rates"));
        assert!(html.contains(&format!(
            r#"data-export-value="{}" data-sort-value="{}""#,
            html_escape(&exported_abilities),
            html_escape(&exported_abilities),
        )));
        assert!(html.contains(r#"<th scope="row""#));
        assert!(html.contains("data-model-detail-open"));
        assert!(html.contains(&format!(r#"aria-label="Use {model_id} in Playground""#)));

        let mut fixed_only = GatewayState::fixture().models_snapshot()[0]
            .mayhem
            .price_ref_au
            .clone();
        fixed_only.rate_map.clear();
        fixed_only.per_req_au = AU_PER_USD;
        fixed_only.min_session_au = 2 * AU_PER_USD;
        assert_eq!(
            dashboard_price(&fixed_only),
            "$1.00 / request + $2.00 minimum / session"
        );

        let base_rate = GatewayState::fixture().models_snapshot()[0]
            .mayhem
            .price_ref_au
            .rate_map[0]
            .clone();
        fixed_only.rate_map = vec![base_rate; 5];
        assert!(dashboard_price(&fixed_only).contains("(+4 more rates)"));
    }

    #[test]
    fn market_export_keeps_full_model_and_enclave_identifiers() {
        let mut model = GatewayState::fixture().models_snapshot()[0].clone();
        model.id = "model/export-identifier-that-must-remain-exact".to_owned();
        let enclave = "enclave-export-identifier-that-is-longer-than-the-visible-summary";
        model.mayhem.markets = vec![GatewayMarketInfo {
            enclave_id: enclave.to_owned(),
            att_tier: 3,
            quant: "int4".to_owned(),
            ctx_bracket: Some("base".to_owned()),
            room_ids: vec!["room-export".to_owned()],
            providers_online: 1,
            route_count: 1,
            availability: "routable".to_owned(),
            price_ref_au: model.mayhem.price_ref_au.clone(),
        }];
        let mut data = DashboardData::from_state(&GatewayState::fixture());
        data.models = Arc::new(vec![model.clone()]);
        let html = network_markets_page(&data, 60, None);

        assert!(html.contains(&format!(
            r#"data-export-value="{} / enclave {}""#,
            model.id, enclave,
        )));
        assert!(html.contains(&format!(r#"data-sort-value="{} / {}""#, model.id, enclave,)));
        assert!(html.contains("enclave-export-..."));
    }

    #[test]
    fn blocked_provider_state_does_not_duplicate_the_shared_update_action() {
        let blocked = RouteState {
            kind: RouteStateKind::Blocked,
            label: "Blocked by update",
            tone: "danger",
            explanation: "Update required".to_owned(),
        };

        assert!(provider_action_center(&blocked).is_empty());

        let loading = RouteState {
            kind: RouteStateKind::Waiting,
            label: "Preparing a model",
            tone: "",
            explanation: "download is 50% complete".to_owned(),
        };
        assert!(provider_action_center(&loading).is_empty());

        let offline = RouteState {
            kind: RouteStateKind::Waiting,
            label: "Waiting for first heartbeat",
            tone: "warn",
            explanation: "No fresh heartbeat".to_owned(),
        };
        let offline_action = provider_action_center(&offline);
        assert!(offline_action.contains("Restore the worker heartbeat"));
        assert!(offline_action.contains("Start or reconnect the provider worker"));

        let unconfigured = RouteState {
            kind: RouteStateKind::Waiting,
            label: "Provider identity not configured",
            tone: "warn",
            explanation: "No provider identity".to_owned(),
        };
        let identity_action = provider_action_center(&unconfigured);
        assert!(identity_action.contains("Start provider setup"));
        assert!(identity_action.contains("mayhem provider start --help"));
        assert!(identity_action.contains("dashboard remains read-only"));
    }

    #[test]
    fn empty_tables_do_not_render_dead_subset_filters() {
        let data = DashboardData::from_state(&GatewayState::from_models(Vec::new()));
        for html in [
            models_page(&data, 60, None),
            activity_page(&data, 60, None),
            network_providers_page(&data, 60, None),
            network_markets_page(&data, 60, None),
        ] {
            assert!(!html.contains("data-table-filter"));
            assert!(!html.contains("Filter shown page"));
            assert!(!html.contains("No matching shown"));
        }
    }

    #[test]
    fn shown_page_tools_label_scope_and_support_independent_query_state() {
        let (controls, empty) = shown_rows_filter_scoped(
            "probes",
            "probe-table",
            "Filter probes on the shown page",
            30,
            "probes",
            Some("probe"),
        );
        assert!(controls.contains(">Filter shown page</label>"));
        assert!(controls.contains(r##"data-table-filter="#probe-table""##));
        assert!(controls.contains(r#"data-table-query-prefix="probe""#));
        assert!(empty.contains("Rows on other pages are not searched."));
    }

    #[test]
    fn connect_page_reloads_tokens_created_and_revoked_after_startup() {
        let path = std::env::temp_dir().join(format!(
            "mayhem-dashboard-token-refresh-{}-{}.json",
            std::process::id(),
            now_millis_u64()
        ));
        let access =
            GatewayAccessControl::new(true, GatewayTokenStore::empty(), Some(path.clone()));
        let mut token = GatewayTokenRecord {
            name: "dashboard-new-token".to_owned(),
            token_hash: gateway_token_hash("dashboard-new-token-secret"),
            token_id: "tok_dashboard_new".to_owned(),
            created_at: now_secs(),
            expires_at: None,
            budget_au: Some(10 * AU_PER_USD),
            budget_period: Some(GatewayTokenBudgetPeriod::Total),
            spent_total_au: AU_PER_USD,
            spent_period_au: 0,
            period_started_at: None,
            max_rate_per_minute: None,
            models: Vec::new(),
            last_used_at: None,
            revoked_at: None,
        };
        let write_store = |token: &GatewayTokenRecord| {
            let store = GatewayTokenStore {
                version: 1,
                tokens: vec![token.clone()],
            };
            fs::write(&path, serde_json::to_vec_pretty(&store).unwrap()).unwrap();
        };
        write_store(&token);
        let state = GatewayState::from_models(Vec::new()).with_access_control(access);

        let current = connect_page(
            &DashboardData::from_state(&state),
            60,
            "http://127.0.0.1:11435",
            None,
        );
        assert!(current.contains("dashboard-new-token"));
        assert!(current.contains("1 active access token"));
        assert!(current.contains("$1.00 / $10.00"));
        assert!(current.contains("lifetime total"));

        token.revoked_at = Some(now_secs());
        write_store(&token);
        let revoked = connect_page(
            &DashboardData::from_state(&state),
            60,
            "http://127.0.0.1:11435",
            None,
        );
        assert!(revoked.contains("0 active access tokens"));
        assert!(revoked.contains(">Inactive<"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn bounded_row_summaries_disclose_truncation() {
        assert_eq!(
            bounded_rows_summary(75, 60, "routes"),
            "Showing first 60 of 75 routes."
        );
        assert_eq!(bounded_rows_summary(12, 60, "routes"), "Showing 12 routes.");
    }

    #[test]
    fn page_query_is_one_based_clamped_and_accessibly_linked() {
        let first = PageWindow::from_query(96, 80, None);
        assert_eq!(
            first,
            PageWindow {
                current: 1,
                page_count: 2,
                start: 0,
                end: 80,
                total: 96,
            }
        );
        assert_eq!(
            first.status("catalog models"),
            "Showing rows 1&ndash;80 of 96 catalog models. Page 1 of 2."
        );
        let first_nav = pagination_nav(
            first,
            "/mayhem/dashboard/earn/opportunities",
            &[("provider", "provider a")],
            "catalog models",
        );
        assert!(first_nav.contains(r#"aria-label="catalog models pagination""#));
        assert!(first_nav.contains(r#"aria-disabled="true">Previous"#));
        assert!(first_nav.contains(r#"rel="next""#));
        assert!(first_nav.contains("?provider=provider%20a&amp;page=2"));

        let probe_nav = pagination_nav_with_param(
            PageWindow::from_query(64, 30, Some("3")),
            "/mayhem/dashboard/network/evidence",
            &[("page", "2")],
            "probe events",
            "probe_page",
        );
        assert!(probe_nav.contains(
            r#"rel="prev" href="/mayhem/dashboard/network/evidence?page=2&amp;probe_page=2""#
        ));
        assert!(!probe_nav.contains("page=2&amp;page="));

        let second = PageWindow::from_query(96, 80, Some("2"));
        assert_eq!((second.current, second.start, second.end), (2, 80, 96));
        let second_nav = pagination_nav(second, "/mayhem/dashboard/models", &[], "catalog models");
        assert!(second_nav.contains(r#"rel="prev" href="/mayhem/dashboard/models?page=1""#));
        assert!(second_nav.contains(r#"aria-disabled="true">Next"#));

        for invalid in ["", "0", "-1", "not-a-page"] {
            assert_eq!(
                PageWindow::from_query(96, 80, Some(invalid)).current,
                1,
                "{invalid:?} must clamp to the first page"
            );
        }
        assert_eq!(PageWindow::from_query(96, 80, Some("999")).current, 2);
        assert_eq!(PageWindow::from_query(0, 80, Some("999")).current, 1);
    }

    #[test]
    fn query_selected_provider_cannot_authorize_earnings() {
        let amount = 42_424_242_000_000_000_000_u128;
        let query = DashboardQuery {
            provider: Some("provider-from-url".to_owned()),
            ..DashboardQuery::default()
        };
        let unscoped = GatewayState::from_models(Vec::new())
            .with_provider_earnings(vec![earnings_fixture("provider-from-url", amount)]);
        let html = render_dashboard_product_page(
            &unscoped,
            60,
            "http://127.0.0.1:11435",
            &query,
            DashboardProductPage::EarnEarnings,
        );
        assert!(html.contains("Provider query ignored"));
        assert!(html.contains("Configure a gateway provider identity before showing earnings."));
        assert!(!html.contains(&format_au_usd(amount)));

        let scoped = GatewayState::from_models(Vec::new())
            .with_provider_earnings(vec![earnings_fixture("provider-from-url", amount)])
            .with_local_provider_id("provider-from-url");
        let html = render_dashboard_product_page(
            &scoped,
            60,
            "http://127.0.0.1:11435",
            &query,
            DashboardProductPage::EarnEarnings,
        );
        assert!(html.contains("Provider identity:"));
        assert!(!html.contains("Configured gateway identity"));
        assert!(!html.contains(r#"class="notice good"><strong>Provider identity"#));
        assert!(html.contains(&format_au_usd(amount)));
    }

    #[test]
    fn evidence_is_linked_on_demand_and_money_stays_identity_scoped() {
        let state = GatewayState::fixture();
        let model_id = state.models_snapshot()[0].id.clone();
        let model_query = DashboardQuery {
            kind: Some("model".to_owned()),
            id: Some(model_id.clone()),
            ..DashboardQuery::default()
        };
        let payload = dashboard_evidence_payload(&state, &model_query).expect("model evidence");
        assert_eq!(
            payload.get("title").and_then(Value::as_str),
            Some("Model evidence")
        );
        assert_eq!(
            payload
                .get("raw")
                .and_then(|raw| raw.get("catalog_model"))
                .and_then(|model| model.get("id"))
                .and_then(Value::as_str),
            Some(model_id.as_str())
        );

        let html = render_dashboard_product_page(
            &state,
            60,
            "http://127.0.0.1:11435",
            &DashboardQuery::default(),
            DashboardProductPage::Models,
        );
        assert!(html.contains("data-evidence-url"));
        assert!(html.contains("/mayhem/dashboard/evidence?kind=model&amp;id="));
        assert_eq!(html.matches("id=\"dashboard-evidence-dialog\"").count(), 1);
        assert_eq!(html.matches("id=\"model-detail-dialog\"").count(), 1);
        assert_eq!(html.matches("<dialog").count(), 2);

        let amount = 123_000_000_000_000_000_u128;
        let earnings_query = DashboardQuery {
            kind: Some("earning".to_owned()),
            rail: Some("fiat".to_owned()),
            ..DashboardQuery::default()
        };
        let unscoped = GatewayState::from_models(Vec::new())
            .with_provider_earnings(vec![earnings_fixture("provider-a", amount)]);
        assert!(dashboard_evidence_payload(&unscoped, &earnings_query).is_none());
        let scoped = unscoped.with_local_provider_id("provider-a");
        let payload =
            dashboard_evidence_payload(&scoped, &earnings_query).expect("scoped earnings evidence");
        assert_eq!(
            payload
                .get("raw")
                .and_then(|raw| raw.get("provider"))
                .and_then(Value::as_str),
            Some("provider-a")
        );
    }
}
