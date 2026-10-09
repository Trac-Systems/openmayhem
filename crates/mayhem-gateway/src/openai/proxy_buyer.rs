//! Explicit paid proxy HTTP ownership. Discovery alone never enables this lane.
//! Owned tasks outlive a disconnected HTTP caller; recovery observes the original
//! purchase and never dispatches Execute again.
use super::{
    attach_gateway_job_headers, gateway_existing_job_response, gateway_job_id,
    gateway_job_pending_response, gateway_prefers_async_response, now_secs,
    proxy_owner::{Binding, Owner},
    proxy_request, ApiError, GatewayRequestCancellation, GatewayState, GatewayTokenAttribution,
    SharedState,
};
use crate::job_store::{BeginGatewayJob, GatewayJobStatus, StoredGatewayJob};
use axum::{
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use mayhem_proto::proxy::{finance::ProxySettlementPolicy, ProxyEndpoint};
use mayhem_proxy::{
    attempts::Digest,
    buyer_controller::{self, Controller, RequestIdentity},
    negotiation,
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    sync::{oneshot, watch, OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
};

mod contract;
mod data_handling;
mod estimation;
pub(super) mod evidence;
mod profile;
mod resolver;
mod retail;
mod streaming;
mod taxonomy_filters;
pub(super) use contract::handle as contract;
pub(super) use estimation::handle as estimate;
pub(super) use evidence::policy as conformance_policy;
pub(super) use profile::handle as prepare_profile;
pub(super) use resolver::handle as resolve_profile;
pub use resolver::Limits as ProfileResolutionLimits;
pub use retail::{
    request_content_digest as retail_request_content_digest, Config as RetailAuthorizationConfig,
};

static STORAGE: Semaphore = Semaphore::const_new(8);

pub struct Runtime {
    controller: Arc<Controller>,
    policy: proxy_request::Policy,
    settlement_policy: ProxySettlementPolicy,
    slots: Arc<Semaphore>,
    streams: Arc<Semaphore>,
    retail: Option<Arc<retail::Authority>>,
    active: Arc<Mutex<BTreeSet<String>>>,
    tasks: Mutex<JoinSet<()>>,
    halt: watch::Sender<bool>,
    running: AtomicBool,
    resolver: resolver::Store,
}
impl fmt::Debug for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyBuyerRuntime")
            .field("running", &self.running.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}
struct Claim {
    id: String,
    active: Arc<Mutex<BTreeSet<String>>>,
    _permit: OwnedSemaphorePermit,
}
struct Running(Arc<Runtime>);
impl Drop for Running {
    fn drop(&mut self) {
        self.0.resolver.clear();
        self.0.running.store(false, Ordering::Release);
        self.0.halt.send_replace(true);
    }
}
impl Drop for Claim {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(&self.id);
        }
    }
}
impl Runtime {
    pub fn new(
        controller: Arc<Controller>,
        policy: proxy_request::Policy,
        settlement_policy: ProxySettlementPolicy,
        sessions: usize,
    ) -> Result<Self, String> {
        settlement_policy.validate()?;
        if !(1..=64).contains(&sessions)
            || settlement_policy.digest()? != policy.settlement_policy_hash().as_str()
        {
            return Err("invalid proxy buyer policy or session bound".into());
        }
        let (halt, _) = watch::channel(false);
        Ok(Self {
            controller,
            policy,
            settlement_policy,
            slots: Arc::new(Semaphore::new(sessions)),
            streams: Arc::new(Semaphore::new(sessions)),
            retail: None,
            active: Arc::new(Mutex::new(BTreeSet::new())),
            tasks: Mutex::new(JoinSet::new()),
            halt,
            running: AtomicBool::new(false),
            resolver: resolver::Store::new(ProfileResolutionLimits::default())?,
        })
    }

    /// Trusted operator resource budgets for read-only profile resolution.
    pub fn with_profile_resolution_limits(
        mut self,
        limits: ProfileResolutionLimits,
    ) -> Result<Self, String> {
        self.resolver = resolver::Store::new(limits)?;
        Ok(self)
    }

    /// Trusted operator configuration only. Matching keys cannot opt out through
    /// request fields/headers. Other buyers keep their ordinary Core owner gate.
    pub fn with_retail_authorization(
        mut self,
        config: RetailAuthorizationConfig,
    ) -> Result<Self, String> {
        self.retail = Some(Arc::new(retail::Authority::new(config)?));
        Ok(self)
    }

    pub(crate) fn validate_owner(&self, state: &GatewayState) -> Result<(), String> {
        let buyer = hex::encode(
            ed25519_dalek::SigningKey::from_bytes(&state.receipt_config.user_seed)
                .verifying_key()
                .to_bytes(),
        );
        if self.controller.identity().controller_pubkey.as_str() != buyer
            || !state.access_control.has_durable_key_budget()
            || !state
                .jobs
                .lock()
                .map_err(|_| "gateway job vault unavailable")?
                .proxy_enabled()
            || state.proxy_control.is_none()
        {
            return Err(
                "paid proxy requires the same wallet, durable jobs/budgets and discovery".into(),
            );
        }
        if let Some(store) = state.proxy_control.as_ref().and_then(|c| c.conformance()) {
            self.controller
                .enable_conformance(store.clone())
                .map_err(|_| "proxy conformance owner differs")?;
        }
        Ok(())
    }

    fn claim(&self, id: &str) -> Result<Claim, ApiError> {
        if !self.running.load(Ordering::Acquire) || *self.halt.borrow() {
            return Err(unavailable());
        }
        let permit = self.slots.clone().try_acquire_owned().map_err(|_| busy())?;
        if !self
            .active
            .lock()
            .map_err(|_| unavailable())?
            .insert(id.to_owned())
        {
            return Err(busy());
        }
        Ok(Claim {
            id: id.into(),
            active: self.active.clone(),
            _permit: permit,
        })
    }

    fn launch(
        self: &Arc<Self>,
        state: GatewayState,
        binding: Binding,
        request: Option<buyer_controller::Request>,
        stream: Option<buyer_controller::StreamSender>,
        claim: Claim,
    ) -> Result<oneshot::Receiver<Result<StoredGatewayJob, ApiError>>, ApiError> {
        let mut tasks = self.tasks.lock().map_err(|_| unavailable())?;
        while tasks.try_join_next().is_some() {}
        if *self.halt.borrow() {
            return Err(unavailable());
        }
        let cancellation = GatewayRequestCancellation::new();
        state
            .active_job_cancellations
            .lock()
            .map_err(|_| unavailable())?
            .insert(binding.job_id.clone(), cancellation.clone());
        let runtime = self.clone();
        let mut halted = self.halt.subscribe();
        let (reply, receive) = oneshot::channel();
        tasks.spawn(async move {
            let _claim = claim;
            let (stop, stopped) = watch::channel(false);
            let owner = Arc::new(Owner::new(
                state.jobs.clone(),
                state.access_control.clone(),
                binding.clone(),
            ));
            let operation = async {
                match request {
                    Some(request) => match stream {
                        Some(stream) => {
                            runtime
                                .controller
                                .execute_stream(request, stream, stopped)
                                .await
                        }
                        None => runtime.controller.execute(request, stopped).await,
                    },
                    None => {
                        runtime
                            .controller
                            .recover(binding.identity.clone(), owner.clone(), stopped)
                            .await
                    }
                }
            };
            tokio::pin!(operation);
            let outcome = tokio::select! {
                value = &mut operation => value,
                _ = cancellation.cancelled() => { stop.send_replace(true); operation.await },
                _ = stop_signal(&mut halted) => { stop.send_replace(true); operation.await },
            };
            // No result/error string from an upstream may release money. Exact
            // canonical closure and durable output are required independently.
            let mut result = runtime.refresh_owner(&state, &binding, &owner).await;
            if outcome
                .as_ref()
                .is_err_and(|error| !error.recovery_required)
            {
                if result.as_ref().is_ok_and(|job| {
                    job.proxy
                        .as_ref()
                        .is_some_and(|p| p.terms().is_none() && p.non_admission().is_none())
                }) {
                    result = fail_unsigned(&state, &binding.job_id).await;
                }
            }
            if let Ok(mut active) = state.active_job_cancellations.lock() {
                if active.get(&binding.job_id) == Some(&cancellation) {
                    active.remove(&binding.job_id);
                }
            }
            let result = match (result, outcome) {
                (Ok(job), _) => Ok(job),
                (Err(_), Err(_)) => Err(unavailable()),
                (Err(error), Ok(_)) => Err(error),
            };
            let _ = reply.send(result);
        });
        Ok(receive)
    }

    async fn refresh_owner(
        &self,
        state: &GatewayState,
        binding: &Binding,
        owner: &Owner,
    ) -> Result<StoredGatewayJob, ApiError> {
        if let Some(saved) = self
            .controller
            .retained_purchase(&binding.identity)
            .await
            .map_err(|_| unavailable())?
        {
            let observation = if saved.authorization().is_some() {
                Some(
                    self.controller
                        .observe_purchase(&saved)
                        .await
                        .map_err(|_| unavailable())?,
                )
            } else {
                None
            };
            owner
                .note_purchase(saved)
                .await
                .map_err(|_| unavailable())?;
            if let Some(observation) = observation {
                if observation
                    .financial_outcome()
                    .map_err(|_| unavailable())?
                    .is_some()
                {
                    return owner.close(observation).await.map_err(|_| unavailable());
                }
            }
        }
        load(state, &binding.job_id).await?.ok_or_else(unavailable)
    }

    /// Caller owns this lifecycle alongside HTTP serving. Each tick reads only a
    /// bounded page of pending job IDs. No ledger/history scan or per-token work.
    pub async fn run(
        self: Arc<Self>,
        state: GatewayState,
        mut stop: watch::Receiver<bool>,
    ) -> Result<(), String> {
        self.validate_owner(&state)?;
        if *self.halt.borrow() || self.running.swap(true, Ordering::AcqRel) {
            return Err("proxy buyer already running or stopped".into());
        }
        let _running = Running(self.clone());
        let mut cursor: Option<String> = None;
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = stop_signal(&mut stop) => break,
                _ = interval.tick() => {
                    self.resolver.expire();
                    let jobs = state.jobs.clone();
                    let after = cursor.clone();
                    let page = storage(move || jobs.lock().map_err(|_| unavailable())?
                        .pending_proxy(after.as_deref(), 64).map_err(|_| unavailable())).await;
                    let Ok(page) = page else { continue };
                    cursor = if page.len() < 64 { None } else { page.last().cloned() };
                    for id in page {
                        let Ok(claim) = self.claim(&id) else { continue };
                        let Ok(Some(job)) = load(&state, &id).await else { continue };
                        let Some(proxy) = &job.proxy else { continue };
                        let Some(owner_id) = &job.owner_token_id else { continue };
                        if proxy.terms().is_none() && proxy.non_admission().is_none() {
                            // With no active owner and no durable authorization,
                            // our mandatory owner gate never permitted signing.
                            // Do not infer this from a missing buyer journal.
                            let _ = fail_unsigned(&state, &id).await;
                            continue;
                        }
                        // Recovery cannot grant new authorization: this name is
                        // only attribution for an already retained obligation.
                        let binding = Binding { job_id: id, model: job.model.clone(),
                            fingerprint: job.request_fingerprint.clone(), identity: proxy.identity().clone(),
                            token: GatewayTokenAttribution { name: String::new(), token_id: owner_id.clone() } };
                        let _ = self.launch(state.clone(), binding, None, None, claim);
                    }
                }
            }
        }
        self.running.store(false, Ordering::Release);
        self.halt.send_replace(true);
        let mut failed = self.controller.shutdown().await.is_err();
        loop {
            let joined = std::future::poll_fn(|cx| match self.tasks.lock() {
                Ok(mut tasks) => tasks.poll_join_next(cx).map(Ok),
                Err(_) => std::task::Poll::Ready(Err("proxy owner task set unavailable")),
            })
            .await?;
            match joined {
                None => break,
                Some(Ok(())) => (),
                Some(Err(_)) => failed = true,
            }
        }
        if failed {
            Err("proxy buyer shutdown or owner task failed".into())
        } else {
            Ok(())
        }
    }
}

async fn stop_signal(stop: &mut watch::Receiver<bool>) {
    while !*stop.borrow_and_update() {
        if stop.changed().await.is_err() {
            break;
        }
    }
}
async fn storage<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let permit = STORAGE.try_acquire().map_err(|_| busy())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        f()
    })
    .await
    .map_err(|_| unavailable())?
}
async fn load(state: &GatewayState, id: &str) -> Result<Option<StoredGatewayJob>, ApiError> {
    let jobs = state.jobs.clone();
    let id = id.to_owned();
    storage(move || {
        jobs.lock()
            .map_err(|_| unavailable())?
            .get(&id, now_secs())
            .map_err(|_| unavailable())
    })
    .await
}
async fn fail_unsigned(state: &GatewayState, id: &str) -> Result<StoredGatewayJob, ApiError> {
    let jobs = state.jobs.clone();
    let id = id.to_owned();
    storage(move || {
        jobs.lock()
            .map_err(|_| unavailable())?
            .fail_proxy_before_authorization(&id, now_secs())
            .map_err(|_| unavailable())
    })
    .await
}
fn digest(domain: &str, parts: &[&[u8]]) -> Digest {
    let mut hash = blake3::Hasher::new_derive_key(domain);
    for part in parts {
        hash.update(&(part.len() as u64).to_le_bytes());
        hash.update(part);
    }
    Digest::new(hash.finalize().to_hex().to_string()).expect("digest is hexadecimal")
}
fn unavailable() -> ApiError {
    ApiError::service_unavailable(
        "proxy purchase is unavailable or awaiting recovery",
        Some("proxy"),
    )
    .with_public_error("proxy_recovery_required", "proxy", true)
}
fn busy() -> ApiError {
    ApiError::service_unavailable("proxy buyer capacity is busy", Some("proxy")).with_public_error(
        "capacity_unavailable",
        "proxy",
        true,
    )
}
fn invalid() -> ApiError {
    ApiError::bad_request("invalid proxy request or policy", Some("proxy"))
}

/// Current operator-approved settlement facts, not a quote or spending grant.
/// Callers pin the hash in their explicit request; admission still checks the
/// runtime policy and the provider/canonical terms independently.
pub(super) async fn buyer_policy(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    let result = (|| -> Result<Response, ApiError> {
        state
            .authorize_existing_gateway_request(&headers, None)?
            .ok_or_else(|| {
                ApiError::unauthorized(
                    "proxy buyer policy requires an authenticated key",
                    Some("Authorization"),
                )
            })?;
        let runtime = state.proxy_buyer.as_ref().ok_or_else(|| {
            ApiError::service_unavailable("proxy buyer is not configured", Some("proxy"))
                .with_public_error("proxy_buyer_disabled", "proxy_discovery", true)
        })?;
        Ok(Json(serde_json::json!({
            "schema_version": 1,
            "settlement_policy_hash": runtime.policy.settlement_policy_hash(),
            "settlement_policy": runtime.settlement_policy,
        }))
        .into_response())
    })();
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    response
        .headers_mut()
        .insert("cache-control", "private, no-store".parse().unwrap());
    response
}

pub(crate) fn selected(raw: &Value) -> bool {
    raw.get("model")
        .and_then(Value::as_str)
        .is_some_and(|m| m.starts_with("proxy/"))
        || raw.get("proxy").is_some()
}

fn response(job: StoredGatewayJob) -> Response {
    if job.status == GatewayJobStatus::Completed {
        if let Some(result) = job.result {
            let mut response = Json(result).into_response();
            attach_gateway_job_headers(&mut response, &job.id);
            return response;
        }
        return unavailable().into_response();
    }
    gateway_existing_job_response(job)
}

pub(crate) async fn handle(
    state: SharedState,
    headers: HeaderMap,
    raw: Value,
    endpoint: ProxyEndpoint,
) -> Response {
    match submit(state, headers, raw, endpoint).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}
async fn submit(
    state: SharedState,
    headers: HeaderMap,
    raw: Value,
    endpoint: ProxyEndpoint,
) -> Result<Response, ApiError> {
    let runtime = state.proxy_buyer.as_ref().ok_or_else(unavailable)?.clone();
    let mut required_headers = headers
        .get_all("x-mayhem-require-retail-authorization")
        .iter();
    let require_retail = match required_headers.next() {
        None => false,
        Some(value) if value.as_bytes() == b"1" && required_headers.next().is_none() => true,
        _ => {
            return Err(ApiError::bad_request(
                "invalid required retail authorization header",
                Some("X-Mayhem-Require-Retail-Authorization"),
            ))
        }
    };
    let request = Arc::new(
        proxy_request::Request::parse(endpoint, raw, &runtime.policy)
            .map_err(|_| invalid())?
            .ok_or_else(invalid)?,
    );
    let provider_body: Value =
        serde_json::from_slice(request.provider_request()).map_err(|_| invalid())?;
    let streaming = match provider_body.get("stream") {
        None | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) if endpoint != ProxyEndpoint::Decisions => true,
        _ => {
            return Err(ApiError::bad_request(
                "unsupported proxy stream mode",
                Some("stream"),
            ))
        }
    };
    if streaming && gateway_prefers_async_response(&headers) {
        return Err(ApiError::bad_request(
            "proxy streaming requires an attached observer; omit Prefer: respond-async",
            Some("Prefer"),
        ));
    }
    let model = request.selector().model();
    // Read/replay authorization still enforces revocation, expiry, model scope
    // and rate limits. New expenditure is checked atomically by the owner gate.
    let token = state
        .authorize_existing_gateway_request(&headers, Some(&model))?
        .ok_or_else(|| {
            ApiError::unauthorized(
                "proxy requests require an authenticated key",
                Some("Authorization"),
            )
        })?;
    let endpoint_name = serde_json::to_value(endpoint)
        .map_err(|_| invalid())?
        .as_str()
        .ok_or_else(invalid)?
        .to_owned();
    let key = headers
        .get("idempotency-key")
        .map(|v| v.to_str().map_err(|_| invalid()))
        .transpose()?;
    let id = gateway_job_id(
        state.receipt_config.user_seed,
        Some(&token.token_id),
        &endpoint_name,
        key,
    )
    .map_err(|e| ApiError::bad_request(e, Some("Idempotency-Key")))?;
    let owner_digest = digest(
        "mayhem/proxy/gateway-owner/v1",
        &[
            runtime
                .controller
                .identity()
                .controller_pubkey
                .as_str()
                .as_bytes(),
            token.token_id.as_bytes(),
        ],
    );
    let body: Value = serde_json::from_slice(request.provider_request()).map_err(|_| invalid())?;
    let retail = runtime
        .retail
        .as_ref()
        .map(|authority| authority.correlation(&token.token_id, key, &body))
        .transpose()?
        .flatten();
    if require_retail && retail.is_none() {
        return Err(ApiError::service_unavailable(
            "required retail authorization is unavailable",
            Some("proxy"),
        )
        .with_public_error(
            "proxy_retail_authorization_unavailable",
            "proxy_admission",
            true,
        ));
    }
    let base_fingerprint = request.fingerprint(&owner_digest).map_err(|_| invalid())?;
    let fingerprint = match &retail {
        Some(correlation) => runtime
            .retail
            .as_ref()
            .unwrap()
            .fingerprint(&base_fingerprint, correlation),
        None => base_fingerprint,
    }
    .as_str()
    .to_owned();
    let identity = RequestIdentity {
        billing_id: digest("mayhem/proxy/gateway-billing/v1", &[id.as_bytes()]),
        billing_attempt: 1,
        session_id: digest(
            "mayhem/proxy/gateway-session/v1",
            &[id.as_bytes(), fingerprint.as_bytes()],
        ),
        request_hash: Digest::new(mayhem_proto::endpoint_request_fingerprint(&body))
            .map_err(|_| invalid())?,
    };
    if let Some(job) = load(&state, &id).await? {
        check_replay(&job, &token, &model, &fingerprint, &identity)?;
        return Ok(response(job));
    }
    let binding = Binding {
        job_id: id.clone(),
        token: token.clone(),
        model: model.clone(),
        fingerprint: fingerprint.clone(),
        identity: identity.clone(),
    };
    if request.check_settlement_policy().is_err() {
        return reject_settlement_policy(&state, binding, endpoint_name).await;
    }
    let candidate = proxy_request::resolve(
        state
            .proxy_control
            .as_ref()
            .ok_or_else(unavailable)?
            .clone(),
        request.clone(),
    )
    .await
    .map_err(selection_error)?;
    profile::validate_admission(
        &runtime,
        state
            .proxy_control
            .as_ref()
            .ok_or_else(unavailable)?
            .clone(),
        request.clone(),
        &candidate,
    )
    .await?;
    let stream = if streaming {
        Some(streaming::channel(&runtime)?)
    } else {
        None
    };
    let owner = Arc::new(Owner::new(
        state.jobs.clone(),
        state.access_control.clone(),
        binding.clone(),
    ));
    let gate: Arc<dyn buyer_controller::AuthorizationGate> = match retail {
        Some(correlation) => Arc::new(retail::Gate {
            owner,
            authority: runtime.retail.as_ref().unwrap().clone(),
            correlation,
            job: id.clone(),
        }),
        None => owner,
    };
    let gate = evidence::gate(
        state
            .proxy_control
            .as_ref()
            .ok_or_else(unavailable)?
            .clone(),
        request.clone(),
        gate,
    )
    .await?;
    let gate = data_handling::gate(
        state.proxy_control().cloned().ok_or_else(unavailable)?,
        runtime.controller.clone(),
        request.clone(),
        gate,
    )
    .await?;
    let gate = taxonomy_filters::gate(
        state.proxy_control().cloned().ok_or_else(unavailable)?,
        request.clone(),
        gate,
    )
    .await?;
    let claim = runtime.claim(&id)?;
    let jobs = state.jobs.clone();
    let begin = binding.clone();
    let result = storage(move || {
        jobs.lock()
            .map_err(|_| unavailable())?
            .begin_proxy(
                begin.job_id,
                endpoint_name,
                begin.model,
                Some(begin.token.token_id),
                begin.fingerprint,
                begin.identity,
                now_secs(),
            )
            .map_err(|_| {
                ApiError::conflict(
                    "proxy idempotency record differs or cannot be retained",
                    Some("Idempotency-Key"),
                )
            })
    })
    .await?;
    if let BeginGatewayJob::Existing(job) = result {
        return Ok(response(job));
    }
    let wallet = runtime.controller.identity();
    let context = negotiation::Context {
        schema_version: 1,
        network_id: wallet.network_id.clone(),
        msb_bootstrap: wallet.msb_bootstrap.clone(),
        subnet_bootstrap: wallet.subnet_bootstrap.clone(),
        contract_version: mayhem_proto::CONTRACT_VERSION,
        session_id: identity.session_id.clone(),
        buyer: wallet.controller_pubkey.clone(),
        billing_id: identity.billing_id.clone(),
        billing_attempt: identity.billing_attempt,
        request_hash: identity.request_hash.clone(),
        offer: candidate.offer,
        rail: request.controls().rail,
        settlement_policy_hash: request.controls().settlement_policy_hash.clone(),
    };
    let paid = buyer_controller::Request {
        context,
        body: request.provider_request().to_vec(),
        gate,
        authorization: buyer_controller::Authorization {
            prices: request.controls().prices.clone(),
            output_units: request.controls().output_units,
            lifetimes: request.policy().lifetimes(),
            settlement_policy: runtime.settlement_policy.clone(),
            endpoint_contract: candidate.endpoint_contract,
            recipe_hash: candidate.recipe_hash,
        },
    };
    let (sender, observer) = match stream {
        Some((sender, receiver, permit)) => (Some(sender), Some((receiver, permit))),
        None => (None, None),
    };
    let receiver = runtime.launch((*state).clone(), binding, Some(paid), sender, claim)?;
    if let Some((events, permit)) = observer {
        return Ok(streaming::response(&id, endpoint, events, receiver, permit));
    }
    if gateway_prefers_async_response(&headers) {
        return Ok(gateway_job_pending_response(&id));
    }
    match receiver.await {
        Ok(Ok(job)) => Ok(response(job)),
        // The durable owner remains queryable even if its settlement observation
        // is unavailable. A failed wait never invites a replacement purchase.
        _ => Ok(gateway_job_pending_response(&id)),
    }
}

async fn reject_settlement_policy(
    state: &GatewayState,
    binding: Binding,
    endpoint_name: String,
) -> Result<Response, ApiError> {
    let jobs = state.jobs.clone();
    let id = binding.job_id.clone();
    // An absent read alone cannot prove non-admission: another request may have
    // begun since load(). Claim and retire this exact ID under the same vault
    // lock. Only a newly created, durably unsigned intent permits the safe code.
    // A prior/concurrent purchase, even with identical request fields, is never
    // overwritten or retired here. Persistence uncertainty remains recoverable.
    let rejected = storage(move || {
        let mut jobs = jobs.lock().map_err(|_| unavailable())?;
        match jobs
            .begin_proxy(
                binding.job_id.clone(),
                endpoint_name,
                binding.model,
                Some(binding.token.token_id),
                binding.fingerprint,
                binding.identity,
                now_secs(),
            )
            .map_err(|_| {
                ApiError::conflict(
                    "proxy idempotency record differs or cannot be retained",
                    Some("Idempotency-Key"),
                )
            })? {
            BeginGatewayJob::Existing(job) => Ok(Some(job)),
            BeginGatewayJob::InProgress => Err(unavailable()),
            BeginGatewayJob::Started => {
                jobs.fail_proxy_before_authorization(&binding.job_id, now_secs())
                    .map_err(|_| unavailable())?;
                Ok(None)
            }
        }
    })
    .await?;
    if let Some(job) = rejected {
        return Ok(response(job));
    }
    let mut response = ApiError::bad_request(
        "requested proxy settlement policy differs from the configured policy",
        Some("proxy.settlement_policy_hash"),
    )
    .with_public_error("proxy_settlement_policy_mismatch", "proxy_admission", false)
    .into_response();
    attach_gateway_job_headers(&mut response, &id);
    Ok(response)
}

fn selection_error(error: proxy_request::Error) -> ApiError {
    use proxy_request::Error;
    // Only candidate selection, before begin_proxy, can establish these public
    // non-admission codes. Generic validation and existing-job errors cannot.
    match error {
        // Only reject_settlement_policy can attach a non-admission guarantee to
        // this error, after retaining the durable original unsigned job.
        Error::SettlementPolicyMismatch => invalid(),
        Error::Invalid => {
            invalid().with_public_error("proxy_request_invalid", "proxy_admission", false)
        }
        Error::Constraints => ApiError::bad_request(
            "selected proxy offer does not satisfy the endpoint, rail or context constraints",
            Some("proxy"),
        )
        .with_public_error("proxy_constraints_not_met", "proxy_admission", false),
        Error::Price => ApiError::payment_required(
            "selected proxy offer exceeds the authorized price limits",
            Some("proxy.prices"),
        )
        .with_public_error("proxy_price_limit_exceeded", "proxy_admission", false),
        Error::Verification => ApiError::service_unavailable(
            "verified operator evidence is unavailable for the selected proxy offer",
            Some("proxy"),
        )
        .with_public_error("proxy_verification_unavailable", "proxy_admission", true),
        Error::ProfileEvidence => ApiError::service_unavailable(
            "required proxy routing profile evidence is unavailable",
            Some("proxy.profile"),
        )
        .with_public_error(
            "proxy_profile_evidence_unavailable",
            "proxy_admission",
            true,
        ),
        Error::Catalog => ApiError::service_unavailable(
            "selected proxy offer has no current catalog evidence",
            Some("model"),
        )
        .with_public_error("proxy_catalog_unavailable", "proxy_admission", true),
        Error::Availability(reason) => ApiError::service_unavailable(
            format!("selected proxy offer is not eligible: {reason:?}"),
            Some("model"),
        )
        .with_public_error("proxy_provider_unavailable", "proxy_admission", true),
        Error::Busy => busy(),
    }
}
fn check_replay(
    job: &StoredGatewayJob,
    token: &GatewayTokenAttribution,
    model: &str,
    fingerprint: &str,
    identity: &RequestIdentity,
) -> Result<(), ApiError> {
    if job.owner_token_id.as_deref() != Some(token.token_id.as_str())
        || job.model != model
        || job.request_fingerprint != fingerprint
        || job.proxy.as_ref().map(|p| p.identity()) != Some(identity)
    {
        return Err(ApiError::conflict(
            "Idempotency-Key belongs to a different request",
            Some("Idempotency-Key"),
        ));
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests;
