//! Resumable, read-only ranking over a frozen indexed scope. Continuations are
//! server-owned; a caller cursor can never attest to skipped candidates.
use super::*;
use axum::{body::to_bytes, extract::Request as HttpRequest};
use mayhem_proto::MoneyAu;
use mayhem_proxy::{
    catalog::CatalogRead,
    directory::PublishedOffer,
    financial::quote::{Maximum, PurchaseRequest},
    presence::Eligibility,
    registry::publication::taxonomy,
    routing::{Continuity, Ranking, Target},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    time::Instant,
};

const DEADLINE: Duration = Duration::from_secs(30);
static CPU: Semaphore = Semaphore::const_new(4);
mod pricing;
use pricing::RetailRanking;
#[cfg(test)]
mod progress_tests;

/// Operator resource budgets, not customer concurrency or financial limits.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub retained_sessions: usize,
    pub retained_bytes: usize,
    pub active_steps: usize,
    pub candidates_per_step: usize,
    pub ttl_ms: u64,
    pub observation_timeout_ms: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            retained_sessions: 64,
            retained_bytes: 256 * 1024 * 1024,
            active_steps: 4,
            candidates_per_step: 4,
            ttl_ms: 600_000,
            observation_timeout_ms: 15_000,
        }
    }
}
impl Limits {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=1024).contains(&self.retained_sessions)
            || self.retained_bytes == 0
            || self.retained_bytes as u128 > 4u128 * 1024 * 1024 * 1024
            || !(1..=16).contains(&self.active_steps)
            || !(1..=16).contains(&self.candidates_per_step)
            || !(1000..=3_600_000).contains(&self.ttl_ms)
            || !(1..=60_000).contains(&self.observation_timeout_ms)
        {
            return Err("invalid proxy resolver resource budgets".into());
        }
        Ok(())
    }
}
pub(super) struct Store {
    limits: Limits,
    entries: Mutex<BTreeMap<String, Arc<Entry>>>,
    reads: Arc<Semaphore>,
}
struct Entry {
    owner: String,
    created: Instant,
    bytes: usize,
    session: Arc<tokio::sync::Mutex<Session>>,
}
impl Store {
    pub(super) fn new(limits: Limits) -> Result<Self, String> {
        limits.validate()?;
        Ok(Self {
            reads: Arc::new(Semaphore::new(limits.active_steps)),
            limits,
            entries: Mutex::new(BTreeMap::new()),
        })
    }
    pub(super) fn expire(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            for entry in entries.values() {
                if let Ok(mut session) = entry.session.try_lock() {
                    if session
                        .lease_until
                        .is_some_and(|until| Instant::now() >= until)
                    {
                        session.waiting = None;
                        session.lease_until = None;
                    }
                }
            }
            entries.retain(|_, e| e.created.elapsed() < Duration::from_millis(self.limits.ttl_ms));
        }
    }
    pub(super) fn clear(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Envelope {
    Start {
        schema_version: u32,
        endpoint: ProxyEndpoint,
        request: Value,
        #[serde(deserialize_with = "nullable")]
        previous_model: Option<String>,
        #[serde(default)]
        model_allowlist: Option<Vec<String>>,
        #[serde(default)]
        retail_ranking: Option<RetailRanking>,
    },
    Continue {
        schema_version: u32,
        resolution_id: String,
        revision: u64,
    },
}
fn nullable<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}
struct Session {
    id: String,
    revision: u64,
    last: Option<(u64, Value)>,
    complete: bool,
    started_at_ms: u64,
    expires_at_ms: u64,
    models: Vec<String>,
    model_allowlist: Option<Vec<String>>,
    model_allowlist_digest: String,
    request_content_digest: String,
    retail_ranking: Option<RetailRanking>,
    endpoint: ProxyEndpoint,
    body: Value,
    controls: proxy_request::Controls,
    previous: Option<String>,
    previous_checked: bool,
    snapshot: String,
    query_key: Option<String>,
    catalog: Option<Arc<CatalogRead>>,
    cursor: Option<String>,
    considered: u64,
    scanned: u64,
    index_reads: u64,
    exclusions: BTreeMap<&'static str, u64>,
    unknown: u64,
    best: Option<Best>,
    page: VecDeque<PublishedOffer>,
    traversal_done: bool,
    taxonomy: Option<TaxonomyTraversal>,
    waiting: Option<Waiting>,
    lease_until: Option<Instant>,
    pending_reason: Option<&'static str>,
}
struct TaxonomyTraversal {
    pin: taxonomy::Pinned,
    reference: taxonomy::Reference,
    category: Option<taxonomy::DocumentReference>,
    cursor: Option<String>,
    scopes: VecDeque<taxonomy::Scope>,
    current: Option<taxonomy::Scope>,
    query: Option<String>,
    exhausted: bool,
}
struct Waiting {
    model: String,
    started: Instant,
    lease: Option<mayhem_proxy::presence::gateway::ObservationLease>,
}
struct Best {
    model: String,
    body: Value,
    request: Arc<proxy_request::Request>,
    maximum: Maximum,
    published: PublishedOffer,
    score: MoneyAu,
    maximum_retail_cost_micro: Option<MoneyAu>,
    evidence: Option<mayhem_proxy::conformance::Signed>,
    declaration: Option<super::data_handling::Observation>,
}
enum Checked {
    Ready(Best),
    Excluded(&'static str, bool),
    Pending(&'static str),
}
fn error(code: &'static str, unavailable: bool) -> ApiError {
    let e = if unavailable {
        ApiError::service_unavailable(
            "Proxy profile resolution needs another bounded read or a fresh start",
            Some("proxy.profile"),
        )
    } else {
        ApiError::bad_request(
            "Invalid proxy profile resolution request",
            Some("proxy.profile"),
        )
    };
    e.with_public_error(code, "proxy_profile_resolution", unavailable)
}
fn invalid() -> ApiError {
    error("proxy_profile_resolution_invalid", false)
}
fn unavailable() -> ApiError {
    error("proxy_profile_resolution_unavailable", true)
}
fn busy() -> ApiError {
    error("proxy_profile_resolution_busy", true)
}

pub(crate) async fn handle(State(state): State<SharedState>, http: HttpRequest) -> Response {
    let result = read(state, http).await;
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    response
        .headers_mut()
        .insert("cache-control", "private, no-store".parse().unwrap());
    response
}
async fn read(state: SharedState, http: HttpRequest) -> Result<Response, ApiError> {
    let headers = http.headers().clone();
    let raw = super::super::gateway_bearer_token(&headers)?.ok_or_else(|| {
        ApiError::unauthorized(
            "Proxy resolution requires an authenticated key",
            Some("Authorization"),
        )
    })?;
    state
        .access_control
        .preauthorize_body_headers_mode(&headers, false)?;
    let actor = state
        .authorize_existing_gateway_request(&headers, None)?
        .ok_or_else(|| {
            ApiError::unauthorized(
                "Proxy resolution requires an authenticated key",
                Some("Authorization"),
            )
        })?;
    let owner = super::super::gateway_token_hash(&raw);
    let models = {
        let store = state
            .access_control
            .store
            .lock()
            .map_err(|_| unavailable())?;
        let token = store
            .tokens
            .iter()
            .find(|t| t.token_id == actor.token_id && t.token_hash == owner)
            .ok_or_else(unavailable)?;
        let mut models = token.models.clone();
        models.sort();
        models.dedup();
        models
    };
    let runtime = state
        .proxy_buyer
        .clone()
        .ok_or_else(|| error("proxy_buyer_disabled", true))?;
    if !runtime.running.load(Ordering::Acquire) {
        return Err(unavailable());
    }
    let permit = runtime
        .resolver
        .reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| busy())?;
    let limit = runtime
        .policy
        .request_byte_limit()
        .saturating_add(98 * 1024);
    let bytes = tokio::time::timeout(DEADLINE, to_bytes(http.into_body(), limit))
        .await
        .map_err(|_| invalid())?
        .map_err(|_| invalid())?;
    let envelope: Envelope = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let (entry, revision) = match envelope {
        Envelope::Start {
            schema_version: 1,
            endpoint,
            request,
            previous_model,
            model_allowlist,
            retail_ranking,
        } => {
            let session = tokio::time::timeout(
                DEADLINE,
                start(
                    &state,
                    &runtime,
                    endpoint,
                    request,
                    previous_model,
                    models.clone(),
                    model_allowlist,
                    retail_ranking,
                ),
            )
            .await
            .map_err(|_| unavailable())??;
            // Covers frozen request, materialized winner, saved reply, and bounded
            // public descriptor/page metadata. This is serialized state accounting.
            let retained = bytes
                .len()
                .checked_mul(6)
                .and_then(|n| n.checked_add(1024 * 1024))
                .ok_or_else(invalid)?;
            let entry = Arc::new(Entry {
                owner,
                created: Instant::now(),
                bytes: retained,
                session: Arc::new(tokio::sync::Mutex::new(session)),
            });
            runtime.resolver.expire();
            let mut entries = runtime.resolver.entries.lock().map_err(|_| unavailable())?;
            if entries.len() >= runtime.resolver.limits.retained_sessions
                || entries
                    .values()
                    .try_fold(retained, |n, e| n.checked_add(e.bytes))
                    .is_none_or(|n| n > runtime.resolver.limits.retained_bytes)
            {
                return Err(busy());
            }
            let id = entry.session.try_lock().map_err(|_| busy())?.id.clone();
            entries.insert(id, entry.clone());
            (entry, 0)
        }
        Envelope::Continue {
            schema_version: 1,
            resolution_id,
            revision,
        } => {
            if Digest::new(&resolution_id).is_err() {
                return Err(invalid());
            }
            runtime.resolver.expire();
            let entries = runtime.resolver.entries.lock().map_err(|_| unavailable())?;
            let entry = entries
                .get(&resolution_id)
                .filter(|e| e.owner == owner)
                .cloned()
                .ok_or_else(|| error("proxy_profile_resolution_expired", true))?;
            (entry, revision)
        }
        _ => return Err(invalid()),
    };
    let mut session = entry.session.clone().try_lock_owned().map_err(|_| busy())?;
    if session.models != models {
        return Err(error("proxy_profile_resolution_permissions_changed", true));
    }
    if let Some((previous, response)) = &session.last {
        if revision == *previous {
            return Ok(Json(response.clone()).into_response());
        }
    }
    if session.complete || revision != session.revision {
        return Err(error("proxy_profile_resolution_revision", false));
    }
    // Bounded owned work completes its progression even when the HTTP caller
    // disconnects. Replaying the same input revision returns its retained reply.
    let (send, receive) = oneshot::channel();
    tokio::spawn(async move {
        let _permit = permit;
        let result = tokio::time::timeout(DEADLINE, advance(&state, &runtime, &mut session)).await;
        let response = match result {
            Ok(Ok(value)) => value,
            failure => {
                session.exclude(
                    if failure.is_err() {
                        "step_deadline"
                    } else {
                        "step_unavailable"
                    },
                    true,
                );
                session.complete = true;
                session.catalog = None;
                session.waiting = None;
                status(&session, "refresh_required", false, None)
            }
        };
        session.revision += 1;
        let mut response = response;
        response["revision"] = serde_json::json!(session.revision);
        response["continuation"] = if session.complete {
            Value::Null
        } else {
            serde_json::json!({"schema_version":1,"kind":"continue","resolution_id":session.id,"revision":session.revision})
        };
        session.last = Some((revision, response.clone()));
        let _ = send.send(response);
    });
    Ok(Json(receive.await.map_err(|_| unavailable())?).into_response())
}

async fn start(
    state: &SharedState,
    runtime: &Arc<Runtime>,
    endpoint: ProxyEndpoint,
    mut body: Value,
    previous: Option<String>,
    models: Vec<String>,
    mut model_allowlist: Option<Vec<String>>,
    retail_ranking: Option<RetailRanking>,
) -> Result<Session, ApiError> {
    let request_content_digest = retail::request_content_digest(&body).map_err(|_| invalid())?;
    if let Some(list) = &mut model_allowlist {
        if list.len() > 200 {
            return Err(invalid());
        }
        for model in list.iter() {
            proxy_request::Selector::parse(model)
                .map_err(|_| invalid())?
                .ok_or_else(invalid)?;
        }
        list.sort();
        list.dedup();
    }
    let model_allowlist_digest =
        retail::request_content_digest(&serde_json::json!(model_allowlist))
            .map_err(|_| invalid())?;
    let object = body.as_object_mut().ok_or_else(invalid)?;
    if object.contains_key("model") {
        return Err(invalid());
    }
    let raw = object.remove("proxy").ok_or_else(invalid)?;
    if serde_json::to_vec(&raw).map_err(|_| invalid())?.len() > 32 * 1024 {
        return Err(invalid());
    }
    let mut controls: proxy_request::Controls =
        serde_json::from_value(raw).map_err(|_| invalid())?;
    let profile = controls.profile.as_ref().ok_or_else(invalid)?;
    profile.validate().map_err(|_| invalid())?;
    if retail_ranking
        .as_ref()
        .is_some_and(|p| !p.valid(profile.max_retail_cost_micro))
    {
        return Err(invalid());
    }
    if profile.endpoint != endpoint
        || !profile.allowed_rails.contains(&controls.rail)
        || !profile.settlement_policies.iter().any(|p| {
            p.rail == controls.rail
                && p.settlement_policy_hash == controls.settlement_policy_hash.as_str()
        })
        || controls.prices.max_total_spend_au == 0
        || controls.prices.max_total_spend_au > profile.prices.max_total_spend_au
        || controls.prices.rates.is_empty()
        || controls.prices.rates.len() > 32
        || controls.minimum_context == Some(0)
        || controls.minimum_tokens_per_second == Some(0)
        || controls
            .output_units
            .is_some_and(|n| n == 0 || n > mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER)
        || profile
            .constraints
            .minimum_context
            .is_some_and(|n| controls.minimum_context.unwrap_or(0) < n)
        || profile
            .constraints
            .minimum_tokens_per_second
            .is_some_and(|n| controls.minimum_tokens_per_second.unwrap_or(0) < n)
        || profile
            .constraints
            .output_units
            .is_some_and(|n| controls.output_units.is_none_or(|v| v > n))
        || (endpoint == ProxyEndpoint::Decisions
            && (controls.output_units.is_some() || controls.minimum_tokens_per_second.is_some()))
        || (endpoint != ProxyEndpoint::Decisions && controls.output_units.is_none())
    {
        return Err(invalid());
    }
    if controls.settlement_policy_hash != *runtime.policy.settlement_policy_hash() {
        return Err(error("proxy_settlement_policy_mismatch", false));
    }
    if super::evidence::unsupported(profile) {
        return Err(error("proxy_profile_evidence_unavailable", true));
    }
    if matches!(profile.ranking, Ranking::PreferredSpeed)
        && (endpoint == ProxyEndpoint::Decisions
            || body["stream"] != true
            || state
                .proxy_control()
                .and_then(|c| c.conformance())
                .is_none())
    {
        return Err(error("proxy_profile_speed_ranking_unavailable", true));
    }
    if let Some(model) = &previous {
        proxy_request::Selector::parse(model)
            .map_err(|_| invalid())?
            .ok_or_else(invalid)?;
    }
    if serde_json::to_vec(&body).map_err(|_| invalid())?.len() > runtime.policy.request_byte_limit()
    {
        return Err(invalid());
    }
    let control = state.proxy_control().cloned().ok_or_else(unavailable)?;
    if !profile.constraints.request_controls.is_empty()
        || !profile.constraints.capabilities.is_empty()
        || !profile.constraints.data_handling.is_empty()
    {
        let reader = control
            .registry()
            .ok_or_else(|| error("proxy_profile_evidence_unavailable", true))?;
        let pin = match &controls.registry_release {
            Some(r) => {
                let pin = reader
                    .pin_release(&r.release_id)
                    .await
                    .map_err(|_| unavailable())?;
                if pin.metadata().release_hash != r.release_hash.as_str() {
                    return Err(invalid());
                }
                pin
            }
            None => reader.current().await.map_err(|_| unavailable())?,
        };
        controls.registry_release = Some(proxy_request::RegistryRelease {
            release_id: pin.metadata().release_id.clone(),
            release_hash: Digest::new(&pin.metadata().release_hash).map_err(|_| unavailable())?,
        });
    } else if controls.registry_release.is_some() {
        return Err(invalid());
    }
    let permit = CPU.try_acquire().map_err(|_| busy())?;
    let catalog = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let read = control.catalog().read().map_err(|_| unavailable())?;
        if !read.status().discovery_is_fresh(
            super::super::now_millis_u64(),
            mayhem_proxy::presence::CATALOG_AGE_MS,
        ) {
            return Err(unavailable());
        }
        Ok::<_, ApiError>(Arc::new(read))
    })
    .await
    .map_err(|_| unavailable())??;
    let taxonomy = if let Target::TaxonomyCategory { taxonomy, .. } =
        &controls.profile.as_ref().unwrap().target
    {
        let control = state.proxy_control().ok_or_else(unavailable)?;
        let reader = control.registry().ok_or_else(unavailable)?;
        let pin = reader
            .pin_taxonomy(taxonomy)
            .await
            .map_err(|_| unavailable())?;
        let network = catalog
            .status()
            .committed
            .ok_or_else(unavailable)?
            .context
            .identity();
        pin.check_network(&network).map_err(|_| invalid())?;
        Some(TaxonomyTraversal {
            pin,
            reference: taxonomy.clone(),
            category: None,
            cursor: None,
            scopes: VecDeque::new(),
            current: None,
            query: None,
            exhausted: false,
        })
    } else {
        None
    };
    let query_key = if taxonomy.is_some() {
        Some(
            digest(
                "mayhem/proxy/taxonomy-candidates/v1",
                &[&mayhem_proto::stable_json_bytes(
                    &serde_json::json!({"profile":controls.profile,"rail":controls.rail}),
                )
                .map_err(|_| invalid())?],
            )
            .as_str()
            .to_owned(),
        )
    } else {
        None
    };
    let started_at_ms = super::super::now_millis_u64();
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|_| unavailable())?;
    let id = digest("mayhem/proxy/profile-resolution-id/v1", &[&random])
        .as_str()
        .to_owned();
    Ok(Session {
        id,
        revision: 0,
        last: None,
        complete: false,
        started_at_ms,
        expires_at_ms: started_at_ms.saturating_add(runtime.resolver.limits.ttl_ms),
        models,
        model_allowlist,
        model_allowlist_digest,
        request_content_digest,
        retail_ranking,
        endpoint,
        body,
        controls,
        previous,
        previous_checked: false,
        snapshot: catalog.status().content_snapshot,
        query_key,
        catalog: Some(catalog),
        cursor: None,
        considered: 0,
        scanned: 0,
        index_reads: 0,
        exclusions: BTreeMap::new(),
        unknown: 0,
        best: None,
        page: VecDeque::new(),
        traversal_done: false,
        taxonomy,
        waiting: None,
        lease_until: None,
        pending_reason: None,
    })
}

impl Session {
    fn exclude(&mut self, reason: &'static str, unknown: bool) {
        *self.exclusions.entry(reason).or_default() += 1;
        self.unknown += u64::from(unknown);
    }
    fn consider(&mut self, checked: Checked) {
        self.considered += 1;
        match checked {
            Checked::Pending(_) => unreachable!("pending evidence never consumes a candidate"),
            Checked::Excluded(code, unknown) => self.exclude(code, unknown),
            Checked::Ready(best) => {
                if matches!(
                    self.controls.profile.as_ref().unwrap().ranking,
                    Ranking::PreferredSpeed
                ) {
                    if let (Some(a), Some(b)) = (
                        best.evidence.as_ref(),
                        self.best.as_ref().and_then(|b| b.evidence.as_ref()),
                    ) {
                        if a.body.class != b.body.class
                            || !a
                                .body
                                .speed
                                .as_ref()
                                .unwrap()
                                .comparable(b.body.speed.as_ref().unwrap())
                        {
                            self.exclude("speed_evidence_incomparable", true);
                            return;
                        }
                    }
                }
                if let Some(record) = &best.declaration {
                    self.expires_at_ms = self.expires_at_ms.min(record.expires_at_ms);
                }
                if let Some(record) = &best.evidence {
                    self.expires_at_ms = self.expires_at_ms.min(record.body.expires_at_ms);
                }

                if self.best.as_ref().is_none_or(|old| {
                    match (
                        best.evidence.as_ref().and_then(|r| r.body.speed.as_ref()),
                        old.evidence.as_ref().and_then(|r| r.body.speed.as_ref()),
                    ) {
                        (Some(a), Some(b))
                            if matches!(
                                self.controls.profile.as_ref().unwrap().ranking,
                                Ranking::PreferredSpeed
                            ) =>
                        {
                            a.faster_than(b)
                                || (!b.faster_than(a)
                                    && better(best.score, &best.model, old.score, &old.model))
                        }
                        _ => better(best.score, &best.model, old.score, &old.model),
                    }
                }) {
                    self.best = Some(best);
                }
            }
        }
    }
}
fn better(cost: MoneyAu, model: &str, old_cost: MoneyAu, old_model: &str) -> bool {
    (cost, model) < (old_cost, old_model)
}
fn allowed(models: &[String], model: &str) -> bool {
    models.is_empty() || models.binary_search_by(|v| v.as_str().cmp(model)).is_ok()
}
fn status(s: &Session, status: &str, exhausted: bool, selection: Option<Value>) -> Value {
    serde_json::json!({"schema_version":1,"lane":"proxy","kind":"profile_resolution",
        "resolution_id":s.id,"revision":s.revision,"status":status,"continuation":null,
        "endpoint":s.endpoint,"rail":s.controls.rail,"profile_hash":s.controls.profile.as_ref().and_then(|p| p.digest().ok()),
        "registry_release":s.controls.registry_release,"snapshot":s.snapshot,"query_key":s.query_key,
        "request_content_digest":s.request_content_digest,"model_allowlist_digest":s.model_allowlist_digest,
        "started_at_ms":s.started_at_ms,"retention_expires_at_ms":s.expires_at_ms,
        "scope_exhausted":exhausted,"considered_candidates":s.considered,"scanned_candidates":s.scanned,
        "index_reads":s.index_reads,"exclusions":s.exclusions,"unresolved_candidates":s.unknown,
        "ranking_basis":if matches!(s.controls.profile.as_ref().unwrap().ranking, Ranking::PreferredSpeed) {"locally_tokenized_generation_rate"} else if s.retail_ranking.is_some() {"maximum_retail_micro"} else {"maximum_wholesale_au"},"ranking_claim":if status == "selected" {if matches!(s.controls.profile.as_ref().unwrap().ranking, Ranking::PreferredSpeed) {"highest_observed_comparable_rate"} else {"lowest_maximum_among_validated_candidates"}} else if status == "retained_compatible" {"continuity_retention"} else {"none"},
        "retail_pricing":if s.retail_ranking.is_some() {"applied_projection"} else {"not_applied"},"retail_ranking":s.retail_ranking,"authorizes_execution":false,"selection":selection,
        "pending_reason":if status == "pending" { s.pending_reason } else { None },"retry_after_ms":if status == "pending" {500} else {0}})
}

async fn advance(
    state: &SharedState,
    runtime: &Arc<Runtime>,
    session: &mut Session,
) -> Result<Value, ApiError> {
    let now = super::super::now_millis_u64();
    if now < session.started_at_ms || now >= session.expires_at_ms {
        return Err(unavailable());
    }
    session.pending_reason = None;
    if !session.previous_checked {
        if matches!(
            session.controls.profile.as_ref().unwrap().continuity,
            Continuity::RetainCompatible
        ) {
            if let Some(model) = session.previous.clone() {
                let id = proxy_request::Selector::parse(&model)
                    .map_err(|_| invalid())?
                    .ok_or_else(invalid)?
                    .id();
                let catalog = session.catalog.as_ref().ok_or_else(unavailable)?.clone();
                let time = session.started_at_ms;
                let permit = CPU.try_acquire().map_err(|_| busy())?;
                let published = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    catalog.proxy_offer(&id, time)
                })
                .await
                .map_err(|_| unavailable())?
                .map_err(|_| unavailable())?;
                if let Some(published) = published {
                    match check(state, runtime, session, published).await {
                        Checked::Ready(best) => {
                            if let Some(record) = &best.declaration {
                                session.expires_at_ms =
                                    session.expires_at_ms.min(record.expires_at_ms);
                            }
                            if let Some(record) = &best.evidence {
                                session.expires_at_ms =
                                    session.expires_at_ms.min(record.body.expires_at_ms);
                            }

                            session.best = Some(best);
                            let selected = finish(state, session).await?;
                            session.complete = true;
                            session.catalog = None;
                            retain_winner(session, &selected);
                            return Ok(status(
                                session,
                                "retained_compatible",
                                false,
                                Some(selected),
                            ));
                        }
                        Checked::Pending(reason) => return Ok(pending(session, reason)),
                        Checked::Excluded(_, true) => {
                            session.exclude("previous_model_unresolved", true)
                        }
                        Checked::Excluded(_, false) => (),
                    }
                    session.waiting = None;
                }
            }
        }
        session.previous_checked = true;
    }
    if session.page.is_empty() && !session.traversal_done {
        // At most one administrative page plus one bounded index page per step.
        // Empty continued membership pages are progress, never exhaustion.
        if let Some(t) = &mut session.taxonomy {
            if t.current.is_none() && t.scopes.is_empty() && !t.exhausted {
                let control = state.proxy_control().ok_or_else(unavailable)?;
                let page = control
                    .registry()
                    .ok_or_else(unavailable)?
                    .taxonomy_members(&t.pin, &t.reference, t.cursor.as_deref(), 16)
                    .await
                    .map_err(|_| unavailable())?;
                if t.category.as_ref().is_some_and(|c| c != &page.category) {
                    return Err(unavailable());
                }
                t.category = Some(page.category);
                t.cursor = page.next_cursor;
                t.exhausted = page.exhausted;
                t.scopes = page.scopes.into();
            }
            if t.current.is_none() {
                t.current = t.scopes.pop_front();
                t.query = None;
                session.cursor = None;
            }
            if t.current.is_none() {
                session.traversal_done = t.exhausted;
                if !session.traversal_done {
                    return Ok(pending(session, "scope_remaining"));
                }
            }
        }
        if !session.traversal_done {
            let catalog = session.catalog.as_ref().ok_or_else(unavailable)?.clone();
            let policy = session.controls.profile.clone().ok_or_else(invalid)?;
            let rail = session.controls.rail;
            let cursor = session.cursor.clone();
            let scope = session.taxonomy.as_ref().and_then(|t| t.current.clone());
            let at = session.started_at_ms;
            let limit = runtime.resolver.limits.candidates_per_step;
            let permit = CPU.try_acquire().map_err(|_| busy())?;
            let page = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                catalog.proxy_candidates_in_scope(
                    &policy,
                    rail,
                    scope.as_ref(),
                    cursor.as_deref(),
                    limit,
                    at,
                )
            })
            .await
            .map_err(|_| unavailable())?
            .map_err(|_| unavailable())?;
            let old_query = session
                .taxonomy
                .as_ref()
                .and_then(|t| t.query.as_ref())
                .or_else(|| {
                    if session.taxonomy.is_none() {
                        session.query_key.as_ref()
                    } else {
                        None
                    }
                });
            if page.snapshot != session.snapshot || old_query.is_some_and(|q| q != &page.query_key)
            {
                return Err(unavailable());
            }
            session.scanned += page.scanned_candidates as u64;
            session.index_reads += page.index_reads as u64;
            session.page = page.entries.into();
            session.cursor = page.next_cursor;
            if let Some(t) = &mut session.taxonomy {
                t.query = Some(page.query_key);
                if page.exhausted {
                    t.current = None;
                }
                session.traversal_done = page.exhausted && t.scopes.is_empty() && t.exhausted;
            } else {
                session.query_key = Some(page.query_key);
                session.traversal_done = page.exhausted;
            }
        }
    }
    while let Some(published) = session.page.front().cloned() {
        match check(state, runtime, session, published).await {
            Checked::Pending(reason) => return Ok(pending(session, reason)),
            checked => {
                session.consider(checked);
                session.page.pop_front();
                session.waiting = None;
            }
        }
    }
    if !session.traversal_done {
        return Ok(pending(session, "scope_remaining"));
    }
    if session.unknown > 0 || session.best.is_none() {
        session.complete = true;
        session.catalog = None;
        session.waiting = None;
        return Ok(status(
            session,
            if session.unknown > 0 {
                "incomplete"
            } else {
                "no_match"
            },
            true,
            None,
        ));
    }
    let best_model = session.best.as_ref().unwrap().model.clone();
    if let Some(outcome) = observe_market(state, runtime, session, &best_model) {
        match outcome {
            Checked::Pending(reason) => return Ok(pending(session, reason)),
            Checked::Excluded(reason, _) => {
                session.exclude(reason, true);
                session.complete = true;
                session.catalog = None;
                session.waiting = None;
                return Ok(status(session, "incomplete", true, None));
            }
            Checked::Ready(_) => unreachable!(),
        }
    }
    let best_request = session.best.as_ref().unwrap().request.clone();
    let current = proxy_request::resolve_estimate(
        state.proxy_control().cloned().ok_or_else(unavailable)?,
        best_request,
    )
    .await
    .map_err(|_| unavailable())?;
    if current.availability.status != Eligibility::Available {
        if observation_pending(current.availability.status) {
            return Ok(pending(session, "presence_pending"));
        }
        return Err(unavailable());
    }
    let selected = finish(state, session).await?;
    session.complete = true;
    session.catalog = None;
    retain_winner(session, &selected);
    Ok(status(session, "selected", true, Some(selected)))
}
fn pending(session: &mut Session, reason: &'static str) -> Value {
    session.pending_reason = Some(reason);
    status(
        session,
        "pending",
        session.traversal_done && session.page.is_empty(),
        None,
    )
}
fn retain_winner(session: &mut Session, selected: &Value) {
    let expires = selected["estimate"]["expires_at_ms"].as_u64().unwrap_or(0);
    session.lease_until = Some(
        Instant::now()
            + Duration::from_millis(expires.saturating_sub(super::super::now_millis_u64())),
    );
}
fn observation_pending(status: Eligibility) -> bool {
    matches!(
        status,
        Eligibility::HeartbeatMissing
            | Eligibility::Checking
            | Eligibility::StaleEvidence
            | Eligibility::ThroughputUnverified
    )
}
fn observe_market(
    state: &SharedState,
    runtime: &Arc<Runtime>,
    session: &mut Session,
    model: &str,
) -> Option<Checked> {
    if session.waiting.as_ref().is_none_or(|w| w.model != model) {
        session.waiting = Some(Waiting {
            model: model.into(),
            started: Instant::now(),
            lease: None,
        });
    }
    let waiting = session.waiting.as_mut().unwrap();
    if waiting.started.elapsed()
        >= Duration::from_millis(runtime.resolver.limits.observation_timeout_ms)
    {
        return Some(Checked::Excluded("observation_deadline", true));
    }
    if waiting.lease.is_none() {
        let Some(control) = state.proxy_control() else {
            return Some(Checked::Excluded("catalog_unavailable", true));
        };
        let market = match proxy_request::Selector::parse(model) {
            Ok(Some(s)) => s.market().clone(),
            _ => return Some(Checked::Excluded("request_invalid", false)),
        };
        match control.presence().observe_market(market) {
            Ok(lease) => waiting.lease = Some(lease),
            Err(_) => return Some(Checked::Pending("observation_budget")),
        }
    }
    None
}

async fn check(
    state: &SharedState,
    runtime: &Arc<Runtime>,
    session: &mut Session,
    published: PublishedOffer,
) -> Checked {
    let model = format!("proxy/offer/{}", published.id);
    if !allowed(&session.models, &model)
        || session
            .model_allowlist
            .as_ref()
            .is_some_and(|v| v.binary_search(&model).is_err())
    {
        return Checked::Excluded("key_model_excluded", false);
    }
    let profile = session.controls.profile.clone().unwrap();
    if !matches!(&profile.target, Target::TaxonomyCategory { .. })
        && profile
            .check_offer(&published, session.endpoint, session.controls.rail)
            .is_err()
    {
        return Checked::Excluded("profile_constraints", false);
    }
    let mut body = session.body.clone();
    body["model"] = serde_json::json!(model);
    body["proxy"] = match serde_json::to_value(&session.controls) {
        Ok(v) => v,
        Err(_) => return Checked::Excluded("request_invalid", false),
    };
    let request = match proxy_request::Request::parse(session.endpoint, body, &runtime.policy) {
        Ok(Some(r)) => Arc::new(r),
        _ => return Checked::Excluded("request_invalid", false),
    };
    let Some(control) = state.proxy_control().cloned() else {
        return Checked::Excluded("catalog_unavailable", true);
    };
    if let Some(pending) = observe_market(state, runtime, session, &model) {
        return pending;
    }
    let selected = match proxy_request::resolve_estimate(control.clone(), request.clone()).await {
        Ok(value) => value,
        Err(proxy_request::Error::Constraints | proxy_request::Error::Price) => {
            return Checked::Excluded("request_constraints", false)
        }
        Err(proxy_request::Error::Verification) => {
            return Checked::Excluded("operator_not_verified", false)
        }
        Err(proxy_request::Error::ProfileEvidence) => {
            return Checked::Excluded("profile_evidence_unavailable", true)
        }
        Err(_) => return Checked::Excluded("catalog_unavailable", true),
    };
    if selected.published.digest != published.digest
        || selected.published.membership != published.membership
        || selected.published.market != published.market
    {
        return Checked::Excluded("publication_changed", true);
    }
    if selected.availability.status != Eligibility::Available {
        if observation_pending(selected.availability.status) {
            return Checked::Pending("presence_pending");
        }
        return Checked::Excluded(
            match selected.availability.status {
                Eligibility::Busy => "provider_busy",
                Eligibility::Draining => "provider_draining",
                Eligibility::ThroughputFloor => "throughput_floor",
                Eligibility::ThroughputUnverified => "throughput_unverified",
                _ => "availability_unavailable",
            },
            !matches!(
                selected.availability.status,
                Eligibility::Busy | Eligibility::Draining | Eligibility::ThroughputFloor
            ),
        );
    }
    let adapter = match runtime
        .controller
        .describe(
            selected.candidate.offer.clone(),
            session.controls.rail,
            session.controls.settlement_policy_hash.clone(),
            &selected.candidate.endpoint_contract,
            &selected.candidate.recipe_hash,
        )
        .await
    {
        Ok(value) => value,
        Err(_) => return Checked::Excluded("descriptor_unavailable", true),
    };
    let body = if profile.constraints.request_controls.is_empty() {
        request.provider_value().clone()
    } else {
        match super::profile::materialize(
            control.clone(),
            request.clone(),
            adapter.clone(),
            selected.candidate.offer.clone(),
            false,
        )
        .await
        {
            Ok((body, _)) => body,
            Err(e) if e.public_code == "proxy_profile_controls_invalid" => {
                return Checked::Excluded("request_controls_incompatible", false)
            }
            Err(_) => return Checked::Excluded("control_preparation_unavailable", true),
        }
    };
    let bytes = match serde_json::to_vec(&body) {
        Ok(b) => b,
        Err(_) => return Checked::Excluded("request_invalid", false),
    };
    let prices = session.controls.prices.clone();
    let output = session.controls.output_units;
    let lifetimes = runtime.policy.lifetimes();
    let offer = published.offer.clone();
    let permit = match CPU.try_acquire() {
        Ok(p) => p,
        Err(_) => return Checked::Excluded("metering_busy", true),
    };
    let maximum = match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        PurchaseRequest::new(adapter, bytes, prices, output, lifetimes)?.maximum(&offer)
    })
    .await
    {
        Ok(Ok(m)) => m,
        Ok(Err(_)) => return Checked::Excluded("request_or_maximum_limit", false),
        Err(_) => return Checked::Excluded("metering_unavailable", true),
    };
    let mut prepared = body.clone();
    prepared["proxy"] = serde_json::to_value(&session.controls).unwrap();
    let request = match proxy_request::Request::parse(session.endpoint, prepared, &runtime.policy) {
        Ok(Some(r)) => Arc::new(r),
        _ => return Checked::Excluded("request_invalid", false),
    };
    let maximum_retail_cost_micro = match &session.retail_ranking {
        Some(policy) => match policy.maximum(&published.offer, &maximum.max_usage) {
            Some(value) => Some(value),
            None => return Checked::Excluded("retail_maximum_limit", false),
        },
        None => None,
    };
    let evidence = match super::evidence::check(control.clone(), request.clone(), &published).await
    {
        Ok(value) => value,
        Err(_) => return Checked::Excluded("conformance_evidence_unavailable", true),
    };
    let declaration = match super::data_handling::check(
        control,
        &runtime.controller,
        request.clone(),
        &published,
    )
    .await
    {
        Ok(value) => value,
        Err(super::data_handling::Failure::Unsatisfied) => {
            return Checked::Excluded("data_handling_constraints", false)
        }
        Err(_) => return Checked::Excluded("data_handling_unavailable", true),
    };
    Checked::Ready(Best {
        declaration,
        evidence,
        score: maximum_retail_cost_micro.unwrap_or(maximum.max_spend_au),
        maximum_retail_cost_micro,
        model,
        body,
        request,
        maximum,
        published,
    })
}

async fn finish(state: &SharedState, session: &Session) -> Result<Value, ApiError> {
    let best = session.best.as_ref().ok_or_else(unavailable)?;
    let control = state.proxy_control().cloned().ok_or_else(unavailable)?;
    let latest = proxy_request::resolve_estimate(control.clone(), best.request.clone())
        .await
        .map_err(|_| unavailable())?;
    let current = control
        .catalog()
        .read()
        .map_err(|_| unavailable())?
        .status();
    let now = super::super::now_millis_u64();
    if current.content_snapshot != session.snapshot
        || latest.published.digest != best.published.digest
        || latest.published.membership != best.published.membership
        || latest.published.market != best.published.market
        || latest.availability.status != Eligibility::Available
        || now < session.started_at_ms
        || now >= session.expires_at_ms
    {
        return Err(unavailable());
    }
    let evidence =
        super::evidence::check(control.clone(), best.request.clone(), &latest.published).await?;
    if best.evidence.as_ref().map(|r| r.digest().ok()) != evidence.as_ref().map(|r| r.digest().ok())
    {
        return Err(unavailable());
    }
    let runtime = state.proxy_buyer.as_ref().ok_or_else(unavailable)?;
    let declaration = super::data_handling::check(
        control,
        &runtime.controller,
        best.request.clone(),
        &latest.published,
    )
    .await
    .map_err(super::data_handling::Failure::api)?;
    if best.declaration.as_ref().map(|r| r.record.digest().ok())
        != declaration.as_ref().map(|r| r.record.digest().ok())
    {
        return Err(unavailable());
    }
    let expires = latest
        .expires_at_ms
        .min(session.expires_at_ms)
        .min(declaration.as_ref().map_or(u64::MAX, |d| d.expires_at_ms))
        .min(latest.availability.expires_at_ms.ok_or_else(unavailable)?);
    if expires <= now {
        return Err(unavailable());
    }
    let content = retail::request_content_digest(&best.body).map_err(|_| invalid())?;
    let mut estimate = serde_json::json!({"schema_version":1,"lane":"proxy","kind":"maximum_estimate",
        "network":latest.network,"model":best.model,"endpoint":session.endpoint,"rail":session.controls.rail,
        "request_hash":best.maximum.request_hash,"request_content_digest":content,"controls":session.controls,
        "offer":latest.published.offer,"offer_digest":latest.published.digest,
        "membership_digest":latest.published.membership.digest().map_err(|_| unavailable())?,
        "endpoint_contract":latest.candidate.endpoint_contract,"recipe_hash":latest.candidate.recipe_hash,
        "settlement_policy_hash":session.controls.settlement_policy_hash,"metering_policy_hash":best.maximum.metering_policy_hash,
        "max_usage":best.maximum.max_usage,"max_spend_au":best.maximum.max_spend_au.to_string(),
        "observed_at_ms":now,"expires_at_ms":expires,"availability":latest.availability});
    let bytes = mayhem_proto::stable_json_bytes(&estimate).map_err(|_| invalid())?;
    estimate["estimate_hash"] =
        serde_json::json!(digest("mayhem/proxy/maximum-estimate/v1", &[&bytes]));
    let mut body = best.body.clone();
    body["proxy"] = serde_json::to_value(&session.controls).map_err(|_| invalid())?;
    Ok(
        serde_json::json!({"model":best.model,"request":body,"estimate":estimate,
        "maximum_retail_cost_micro":best.maximum_retail_cost_micro.map(|v| v.to_string())}),
    )
}
