use std::{
    collections::{BTreeMap, BTreeSet},
    convert::Infallible,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    failover::DEFAULT_MAX_OPEN_ATTEMPTS, verify_tier1_attestation, AttestationVerificationRequest,
    EnclaveContractRecord,
};
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{
        sse::{Event, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use futures_util::stream;
use mayhem_bridge::{BridgeError, ScBridgeClient, ScBridgeConfig};
use mayhem_proto::{
    receipt_signing_bytes, session_accept_signing_bytes, session_frame_head,
    spend_voucher_signing_bytes, AttestationReport, CheckpointPolicy, ReceiptAck, ReceiptBody,
    ReceiptUsage, SessionReceipt, SpendVoucher, SpendVoucherBody, ATTESTATION_ALG,
    ATTESTATION_SCHEMA_VERSION, SESSION_RECEIPT_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::TcpListener;

type SharedState = Arc<GatewayState>;

const EMBEDDED_CATALOG: &str = include_str!("../../../catalog/models.json");

#[derive(Clone, Debug)]
pub struct GatewayState {
    models: Arc<Vec<GatewayModel>>,
    receipts: Arc<Mutex<Vec<StoredReceipt>>>,
    paused_sessions: Arc<Mutex<Vec<PausedSession>>>,
    receipt_config: ReceiptConfig,
    session_backend: Arc<dyn GatewaySessionBackend>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GatewayModel {
    pub id: String,
    pub created: u64,
    pub owned_by: String,
    pub mayhem: MayhemModelInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MayhemModelInfo {
    pub providers_online: u32,
    pub rooms: u32,
    pub price_ref_mu: PriceRefMu,
    pub attestation_tiers: BTreeMap<String, u32>,
    pub caps: ModelCaps,
    pub source: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route_candidates: Vec<GatewayRouteCandidate>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct GatewayRouteCandidate {
    pub provider: String,
    pub enclave_id: String,
    pub room_id: String,
    pub price_ver: u64,
    pub att_tier: u8,
    pub admin_pubkey: String,
    pub artifact_root: String,
    pub manifest_hash: String,
    pub binary_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PriceRefMu {
    pub denom: String,
    pub ver: u64,
    pub in_per_1k: u64,
    pub out_per_1k: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelCaps {
    pub tools: bool,
    pub json: bool,
    pub ctx: u32,
    pub vision: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct StoredReceipt {
    pub voucher: SpendVoucher,
    pub receipt: SessionReceipt,
    pub receipt_ack: ReceiptAck,
}

#[derive(Clone, Debug, Serialize)]
pub struct PausedSession {
    pub session_id: String,
    pub reason: String,
}

#[derive(Clone, Debug)]
struct ReceiptConfig {
    cosign_enabled: bool,
    balance_mu: u64,
    rules_ver: u64,
    checkpoint_every: CheckpointPolicy,
    user_seed: [u8; 32],
    provider_seed: [u8; 32],
    enclave_seed: [u8; 32],
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub stream_options: Option<StreamOptions>,
    #[serde(default)]
    pub tools: Option<Vec<Value>>,
    #[serde(default)]
    pub tool_choice: Option<Value>,
    #[serde(default)]
    pub response_format: Option<Value>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub seed: Option<i64>,
    #[serde(default)]
    pub stop: Option<Value>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: Value,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct StreamOptions {
    #[serde(default)]
    pub include_usage: bool,
}

#[derive(Debug, Deserialize)]
pub struct CompletionRequest {
    pub model: String,
    #[serde(default)]
    pub prompt: Value,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub seed: Option<i64>,
    #[serde(default)]
    pub stop: Option<Value>,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
    param: Option<&'static str>,
}

pub type GatewaySessionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<GatewaySessionResult, GatewaySessionError>> + Send + 'a>>;

pub trait GatewaySessionBackend: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;
    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a>;
}

#[derive(Clone, Debug)]
pub struct GatewaySessionResult {
    pub output: ChatOutput,
    pub backend: String,
    pub direct_session: bool,
    pub provider_receipt: Option<ProviderSignedReceipt>,
}

#[derive(Clone, Debug)]
pub struct ProviderSignedReceipt {
    pub body: ReceiptBody,
    pub enclave_sig: String,
    pub enclave_pubkey: String,
}

#[derive(Clone, Debug)]
pub struct GatewaySessionInvocation {
    pub session_id: String,
    pub user_pubkey: String,
    pub provider_pubkey: Option<String>,
    pub enclave_id: String,
    pub price_ver: u64,
    pub rules_ver: u64,
    pub spend_voucher: SpendVoucher,
    pub attestation: Option<GatewaySessionAttestation>,
    receipt_cosign_enabled: bool,
    receipt_user_seed: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct GatewaySessionAttestation {
    pub contract: EnclaveContractRecord,
    pub trusted_binary_hashes: BTreeSet<String>,
}

#[derive(Clone, Debug)]
pub struct GatewaySessionError {
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug)]
struct LocalOpenAiShapeBackend;

#[derive(Clone, Debug)]
pub struct ScBridgeGatewaySessionConfig {
    pub url: String,
    pub token: String,
    pub open_timeout: Duration,
    pub frame_timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct ScBridgeGatewaySessionBackend {
    config: ScBridgeGatewaySessionConfig,
}

#[derive(Clone, Debug)]
pub struct ChatOutput {
    pub content: Option<String>,
    pub tool_call: Option<ToolCallOutput>,
    pub finish_reason: String,
    pub usage: Usage,
}

#[derive(Clone, Debug)]
pub struct ToolCallOutput {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl GatewayState {
    pub fn from_embedded_catalog() -> Self {
        Self::from_catalog_json(EMBEDDED_CATALOG).unwrap_or_else(|_| Self::fixture())
    }

    pub fn from_catalog_json(catalog: &str) -> Result<Self, serde_json::Error> {
        let root: Value = serde_json::from_str(catalog)?;
        let created = 1_782_950_400;
        let models = root
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| model_from_catalog_value(model, created))
            .collect::<Vec<_>>();
        if models.is_empty() {
            Ok(Self::fixture())
        } else {
            Ok(Self::with_models(models))
        }
    }

    pub fn fixture() -> Self {
        let mut tiers = BTreeMap::new();
        tiers.insert("T1".to_owned(), 1);
        Self::with_models(vec![GatewayModel {
            id: "mayhem/dev-chat-tools".to_owned(),
            created: 1_782_950_400,
            owned_by: "mayhem".to_owned(),
            mayhem: MayhemModelInfo {
                providers_online: 1,
                rooms: 1,
                price_ref_mu: PriceRefMu {
                    denom: "mu_usd".to_owned(),
                    ver: 1,
                    in_per_1k: 20,
                    out_per_1k: 60,
                },
                attestation_tiers: tiers,
                caps: ModelCaps {
                    tools: true,
                    json: true,
                    ctx: 8192,
                    vision: false,
                },
                source: "local-fixture".to_owned(),
                route_candidates: Vec::new(),
            },
        }])
    }

    pub fn from_models(models: Vec<GatewayModel>) -> Self {
        Self::with_models(models)
    }

    fn with_models(models: Vec<GatewayModel>) -> Self {
        Self {
            models: Arc::new(models),
            receipts: Arc::new(Mutex::new(Vec::new())),
            paused_sessions: Arc::new(Mutex::new(Vec::new())),
            receipt_config: ReceiptConfig::default(),
            session_backend: Arc::new(LocalOpenAiShapeBackend),
        }
    }

    pub fn with_receipt_cosign_enabled(mut self, enabled: bool) -> Self {
        self.receipt_config.cosign_enabled = enabled;
        self
    }

    pub fn with_session_backend(mut self, backend: Arc<dyn GatewaySessionBackend>) -> Self {
        self.session_backend = backend;
        self
    }

    pub fn receipts(&self) -> Vec<StoredReceipt> {
        self.receipts
            .lock()
            .expect("receipt store poisoned")
            .clone()
    }

    pub fn paused_sessions(&self) -> Vec<PausedSession> {
        self.paused_sessions
            .lock()
            .expect("paused session store poisoned")
            .clone()
    }

    fn receipt_count(&self) -> usize {
        self.receipts.lock().expect("receipt store poisoned").len()
    }

    fn paused_session_count(&self) -> usize {
        self.paused_sessions
            .lock()
            .expect("paused session store poisoned")
            .len()
    }

    fn record_receipt(&self, receipt: StoredReceipt) {
        self.receipts
            .lock()
            .expect("receipt store poisoned")
            .push(receipt);
    }

    fn pause_session(&self, paused: PausedSession) {
        self.paused_sessions
            .lock()
            .expect("paused session store poisoned")
            .push(paused);
    }

    fn model(&self, id: &str) -> Option<GatewayModel> {
        self.models.iter().find(|model| model.id == id).cloned()
    }

    fn first_model(&self) -> Option<GatewayModel> {
        self.models.first().cloned()
    }
}

impl Default for ReceiptConfig {
    fn default() -> Self {
        Self {
            cosign_enabled: true,
            balance_mu: 1_000_000,
            rules_ver: 1,
            checkpoint_every: CheckpointPolicy {
                tokens: 8192,
                ms: 30000,
            },
            user_seed: [41_u8; 32],
            provider_seed: [42_u8; 32],
            enclave_seed: [43_u8; 32],
        }
    }
}

pub fn openai_router(state: GatewayState) -> Router {
    Router::new()
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(create_chat_completion))
        .route("/v1/completions", post(create_completion))
        .route("/mayhem/status", get(mayhem_status))
        .route("/mayhem/receipts", get(mayhem_receipts))
        .route("/mayhem/balance", get(mayhem_balance))
        .with_state(Arc::new(state))
}

pub async fn serve(bind: SocketAddr, state: GatewayState) -> std::io::Result<()> {
    let listener = TcpListener::bind(bind).await?;
    axum::serve(listener, openai_router(state)).await
}

async fn list_models(State(state): State<SharedState>) -> Response {
    let data = state
        .models
        .iter()
        .map(|model| {
            json!({
                "id": model.id,
                "object": "model",
                "created": model.created,
                "owned_by": model.owned_by,
                "mayhem": model.mayhem,
            })
        })
        .collect::<Vec<_>>();
    Json(json!({ "object": "list", "data": data })).into_response()
}

async fn create_chat_completion(
    State(state): State<SharedState>,
    Json(request): Json<ChatCompletionRequest>,
) -> Response {
    match build_chat_completion(&state, request).await {
        Ok(ChatResponse::Json(value)) => Json(value).into_response(),
        Ok(ChatResponse::Sse(chunks)) => sse_response(chunks),
        Err(err) => err.into_response(),
    }
}

async fn create_completion(
    State(state): State<SharedState>,
    Json(request): Json<CompletionRequest>,
) -> Response {
    match build_completion(&state, request) {
        Ok(ChatResponse::Json(value)) => Json(value).into_response(),
        Ok(ChatResponse::Sse(chunks)) => sse_response(chunks),
        Err(err) => err.into_response(),
    }
}

async fn mayhem_status(State(state): State<SharedState>) -> Response {
    Json(json!({
        "ok": true,
        "version": 1,
        "backend": state.session_backend.name(),
        "models": state.models.len(),
        "sessions_active": 0,
        "sessions_paused": state.paused_session_count(),
        "receipts": state.receipt_count(),
    }))
    .into_response()
}

async fn mayhem_receipts(State(state): State<SharedState>) -> Response {
    Json(json!({
        "object": "list",
        "data": state.receipts(),
        "paused": state.paused_sessions(),
    }))
    .into_response()
}

async fn mayhem_balance(State(state): State<SharedState>) -> Response {
    Json(json!({
        "denom": "mu_usd",
        "balance_mu": state.receipt_config.balance_mu,
        "held_mu": 0
    }))
    .into_response()
}

enum ChatResponse {
    Json(Value),
    Sse(Vec<Value>),
}

#[derive(Clone, Copy)]
struct ResponseMayhemMeta<'a> {
    backend: &'a str,
    direct_session: bool,
}

#[derive(Clone, Debug)]
struct ValidatedDirectSessionAccept {
    enclave_pubkey: String,
}

#[derive(Clone, Debug)]
struct DirectSessionCollected {
    output: ChatOutput,
    provider_receipt: ProviderSignedReceipt,
}

#[derive(Debug, Deserialize)]
struct ProviderReceiptWire {
    #[serde(flatten)]
    body: ReceiptBody,
    enclave_sig: String,
}

struct ExpectedProviderReceipt<'a> {
    provider: &'a str,
    usage: ReceiptUsage,
    mu_owed_cum: u64,
    prompt_hash: String,
}

impl GatewaySessionError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }

    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }

    pub fn into_retryable(mut self) -> Self {
        self.retryable = true;
        self
    }
}

impl From<BridgeError> for GatewaySessionError {
    fn from(error: BridgeError) -> Self {
        Self::new(error.to_string())
    }
}

impl GatewaySessionResult {
    pub fn local_openai_shape(output: ChatOutput) -> Self {
        Self {
            output,
            backend: "local-openai-shape".to_owned(),
            direct_session: false,
            provider_receipt: None,
        }
    }
}

impl GatewaySessionBackend for LocalOpenAiShapeBackend {
    fn name(&self) -> &str {
        "local-openai-shape"
    }

    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            Ok(GatewaySessionResult::local_openai_shape(
                deterministic_chat_output(model, request),
            ))
        })
    }
}

impl ScBridgeGatewaySessionConfig {
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            token: token.into(),
            open_timeout: Duration::from_secs(3),
            frame_timeout: Duration::from_secs(60),
        }
    }
}

impl ScBridgeGatewaySessionBackend {
    pub fn new(config: ScBridgeGatewaySessionConfig) -> Self {
        Self { config }
    }
}

impl GatewaySessionBackend for ScBridgeGatewaySessionBackend {
    fn name(&self) -> &str {
        "sc-bridge-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move { self.run_chat_over_bridge(model, request, invocation).await })
    }
}

impl ScBridgeGatewaySessionBackend {
    async fn run_chat_over_bridge(
        &self,
        model: &GatewayModel,
        request: &ChatCompletionRequest,
        invocation: &GatewaySessionInvocation,
    ) -> Result<GatewaySessionResult, GatewaySessionError> {
        let provider = invocation
            .provider_pubkey
            .as_deref()
            .ok_or_else(|| GatewaySessionError::new("model has no canonical provider route"))?;
        let mut bridge = ScBridgeClient::connect(ScBridgeConfig::new(
            &self.config.url,
            self.config.token.clone(),
        )?)
        .await?;
        bridge
            .session_subscribe([invocation.session_id.as_str()])
            .await?;
        let opened = bridge
            .session_open(provider, &invocation.session_id)
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "opening direct session {} to provider {} failed: {err}",
                    invocation.session_id, provider
                ))
            })?;
        if opened.get("direct").and_then(Value::as_bool) != Some(true)
            || opened.get("relayed").and_then(Value::as_bool) == Some(true)
        {
            return Err(GatewaySessionError::retryable(format!(
                "session {} was not opened as a direct non-relayed channel",
                invocation.session_id
            )));
        }

        let now = now_millis_u64();
        let att_nonce = blake3_hex(format!("att:{}:{now}", invocation.session_id).as_bytes());
        let open_frame = json!({
            "t": "s.open",
            "v": 1,
            "session_id": invocation.session_id,
            "user": invocation.user_pubkey,
            "enclave_id": invocation.enclave_id,
            "price_ver": invocation.price_ver,
            "rules_ver": invocation.rules_ver,
            "voucher": invocation.spend_voucher,
            "att_nonce": att_nonce,
            "ts": now,
            "nonce": blake3_hex(format!("open:{}:{now}", invocation.session_id).as_bytes()),
            "sig": invocation.spend_voucher.user_sig,
        });
        let open_head = session_frame_head(&open_frame)
            .map_err(|err| GatewaySessionError::new(format!("s.open hash failed: {err}")))?;
        bridge
            .session_send(provider, &invocation.session_id, open_frame)
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "sending s.open for session {} to provider {} failed: {err}",
                    invocation.session_id, provider
                ))
            })?;

        let accept = next_session_frame(
            &mut bridge,
            &invocation.session_id,
            self.config.open_timeout,
            &["s.accept", "s.reject"],
        )
        .await
        .map_err(GatewaySessionError::into_retryable)?;
        if accept.get("t").and_then(Value::as_str) == Some("s.reject") {
            let code = accept
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("UNKNOWN");
            return Err(GatewaySessionError::retryable(format!(
                "provider rejected session {} with {code}",
                invocation.session_id
            )));
        }
        let accept_info = validate_direct_session_accept(
            &accept,
            invocation,
            &open_head,
            &att_nonce,
            now / 1000,
        )?;

        let request_id = blake3_hex(
            format!(
                "rid:{}:{}",
                invocation.session_id,
                serde_json::to_string(&request.messages).unwrap_or_default()
            )
            .as_bytes(),
        )
        .chars()
        .take(32)
        .collect::<String>();
        bridge
            .session_send(
                provider,
                &invocation.session_id,
                json!({
                    "t": "s.req",
                    "rid": request_id,
                    "body": direct_session_request_body(request),
                }),
            )
            .await?;

        let collected = collect_direct_session_output(
            &mut bridge,
            &invocation.session_id,
            &request_id,
            self.config.frame_timeout,
            request,
            &accept_info.enclave_pubkey,
        )
        .await
        .map_err(GatewaySessionError::into_retryable)?;
        let receipt_ack = direct_session_receipt_ack(
            request,
            &collected.output,
            invocation,
            &collected.provider_receipt,
            provider,
            model,
        )?;
        bridge
            .session_send(
                provider,
                &invocation.session_id,
                json!({
                    "t": "s.receipt_ack",
                    "v": 1,
                    "session_id": receipt_ack.session_id,
                    "seq": receipt_ack.seq,
                    "user_sig": receipt_ack.user_sig,
                }),
            )
            .await
            .map_err(|err| {
                GatewaySessionError::retryable(format!(
                    "sending s.receipt_ack for session {} to provider {} failed: {err}",
                    invocation.session_id, provider
                ))
            })?;
        let _ = bridge
            .session_send(
                provider,
                &invocation.session_id,
                json!({
                    "t": "s.close",
                    "v": 1,
                    "session_id": invocation.session_id,
                    "reason": "done",
                }),
            )
            .await;
        let _ = bridge.session_close(provider, &invocation.session_id).await;

        Ok(GatewaySessionResult {
            output: collected.output,
            backend: self.name().to_owned(),
            direct_session: true,
            provider_receipt: Some(collected.provider_receipt),
        })
    }
}

fn validate_direct_session_accept(
    frame: &Value,
    invocation: &GatewaySessionInvocation,
    expected_open_head: &str,
    expected_att_nonce: &str,
    now_ts: u64,
) -> Result<ValidatedDirectSessionAccept, GatewaySessionError> {
    let fail = |message: String| GatewaySessionError::new(message);
    if frame.get("t").and_then(Value::as_str) != Some("s.accept") {
        return Err(fail("provider response was not s.accept".to_owned()));
    }
    if frame.get("session_id").and_then(Value::as_str) != Some(invocation.session_id.as_str()) {
        return Err(fail(format!(
            "provider accept session_id did not match {}",
            invocation.session_id
        )));
    }
    if frame.get("open_head").and_then(Value::as_str) != Some(expected_open_head) {
        return Err(fail(
            "provider accept open_head did not match sent s.open".to_owned(),
        ));
    }
    if frame.get("att_nonce").and_then(Value::as_str) != Some(expected_att_nonce) {
        return Err(fail(
            "provider accept att_nonce did not match sent s.open".to_owned(),
        ));
    }
    let report_value = frame
        .get("att_report")
        .cloned()
        .ok_or_else(|| fail("provider accept missing att_report".to_owned()))?;
    let report: AttestationReport = serde_json::from_value(report_value)
        .map_err(|err| fail(format!("provider accept att_report invalid: {err}")))?;
    if report.schema_version != ATTESTATION_SCHEMA_VERSION {
        return Err(fail(format!(
            "provider accept att_report schema_version {} is not supported",
            report.schema_version
        )));
    }
    if report.alg != ATTESTATION_ALG {
        return Err(fail(format!(
            "provider accept att_report alg {} is not supported",
            report.alg
        )));
    }
    if report.enclave_id != invocation.enclave_id {
        return Err(fail(format!(
            "provider accept att_report enclave_id did not match {}",
            invocation.enclave_id
        )));
    }
    if report.nonce_u != expected_att_nonce {
        return Err(fail(
            "provider accept att_report nonce_u did not match sent s.open".to_owned(),
        ));
    }
    let top_sig = frame
        .get("sig")
        .and_then(Value::as_str)
        .ok_or_else(|| fail("provider accept missing sig".to_owned()))?;
    if let Some(provider) = invocation.provider_pubkey.as_deref() {
        let attestation = invocation.attestation.as_ref().ok_or_else(|| {
            fail("provider accept missing admin enclave attestation metadata".to_owned())
        })?;
        let mut request = AttestationVerificationRequest::new(
            &report,
            &attestation.contract,
            &attestation.trusted_binary_hashes,
            expected_att_nonce,
            now_ts,
        );
        request.expected_provider_pubkey = Some(provider);
        verify_tier1_attestation(&request).map_err(|err| {
            fail(format!(
                "provider accept attestation verification failed: {err}"
            ))
        })?;
        verify_direct_session_accept_signature(frame, provider, top_sig)?;
    }
    Ok(ValidatedDirectSessionAccept {
        enclave_pubkey: report.enclave_pubkey,
    })
}

fn verify_direct_session_accept_signature(
    frame: &Value,
    provider_pubkey: &str,
    signature_hex: &str,
) -> Result<(), GatewaySessionError> {
    let provider_key = decode_hex_array::<32>(provider_pubkey, "provider pubkey")?;
    let signature = decode_hex_array::<64>(signature_hex, "provider accept sig")?;
    let verifying_key = VerifyingKey::from_bytes(&provider_key)
        .map_err(|err| GatewaySessionError::new(format!("invalid provider pubkey: {err}")))?;
    let signature = Signature::from_bytes(&signature);
    verifying_key
        .verify(
            &session_accept_signing_bytes(frame).map_err(|err| {
                GatewaySessionError::new(format!("provider accept signing payload failed: {err}"))
            })?,
            &signature,
        )
        .map_err(|err| GatewaySessionError::new(format!("provider accept signature failed: {err}")))
}

fn verify_provider_receipt_signature(
    receipt: &ProviderSignedReceipt,
) -> Result<(), GatewaySessionError> {
    let enclave_key = decode_hex_array::<32>(&receipt.enclave_pubkey, "enclave pubkey")?;
    let signature = decode_hex_array::<64>(&receipt.enclave_sig, "provider receipt enclave sig")?;
    let verifying_key = VerifyingKey::from_bytes(&enclave_key)
        .map_err(|err| GatewaySessionError::new(format!("invalid enclave pubkey: {err}")))?;
    let signature = Signature::from_bytes(&signature);
    verifying_key
        .verify(
            &receipt_signing_bytes(&receipt.body).map_err(|err| {
                GatewaySessionError::new(format!("provider receipt signing payload failed: {err}"))
            })?,
            &signature,
        )
        .map_err(|err| {
            GatewaySessionError::new(format!("provider receipt enclave signature failed: {err}"))
        })
}

fn decode_hex_array<const N: usize>(
    value: &str,
    label: &'static str,
) -> Result<[u8; N], GatewaySessionError> {
    let bytes = hex::decode(value)
        .map_err(|err| GatewaySessionError::new(format!("{label} is not hex: {err}")))?;
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        GatewaySessionError::new(format!("{label} must be {N} bytes, got {}", bytes.len()))
    })
}

async fn next_session_frame(
    bridge: &mut ScBridgeClient,
    session_id: &str,
    wait: Duration,
    expected_types: &[&str],
) -> Result<Value, GatewaySessionError> {
    let deadline = Instant::now() + wait;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(GatewaySessionError::new(format!(
                "timed out waiting for {} on session {}",
                expected_types.join("|"),
                session_id
            )));
        }
        match bridge.next_session_frame(remaining).await {
            Ok(event) => {
                if event.get("session_id").and_then(Value::as_str) != Some(session_id) {
                    continue;
                }
                let frame = event.get("frame").cloned().unwrap_or(Value::Null);
                let frame_type = frame.get("t").and_then(Value::as_str).unwrap_or("");
                if expected_types.contains(&frame_type) {
                    return Ok(frame);
                }
            }
            Err(BridgeError::Timeout) => {
                return Err(GatewaySessionError::new(format!(
                    "timed out waiting for {} on session {}",
                    expected_types.join("|"),
                    session_id
                )));
            }
            Err(err) => return Err(err.into()),
        }
    }
}

fn direct_session_request_body(request: &ChatCompletionRequest) -> Value {
    let mut body = json!({
        "messages": &request.messages,
        "stream": true,
    });
    set_optional_json(
        &mut body,
        "tools",
        request.tools.as_ref().map(|value| json!(value)),
    );
    set_optional_json(
        &mut body,
        "tool_choice",
        request.tool_choice.as_ref().cloned(),
    );
    set_optional_json(
        &mut body,
        "response_format",
        request.response_format.as_ref().cloned(),
    );
    set_optional_json(
        &mut body,
        "temperature",
        request.temperature.map(|value| json!(value)),
    );
    set_optional_json(&mut body, "top_p", request.top_p.map(|value| json!(value)));
    set_optional_json(&mut body, "seed", request.seed.map(|value| json!(value)));
    set_optional_json(&mut body, "stop", request.stop.as_ref().cloned());
    set_optional_json(
        &mut body,
        "max_tokens",
        request.max_tokens.map(|value| json!(value)),
    );
    body
}

fn set_optional_json(body: &mut Value, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        body[key] = value;
    }
}

async fn collect_direct_session_output(
    bridge: &mut ScBridgeClient,
    session_id: &str,
    request_id: &str,
    wait: Duration,
    request: &ChatCompletionRequest,
    enclave_pubkey: &str,
) -> Result<DirectSessionCollected, GatewaySessionError> {
    let mut content = String::new();
    let mut tool_call = None;
    let mut finish_reason = None;
    let mut usage = None;
    let mut provider_receipt = None;
    let deadline = Instant::now() + wait;

    while finish_reason.is_none() || provider_receipt.is_none() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(GatewaySessionError::new(format!(
                "timed out waiting for final s.delta and s.receipt on session {session_id}"
            )));
        }
        let frame = next_session_frame(
            bridge,
            session_id,
            remaining,
            &["s.delta", "s.receipt", "s.close"],
        )
        .await?;
        match frame.get("t").and_then(Value::as_str) {
            Some("s.delta") if frame.get("rid").and_then(Value::as_str) == Some(request_id) => {
                if let Some(delta) = frame.get("d").and_then(Value::as_str) {
                    content.push_str(delta);
                }
                if tool_call.is_none() {
                    tool_call = tool_call_from_session_delta(&frame);
                }
                if let Some(fin) = frame.get("fin").and_then(Value::as_str) {
                    finish_reason = Some(fin.to_owned());
                    usage = usage_from_session_delta(&frame);
                }
            }
            Some("s.receipt") => {
                provider_receipt = Some(provider_signed_receipt_from_frame(
                    &frame,
                    session_id,
                    enclave_pubkey,
                )?);
            }
            Some("s.close") => {
                let reason = frame
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                if finish_reason.is_none() {
                    return Err(GatewaySessionError::new(format!(
                        "provider closed session {session_id} before final delta: {reason}"
                    )));
                }
                return Err(GatewaySessionError::new(format!(
                    "provider closed session {session_id} before s.receipt: {reason}"
                )));
            }
            _ => {}
        }
    }

    let prompt_text = chat_prompt_text(request);
    let usage = usage.unwrap_or_else(|| usage_for(&prompt_text, &content));
    Ok(DirectSessionCollected {
        output: ChatOutput {
            content: tool_call.is_none().then_some(content),
            tool_call,
            finish_reason: finish_reason.expect("loop ended with final delta"),
            usage,
        },
        provider_receipt: provider_receipt.expect("loop ended with provider receipt"),
    })
}

fn provider_signed_receipt_from_frame(
    frame: &Value,
    session_id: &str,
    enclave_pubkey: &str,
) -> Result<ProviderSignedReceipt, GatewaySessionError> {
    let fail = |message: String| GatewaySessionError::new(message);
    if frame.get("session_id").and_then(Value::as_str) != Some(session_id) {
        return Err(fail(format!(
            "provider receipt session_id did not match {session_id}"
        )));
    }
    let receipt_value = frame
        .get("receipt")
        .cloned()
        .ok_or_else(|| fail("provider receipt frame missing receipt".to_owned()))?;
    let receipt: ProviderReceiptWire = serde_json::from_value(receipt_value)
        .map_err(|err| fail(format!("provider receipt invalid: {err}")))?;
    if receipt.body.session_id != session_id {
        return Err(fail(format!(
            "provider receipt body session_id did not match {session_id}"
        )));
    }
    if frame.get("seq").and_then(Value::as_u64) != Some(receipt.body.seq) {
        return Err(fail(
            "provider receipt frame seq did not match receipt body".to_owned(),
        ));
    }
    Ok(ProviderSignedReceipt {
        body: receipt.body,
        enclave_sig: receipt.enclave_sig,
        enclave_pubkey: enclave_pubkey.to_owned(),
    })
}

fn direct_session_receipt_ack(
    request: &ChatCompletionRequest,
    output: &ChatOutput,
    invocation: &GatewaySessionInvocation,
    provider_receipt: &ProviderSignedReceipt,
    provider: &str,
    model: &GatewayModel,
) -> Result<ReceiptAck, GatewaySessionError> {
    if !invocation.receipt_cosign_enabled {
        return Err(GatewaySessionError::new(
            "receipt co-signing refused; session paused",
        ));
    }
    let expected = expected_provider_receipt(model, request, output, provider);
    validate_provider_receipt(model, invocation, provider_receipt, expected)?;
    receipt_ack_for_body(&invocation.receipt_user_seed, &provider_receipt.body).map_err(|err| {
        GatewaySessionError::new(format!(
            "provider receipt ack signing payload failed: {err}"
        ))
    })
}

fn expected_provider_receipt<'a>(
    model: &GatewayModel,
    request: &ChatCompletionRequest,
    output: &ChatOutput,
    provider: &'a str,
) -> ExpectedProviderReceipt<'a> {
    let usage = ReceiptUsage {
        in_tokens: output.usage.prompt_tokens,
        out_tokens: output.usage.completion_tokens,
    };
    ExpectedProviderReceipt {
        provider,
        mu_owed_cum: calculate_mu_owed(&model.mayhem.price_ref_mu, &usage),
        prompt_hash: blake3_hex(chat_prompt_text(request).as_bytes()),
        usage,
    }
}

fn validate_provider_receipt(
    model: &GatewayModel,
    invocation: &GatewaySessionInvocation,
    provider_receipt: &ProviderSignedReceipt,
    expected: ExpectedProviderReceipt<'_>,
) -> Result<(), GatewaySessionError> {
    let body = &provider_receipt.body;
    let checks = [
        (
            body.schema_version == SESSION_RECEIPT_SCHEMA_VERSION,
            "provider receipt schema_version is not supported",
        ),
        (
            body.session_id == invocation.session_id,
            "provider receipt session_id mismatch",
        ),
        (body.seq == 1, "provider receipt seq mismatch"),
        (body.final_receipt, "provider receipt is not final"),
        (
            body.user == invocation.user_pubkey,
            "provider receipt user mismatch",
        ),
        (
            body.provider == expected.provider,
            "provider receipt provider mismatch",
        ),
        (
            body.enclave_id == invocation.enclave_id,
            "provider receipt enclave mismatch",
        ),
        (body.model_id == model.id, "provider receipt model mismatch"),
        (
            body.price_ver == invocation.price_ver,
            "provider receipt price_ver mismatch",
        ),
        (
            body.rules_ver == invocation.rules_ver,
            "provider receipt rules_ver mismatch",
        ),
        (
            body.usage == expected.usage,
            "provider receipt usage mismatch",
        ),
        (
            body.mu_owed_cum == expected.mu_owed_cum,
            "provider receipt amount mismatch",
        ),
        (
            body.prompt_hash == expected.prompt_hash,
            "provider receipt prompt_hash mismatch",
        ),
    ];
    for (ok, message) in checks {
        if !ok {
            return Err(GatewaySessionError::new(message));
        }
    }
    verify_provider_receipt_signature(provider_receipt)
}

fn receipt_ack_for_body(
    user_seed: &[u8; 32],
    body: &ReceiptBody,
) -> Result<ReceiptAck, serde_json::Error> {
    let payload = receipt_signing_bytes(body)?;
    Ok(ReceiptAck {
        session_id: body.session_id.clone(),
        seq: body.seq,
        user_sig: sign_hex(user_seed, &payload),
    })
}

fn tool_call_from_session_delta(frame: &Value) -> Option<ToolCallOutput> {
    let tool = frame.get("tool")?;
    if tool.is_null() {
        return None;
    }
    Some(ToolCallOutput {
        id: tool
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| make_id("call")),
        name: tool
            .get("name")
            .or_else(|| {
                tool.get("function")
                    .and_then(|function| function.get("name"))
            })
            .and_then(Value::as_str)?
            .to_owned(),
        arguments: tool
            .get("arguments")
            .or_else(|| {
                tool.get("function")
                    .and_then(|function| function.get("arguments"))
            })
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| "{}".to_owned()),
    })
}

fn usage_from_session_delta(frame: &Value) -> Option<Usage> {
    let usage = frame.get("usage")?;
    let prompt_tokens = usage
        .get("in")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_u64)?;
    let completion_tokens = usage
        .get("out")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_u64)?;
    Some(Usage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
    })
}

async fn build_chat_completion(
    state: &GatewayState,
    request: ChatCompletionRequest,
) -> Result<ChatResponse, ApiError> {
    let model = require_model(state, &request.model)?;
    if request.messages.is_empty() {
        return Err(ApiError::bad_request(
            "messages must contain at least one item",
            Some("messages"),
        ));
    }
    if request
        .tools
        .as_ref()
        .is_some_and(|tools| !tools.is_empty())
        && !model.mayhem.caps.tools
    {
        return Err(ApiError::bad_request(
            "model does not support tool calling",
            Some("tools"),
        ));
    }
    if request.response_format.is_some() && !model.mayhem.caps.json {
        return Err(ApiError::bad_request(
            "model does not support response_format JSON constraints",
            Some("response_format"),
        ));
    }

    let id = make_id("chatcmpl");
    let created = now_secs();
    let (
        GatewaySessionResult {
            output,
            backend,
            direct_session,
            provider_receipt,
        },
        invocation,
    ) = run_chat_with_route_retry(state, &model, &request).await?;
    let mayhem_meta = ResponseMayhemMeta {
        backend: &backend,
        direct_session,
    };
    let receipt = state.meter_chat_session(
        &model,
        &request,
        &output,
        &invocation,
        provider_receipt.as_ref(),
    )?;
    if request.stream {
        Ok(ChatResponse::Sse(chat_stream_chunks(
            &id,
            created,
            &model.id,
            &output,
            &receipt,
            mayhem_meta,
            request
                .stream_options
                .as_ref()
                .is_some_and(|options| options.include_usage),
        )))
    } else {
        Ok(ChatResponse::Json(chat_response_value(
            &id,
            created,
            &model,
            &output,
            &receipt,
            mayhem_meta,
        )))
    }
}

async fn run_chat_with_route_retry(
    state: &GatewayState,
    model: &GatewayModel,
    request: &ChatCompletionRequest,
) -> Result<(GatewaySessionResult, GatewaySessionInvocation), ApiError> {
    let attempt_count = if model.mayhem.route_candidates.is_empty() {
        1
    } else {
        model
            .mayhem
            .route_candidates
            .len()
            .min(usize::from(DEFAULT_MAX_OPEN_ATTEMPTS))
    };
    let mut last_retryable_error = None;

    for attempt_index in 0..attempt_count {
        let invocation = state.prepare_chat_invocation_for_route(
            model,
            request,
            model.mayhem.route_candidates.get(attempt_index),
        )?;
        match state
            .session_backend
            .run_chat(model, request, &invocation)
            .await
        {
            Ok(result) => return Ok((result, invocation)),
            Err(err) if err.retryable => {
                last_retryable_error = Some(err.message);
            }
            Err(err) => return Err(ApiError::bad_gateway(err.message, Some("model"))),
        }
    }

    Err(ApiError::bad_gateway(
        format!(
            "all {attempt_count} route attempt(s) failed before spend; last error: {}",
            last_retryable_error.unwrap_or_else(|| "no route attempted".to_owned())
        ),
        Some("model"),
    ))
}

fn build_completion(
    state: &GatewayState,
    request: CompletionRequest,
) -> Result<ChatResponse, ApiError> {
    let model = require_model(state, &request.model)?;
    let id = make_id("cmpl");
    let created = now_secs();
    let prompt = prompt_to_text(&request.prompt);
    let max_tokens = request.max_tokens.unwrap_or(64).max(1);
    let text = format!("Mayhem completion: {}", prompt.trim())
        .chars()
        .take(max_tokens as usize * 8)
        .collect::<String>();
    let usage = usage_for(&prompt, &text);
    let receipt_usage = ReceiptUsage {
        in_tokens: usage.prompt_tokens,
        out_tokens: usage.completion_tokens,
    };
    let receipt = state.meter_session(&model, &prompt, receipt_usage)?;
    let chunk = json!({
        "id": id,
        "object": "text_completion",
        "created": created,
        "model": model.id,
        "choices": [{
            "text": text,
            "index": 0,
            "logprobs": null,
            "finish_reason": "stop"
        }],
        "usage": usage,
        "mayhem": {
            "receipt": receipt_summary(&receipt),
        },
    });
    if request.stream {
        Ok(ChatResponse::Sse(vec![chunk]))
    } else {
        Ok(ChatResponse::Json(chunk))
    }
}

impl GatewayState {
    fn prepare_chat_invocation_for_route(
        &self,
        model: &GatewayModel,
        request: &ChatCompletionRequest,
        route: Option<&GatewayRouteCandidate>,
    ) -> Result<GatewaySessionInvocation, ApiError> {
        let prompt_text = chat_prompt_text(request);
        let session_id = session_id_for(&model.id, &prompt_text);
        let enclave_id = route
            .map(|candidate| candidate.enclave_id.clone())
            .unwrap_or_else(|| enclave_id_for_model(&model.id));
        let price_ver = route
            .map(|candidate| candidate.price_ver)
            .unwrap_or(model.mayhem.price_ref_mu.ver);
        let attestation = route.map(|candidate| GatewaySessionAttestation {
            contract: EnclaveContractRecord {
                enclave_id: candidate.enclave_id.clone(),
                admin_pubkey: candidate.admin_pubkey.clone(),
                model_id: model.id.clone(),
                artifact_root: candidate.artifact_root.clone(),
                manifest_hash: candidate.manifest_hash.clone(),
                binary_hash: candidate.binary_hash.clone(),
                att_tier: candidate.att_tier,
            },
            trusted_binary_hashes: BTreeSet::from([candidate.binary_hash.clone()]),
        });
        let max_spend_mu = estimate_max_spend_mu(model, request, &prompt_text);
        if max_spend_mu > self.receipt_config.balance_mu {
            return Err(ApiError::payment_required(
                "insufficient local balance for spend voucher",
                Some("model"),
            ));
        }
        let voucher_body = SpendVoucherBody {
            session_id: session_id.clone(),
            enclave_id: enclave_id.clone(),
            price_ver,
            max_spend_mu,
            checkpoint_every: self.receipt_config.checkpoint_every.clone(),
        };
        let voucher_payload =
            spend_voucher_signing_bytes(&voucher_body).map_err(ApiError::internal)?;
        Ok(GatewaySessionInvocation {
            session_id,
            user_pubkey: verifying_key_hex(&self.receipt_config.user_seed),
            provider_pubkey: route.map(|candidate| candidate.provider.clone()),
            enclave_id,
            price_ver,
            rules_ver: self.receipt_config.rules_ver,
            spend_voucher: SpendVoucher {
                body: voucher_body,
                user_sig: sign_hex(&self.receipt_config.user_seed, &voucher_payload),
            },
            attestation,
            receipt_cosign_enabled: self.receipt_config.cosign_enabled,
            receipt_user_seed: self.receipt_config.user_seed,
        })
    }

    fn meter_chat_session(
        &self,
        model: &GatewayModel,
        request: &ChatCompletionRequest,
        output: &ChatOutput,
        invocation: &GatewaySessionInvocation,
        provider_receipt: Option<&ProviderSignedReceipt>,
    ) -> Result<StoredReceipt, ApiError> {
        let prompt_text = chat_prompt_text(request);
        let usage = ReceiptUsage {
            in_tokens: output.usage.prompt_tokens,
            out_tokens: output.usage.completion_tokens,
        };
        let mu_owed_cum = calculate_mu_owed(&model.mayhem.price_ref_mu, &usage);
        if mu_owed_cum > invocation.spend_voucher.body.max_spend_mu {
            return Err(ApiError::payment_required(
                "session usage exceeded signed spend voucher",
                Some("model"),
            ));
        }

        if !self.receipt_config.cosign_enabled {
            self.pause_session(PausedSession {
                session_id: invocation.session_id.clone(),
                reason: "receipt co-signing refused; session paused".to_owned(),
            });
            return Err(ApiError::conflict(
                "receipt co-signing refused; session paused",
                None,
            ));
        }

        let provider = invocation
            .provider_pubkey
            .clone()
            .unwrap_or_else(|| verifying_key_hex(&self.receipt_config.provider_seed));
        let receipt = if let Some(provider_receipt) = provider_receipt {
            self.cosign_provider_receipt(
                model,
                invocation,
                provider_receipt,
                ExpectedProviderReceipt {
                    provider: &provider,
                    usage: usage.clone(),
                    mu_owed_cum,
                    prompt_hash: blake3_hex(prompt_text.as_bytes()),
                },
            )?
        } else {
            let body = ReceiptBody {
                schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
                session_id: invocation.session_id.clone(),
                seq: 1,
                final_receipt: true,
                user: invocation.user_pubkey.clone(),
                provider,
                enclave_id: invocation.enclave_id.clone(),
                model_id: model.id.clone(),
                price_ver: invocation.price_ver,
                rules_ver: invocation.rules_ver,
                usage,
                mu_owed_cum,
                prompt_hash: blake3_hex(prompt_text.as_bytes()),
                ts: now_millis_u64(),
            };
            let receipt_payload = receipt_signing_bytes(&body).map_err(ApiError::internal)?;
            let user_sig = sign_hex(&self.receipt_config.user_seed, &receipt_payload);
            SessionReceipt {
                body,
                enclave_sig: sign_hex(&self.receipt_config.enclave_seed, &receipt_payload),
                user_sig,
            }
        };
        let user_sig = receipt.user_sig.clone();
        let receipt_ack = ReceiptAck {
            session_id: receipt.body.session_id.clone(),
            seq: receipt.body.seq,
            user_sig,
        };
        let stored = StoredReceipt {
            voucher: invocation.spend_voucher.clone(),
            receipt,
            receipt_ack,
        };
        self.record_receipt(stored.clone());
        Ok(stored)
    }

    fn cosign_provider_receipt(
        &self,
        model: &GatewayModel,
        invocation: &GatewaySessionInvocation,
        provider_receipt: &ProviderSignedReceipt,
        expected: ExpectedProviderReceipt<'_>,
    ) -> Result<SessionReceipt, ApiError> {
        let body = &provider_receipt.body;
        validate_provider_receipt(model, invocation, provider_receipt, expected)
            .map_err(|err| ApiError::bad_gateway(err.message, Some("model")))?;
        let receipt_ack = receipt_ack_for_body(&self.receipt_config.user_seed, body)
            .map_err(ApiError::internal)?;
        Ok(SessionReceipt {
            body: body.clone(),
            enclave_sig: provider_receipt.enclave_sig.clone(),
            user_sig: receipt_ack.user_sig,
        })
    }

    fn meter_session(
        &self,
        model: &GatewayModel,
        prompt_text: &str,
        usage: ReceiptUsage,
    ) -> Result<StoredReceipt, ApiError> {
        let mu_owed_cum = calculate_mu_owed(&model.mayhem.price_ref_mu, &usage);
        let max_spend_mu = mu_owed_cum.max(1_000);
        if max_spend_mu > self.receipt_config.balance_mu {
            return Err(ApiError::payment_required(
                "insufficient local balance for spend voucher",
                Some("model"),
            ));
        }

        let session_id = session_id_for(&model.id, prompt_text);
        let enclave_id = enclave_id_for_model(&model.id);
        let voucher_body = SpendVoucherBody {
            session_id: session_id.clone(),
            enclave_id: enclave_id.clone(),
            price_ver: model.mayhem.price_ref_mu.ver,
            max_spend_mu,
            checkpoint_every: self.receipt_config.checkpoint_every.clone(),
        };
        let voucher_payload =
            spend_voucher_signing_bytes(&voucher_body).map_err(ApiError::internal)?;
        let voucher = SpendVoucher {
            body: voucher_body,
            user_sig: sign_hex(&self.receipt_config.user_seed, &voucher_payload),
        };

        if !self.receipt_config.cosign_enabled {
            self.pause_session(PausedSession {
                session_id,
                reason: "receipt co-signing refused; session paused".to_owned(),
            });
            return Err(ApiError::conflict(
                "receipt co-signing refused; session paused",
                None,
            ));
        }

        let body = ReceiptBody {
            schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
            session_id: session_id.clone(),
            seq: 1,
            final_receipt: true,
            user: verifying_key_hex(&self.receipt_config.user_seed),
            provider: verifying_key_hex(&self.receipt_config.provider_seed),
            enclave_id,
            model_id: model.id.clone(),
            price_ver: model.mayhem.price_ref_mu.ver,
            rules_ver: self.receipt_config.rules_ver,
            usage,
            mu_owed_cum,
            prompt_hash: blake3_hex(prompt_text.as_bytes()),
            ts: now_millis_u64(),
        };
        let receipt_payload = receipt_signing_bytes(&body).map_err(ApiError::internal)?;
        let user_sig = sign_hex(&self.receipt_config.user_seed, &receipt_payload);
        let receipt = SessionReceipt {
            body,
            enclave_sig: sign_hex(&self.receipt_config.enclave_seed, &receipt_payload),
            user_sig: user_sig.clone(),
        };
        let receipt_ack = ReceiptAck {
            session_id: receipt.body.session_id.clone(),
            seq: receipt.body.seq,
            user_sig,
        };
        let stored = StoredReceipt {
            voucher,
            receipt,
            receipt_ack,
        };
        self.record_receipt(stored.clone());
        Ok(stored)
    }
}

fn deterministic_chat_output(model: &GatewayModel, request: &ChatCompletionRequest) -> ChatOutput {
    let prompt_text = request
        .messages
        .iter()
        .map(message_to_text)
        .collect::<Vec<_>>()
        .join("\n");
    let output = if let Some(tool_result) = last_tool_result(&request.messages) {
        ChatOutput {
            content: Some(format!("Tool result received: {tool_result}")),
            tool_call: None,
            finish_reason: "stop".to_owned(),
            usage: usage_for(&prompt_text, &tool_result),
        }
    } else if let Some(name) = requested_tool_name(request) {
        let tool_call = ToolCallOutput {
            id: make_id("call"),
            arguments: deterministic_tool_arguments(&name),
            name,
        };
        ChatOutput {
            content: None,
            tool_call: Some(tool_call),
            finish_reason: "tool_calls".to_owned(),
            usage: usage_for(&prompt_text, "{}"),
        }
    } else {
        let content = if wants_json(&request.response_format) {
            json!({
                "ok": true,
                "model": model.id,
                "mayhem": { "response_format": request.response_format },
            })
            .to_string()
        } else {
            format!(
                "Mayhem response from {}: {}",
                model.id,
                last_user_text(&request.messages)
            )
        };
        ChatOutput {
            usage: usage_for(&prompt_text, &content),
            content: Some(content),
            tool_call: None,
            finish_reason: "stop".to_owned(),
        }
    };
    output
}

fn chat_response_value(
    id: &str,
    created: u64,
    model: &GatewayModel,
    output: &ChatOutput,
    receipt: &StoredReceipt,
    mayhem_meta: ResponseMayhemMeta<'_>,
) -> Value {
    let message = if let Some(tool_call) = &output.tool_call {
        json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [tool_call_value(tool_call)],
        })
    } else {
        json!({
            "role": "assistant",
            "content": output.content.clone().unwrap_or_default(),
        })
    };
    json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": model.id,
        "choices": [{
            "index": 0,
            "message": message,
            "logprobs": null,
            "finish_reason": output.finish_reason,
        }],
        "usage": output.usage,
        "mayhem": {
            "backend": mayhem_meta.backend,
            "direct_session": mayhem_meta.direct_session,
            "model": model.mayhem,
            "receipt": receipt_summary(receipt),
        },
    })
}

fn chat_stream_chunks(
    id: &str,
    created: u64,
    model: &str,
    output: &ChatOutput,
    receipt: &StoredReceipt,
    mayhem_meta: ResponseMayhemMeta<'_>,
    include_usage: bool,
) -> Vec<Value> {
    let mut chunks = vec![chat_chunk(
        id,
        created,
        model,
        json!({ "role": "assistant" }),
        None,
        None,
    )];
    if let Some(tool_call) = &output.tool_call {
        chunks.push(chat_chunk(
            id,
            created,
            model,
            json!({ "tool_calls": [tool_call_value(tool_call)] }),
            None,
            None,
        ));
    } else if let Some(content) = &output.content {
        for part in stream_parts(content) {
            chunks.push(chat_chunk(
                id,
                created,
                model,
                json!({ "content": part }),
                None,
                None,
            ));
        }
    }
    chunks.push(chat_chunk(
        id,
        created,
        model,
        json!({}),
        Some(output.finish_reason.as_str()),
        None,
    ));
    if include_usage {
        chunks.push(json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [],
            "usage": output.usage,
            "mayhem": {
                "backend": mayhem_meta.backend,
                "direct_session": mayhem_meta.direct_session,
                "receipt": receipt_summary(receipt),
            },
        }));
    }
    chunks
}

fn chat_chunk(
    id: &str,
    created: u64,
    model: &str,
    delta: Value,
    finish_reason: Option<&str>,
    usage: Option<Usage>,
) -> Value {
    json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": delta,
            "logprobs": null,
            "finish_reason": finish_reason,
        }],
        "usage": usage,
    })
}

fn tool_call_value(tool_call: &ToolCallOutput) -> Value {
    json!({
        "id": tool_call.id,
        "type": "function",
        "function": {
            "name": tool_call.name,
            "arguments": tool_call.arguments,
        }
    })
}

fn receipt_summary(receipt: &StoredReceipt) -> Value {
    json!({
        "session_id": receipt.receipt.body.session_id,
        "seq": receipt.receipt.body.seq,
        "final": receipt.receipt.body.final_receipt,
        "mu_owed_cum": receipt.receipt.body.mu_owed_cum,
        "prompt_hash": receipt.receipt.body.prompt_hash,
        "receipt_ack": receipt.receipt_ack,
    })
}

fn sse_response(chunks: Vec<Value>) -> Response {
    let events = chunks
        .into_iter()
        .map(|chunk| {
            Ok::<Event, Infallible>(
                Event::default()
                    .json_data(chunk)
                    .unwrap_or_else(|_| Event::default().data("{}")),
            )
        })
        .chain(std::iter::once(Ok(Event::default().data("[DONE]"))));
    let mut response = Sse::new(stream::iter(events)).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache"),
    );
    response
}

fn model_from_catalog_value(model: &Value, created: u64) -> Option<GatewayModel> {
    let id = model.get("model_id")?.as_str()?.to_owned();
    let caps = model.get("caps").unwrap_or(&Value::Null);
    let price = model.get("price_ref_mu").unwrap_or(&Value::Null);
    let tiers = attestation_tiers_from_catalog_value(model);
    Some(GatewayModel {
        id,
        created,
        owned_by: "mayhem".to_owned(),
        mayhem: MayhemModelInfo {
            providers_online: 0,
            rooms: 0,
            price_ref_mu: PriceRefMu {
                denom: price
                    .get("denom")
                    .and_then(Value::as_str)
                    .unwrap_or("mu_usd")
                    .to_owned(),
                ver: price
                    .get("ver")
                    .or_else(|| price.get("price_ver"))
                    .and_then(Value::as_u64)
                    .unwrap_or(1),
                in_per_1k: price.get("in_per_1k").and_then(Value::as_u64).unwrap_or(0),
                out_per_1k: price.get("out_per_1k").and_then(Value::as_u64).unwrap_or(0),
            },
            attestation_tiers: tiers,
            caps: ModelCaps {
                tools: caps.get("tools").and_then(Value::as_bool).unwrap_or(false),
                json: caps.get("json").and_then(Value::as_bool).unwrap_or(false),
                ctx: caps
                    .get("ctx_max")
                    .and_then(Value::as_u64)
                    .and_then(|ctx| u32::try_from(ctx).ok())
                    .unwrap_or(0),
                vision: caps.get("vision").and_then(Value::as_bool).unwrap_or(false),
            },
            source: "catalog".to_owned(),
            route_candidates: Vec::new(),
        },
    })
}

fn attestation_tiers_from_catalog_value(model: &Value) -> BTreeMap<String, u32> {
    let mut tiers = BTreeMap::new();
    if let Some(object) = model.get("attestation_tiers").and_then(Value::as_object) {
        for (tier, count) in object {
            if let Some(count) = count.as_u64().and_then(|count| u32::try_from(count).ok()) {
                tiers.insert(tier.clone(), count);
            }
        }
    }
    if tiers.is_empty() {
        tiers.insert("T1".to_owned(), 0);
    }
    tiers
}

fn require_model(state: &GatewayState, model: &str) -> Result<GatewayModel, ApiError> {
    if let Some(model) = state.model(model) {
        return Ok(model);
    }
    if model == "mayhem/default" {
        if let Some(model) = state.first_model() {
            return Ok(model);
        }
    }
    Err(ApiError {
        status: StatusCode::NOT_FOUND,
        message: format!("model '{model}' is not available"),
        param: Some("model"),
    })
}

fn calculate_mu_owed(price: &PriceRefMu, usage: &ReceiptUsage) -> u64 {
    let raw = u128::from(usage.in_tokens) * u128::from(price.in_per_1k)
        + u128::from(usage.out_tokens) * u128::from(price.out_per_1k);
    let rounded = if raw == 0 { 0 } else { raw.div_ceil(1000) };
    rounded.min(u128::from(u64::MAX)) as u64
}

fn session_id_for(model_id: &str, prompt_text: &str) -> String {
    blake3_hex(format!("{model_id}:{prompt_text}:{}", now_millis()).as_bytes())
}

fn enclave_id_for_model(model_id: &str) -> String {
    blake3_hex(format!("mayhem-local-enclave:{model_id}").as_bytes())
}

fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn sign_hex(seed: &[u8; 32], payload: &[u8]) -> String {
    let key = SigningKey::from_bytes(seed);
    hex::encode(key.sign(payload).to_bytes())
}

fn verifying_key_hex(seed: &[u8; 32]) -> String {
    hex::encode(SigningKey::from_bytes(seed).verifying_key().to_bytes())
}

fn requested_tool_name(request: &ChatCompletionRequest) -> Option<String> {
    if matches_tool_choice_none(&request.tool_choice) {
        return None;
    }
    if let Some(name) = tool_choice_function_name(&request.tool_choice) {
        return Some(name);
    }
    request.tools.as_ref()?.iter().find_map(|tool| {
        tool.get("function")?
            .get("name")?
            .as_str()
            .map(str::to_owned)
    })
}

fn deterministic_tool_arguments(name: &str) -> String {
    match name {
        "bash" => json!({ "command": "printf mayhem-opencode-tool-ok" }).to_string(),
        "write" => {
            json!({ "filePath": "mayhem-opencode-tool-ok.txt", "content": "mayhem-opencode-tool-ok" })
                .to_string()
        }
        _ => "{}".to_owned(),
    }
}

fn matches_tool_choice_none(value: &Option<Value>) -> bool {
    matches!(value, Some(Value::String(choice)) if choice == "none")
}

fn tool_choice_function_name(value: &Option<Value>) -> Option<String> {
    value
        .as_ref()?
        .get("function")?
        .get("name")?
        .as_str()
        .map(str::to_owned)
}

fn wants_json(value: &Option<Value>) -> bool {
    matches!(
        value
            .as_ref()
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str),
        Some("json_object" | "json_schema")
    )
}

fn last_tool_result(messages: &[ChatMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "tool")
        .map(message_to_text)
}

fn last_user_text(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(message_to_text)
        .unwrap_or_default()
}

fn message_to_text(message: &ChatMessage) -> String {
    content_to_text(&message.content)
}

fn chat_prompt_text(request: &ChatCompletionRequest) -> String {
    request
        .messages
        .iter()
        .map(message_to_text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn content_to_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| content_to_text(part))
            })
            .collect::<Vec<_>>()
            .join(""),
        other => other.to_string(),
    }
}

fn prompt_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(content_to_text)
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn stream_parts(content: &str) -> Vec<String> {
    let mut parts = content
        .split_inclusive(' ')
        .map(str::to_owned)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() && !content.is_empty() {
        parts.push(content.to_owned());
    }
    parts
}

fn usage_for(input: &str, output: &str) -> Usage {
    let prompt_tokens = rough_tokens(input);
    let completion_tokens = rough_tokens(output);
    Usage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
    }
}

fn estimate_max_spend_mu(
    model: &GatewayModel,
    request: &ChatCompletionRequest,
    prompt_text: &str,
) -> u64 {
    let usage = ReceiptUsage {
        in_tokens: rough_tokens(prompt_text),
        out_tokens: u64::from(request.max_tokens.unwrap_or(1024).max(1)),
    };
    calculate_mu_owed(&model.mayhem.price_ref_mu, &usage).max(1_000)
}

fn rough_tokens(text: &str) -> u64 {
    if text.trim().is_empty() {
        0
    } else {
        text.split_whitespace().count() as u64
    }
}

fn make_id(prefix: &str) -> String {
    format!("{prefix}-mayhem-{}", now_millis())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_secs()
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_millis()
}

fn now_millis_u64() -> u64 {
    match u64::try_from(now_millis()) {
        Ok(millis) => millis,
        Err(_) => u64::MAX,
    }
}

impl ApiError {
    fn bad_request(message: impl Into<String>, param: Option<&'static str>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
            param,
        }
    }

    fn payment_required(message: impl Into<String>, param: Option<&'static str>) -> Self {
        Self {
            status: StatusCode::PAYMENT_REQUIRED,
            message: message.into(),
            param,
        }
    }

    fn conflict(message: impl Into<String>, param: Option<&'static str>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
            param,
        }
    }

    fn bad_gateway(message: impl Into<String>, param: Option<&'static str>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: message.into(),
            param,
        }
    }

    fn internal(err: serde_json::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("receipt signing payload failed: {err}"),
            param: None,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "error": {
                    "message": self.message,
                    "type": "invalid_request_error",
                    "param": self.param,
                    "code": null,
                }
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mayhem_proto::{attestation_signing_bytes, AttestationSigner};

    #[test]
    fn direct_session_accept_pins_session_enclave_provider_and_signature() {
        let invocation = test_invocation();
        let frame = test_accept_frame(&invocation);
        validate_direct_session_accept(
            &frame,
            &invocation,
            test_open_head().as_str(),
            test_att_nonce().as_str(),
            test_now_ts(),
        )
        .expect("matching accept is valid");

        let mut wrong_session = frame.clone();
        wrong_session["session_id"] = json!("bb".repeat(32));
        assert_accept_err(&wrong_session, &invocation, "session_id");

        let mut wrong_open_head = frame.clone();
        wrong_open_head["open_head"] = json!("99".repeat(32));
        assert_accept_err(&wrong_open_head, &invocation, "open_head");

        let mut wrong_att_nonce = frame.clone();
        wrong_att_nonce["att_nonce"] = json!("98".repeat(32));
        assert_accept_err(&wrong_att_nonce, &invocation, "att_nonce");

        let mut wrong_enclave = frame.clone();
        wrong_enclave["att_report"]["enclave_id"] = json!("cc".repeat(32));
        assert_accept_err(&wrong_enclave, &invocation, "enclave_id");

        let mut wrong_report_nonce = frame.clone();
        wrong_report_nonce["att_report"] =
            test_attestation_report_with_nonce(&invocation, "97".repeat(32));
        sign_accept_frame(&mut wrong_report_nonce);
        assert_accept_err(&wrong_report_nonce, &invocation, "nonce_u");

        let mut wrong_manifest = frame.clone();
        wrong_manifest["att_report"] =
            test_attestation_report_with_mutation(&invocation, test_att_nonce(), |report| {
                report.manifest_hash = "ab".repeat(32);
            });
        sign_accept_frame(&mut wrong_manifest);
        assert_accept_err(&wrong_manifest, &invocation, "manifest_hash");

        let mut wrong_provider = frame.clone();
        wrong_provider["att_report"]["provider_pubkey"] = json!("dd".repeat(32));
        assert_accept_err(&wrong_provider, &invocation, "provider_pubkey");

        let mut wrong_report_sig = frame.clone();
        wrong_report_sig["att_report"]["sig_provider"] = json!("12".repeat(64));
        sign_accept_frame(&mut wrong_report_sig);
        assert_accept_err(&wrong_report_sig, &invocation, "provider signature");

        let mut wrong_sig = frame;
        wrong_sig["sig"] = json!("ee".repeat(64));
        assert_accept_err(&wrong_sig, &invocation, "signature");
    }

    #[test]
    fn provider_signed_receipt_must_match_admin_terms_usage_and_enclave_key() {
        let state = GatewayState::fixture();
        let model = test_model();
        let request = test_chat_request(&model.id);
        let output = test_chat_output();
        let invocation = test_invocation();
        let provider_receipt = test_provider_receipt(&model, &request, &output, &invocation);

        let stored = state
            .meter_chat_session(
                &model,
                &request,
                &output,
                &invocation,
                Some(&provider_receipt),
            )
            .expect("matching provider receipt is co-signed");
        assert_eq!(stored.receipt.enclave_sig, provider_receipt.enclave_sig);
        assert_eq!(stored.receipt.body, provider_receipt.body);
        assert_eq!(stored.receipt_ack.user_sig, stored.receipt.user_sig);

        let live_ack = direct_session_receipt_ack(
            &request,
            &output,
            &invocation,
            &provider_receipt,
            invocation.provider_pubkey.as_deref().unwrap(),
            &model,
        )
        .expect("direct session ack signs the accepted receipt");
        assert_eq!(live_ack, stored.receipt_ack);

        let mut wrong_amount = provider_receipt.clone();
        wrong_amount.body.mu_owed_cum = wrong_amount.body.mu_owed_cum.saturating_add(1);
        wrong_amount.enclave_sig = sign_hex(
            &test_enclave_seed(),
            &receipt_signing_bytes(&wrong_amount.body).unwrap(),
        );
        let err = state
            .meter_chat_session(&model, &request, &output, &invocation, Some(&wrong_amount))
            .expect_err("wrong amount must be rejected");
        assert!(err.message.contains("amount"));

        let mut wrong_sig = provider_receipt;
        wrong_sig.enclave_sig = "11".repeat(64);
        let err = state
            .meter_chat_session(&model, &request, &output, &invocation, Some(&wrong_sig))
            .expect_err("wrong enclave signature must be rejected");
        assert!(err.message.contains("signature"));
    }

    fn assert_accept_err(frame: &Value, invocation: &GatewaySessionInvocation, needle: &str) {
        let err = validate_direct_session_accept(
            frame,
            invocation,
            test_open_head().as_str(),
            test_att_nonce().as_str(),
            test_now_ts(),
        )
        .expect_err("mutated accept must be rejected");
        assert!(
            err.message.contains(needle),
            "expected error to contain {needle}, got {}",
            err.message
        );
    }

    fn test_invocation() -> GatewaySessionInvocation {
        let identity = test_identity();
        let session_id = "aa".repeat(32);
        let enclave_id = mayhem_proto::catalog_enclave_id(&identity);
        let contract = EnclaveContractRecord {
            enclave_id: enclave_id.clone(),
            admin_pubkey: identity.admin_pubkey.clone(),
            model_id: identity.model_id.clone(),
            artifact_root: identity.artifact_root.clone(),
            manifest_hash: identity.manifest_hash.clone(),
            binary_hash: identity.binary_hash.clone(),
            att_tier: 1,
        };
        let voucher_body = SpendVoucherBody {
            session_id: session_id.clone(),
            enclave_id: enclave_id.clone(),
            price_ver: 7,
            max_spend_mu: 1000,
            checkpoint_every: CheckpointPolicy {
                tokens: 128,
                ms: 30_000,
            },
        };
        GatewaySessionInvocation {
            session_id,
            user_pubkey: verifying_key_hex(&test_user_seed()),
            provider_pubkey: Some(verifying_key_hex(&test_provider_seed())),
            enclave_id,
            price_ver: 7,
            rules_ver: 3,
            spend_voucher: SpendVoucher {
                body: voucher_body,
                user_sig: "44".repeat(64),
            },
            attestation: Some(GatewaySessionAttestation {
                contract,
                trusted_binary_hashes: BTreeSet::from([identity.binary_hash]),
            }),
            receipt_cosign_enabled: true,
            receipt_user_seed: test_user_seed(),
        }
    }

    fn test_model() -> GatewayModel {
        GatewayModel {
            id: test_identity().model_id,
            created: 1,
            owned_by: "mayhem".to_owned(),
            mayhem: MayhemModelInfo {
                providers_online: 1,
                rooms: 1,
                price_ref_mu: PriceRefMu {
                    denom: "mu_usd".to_owned(),
                    ver: 7,
                    in_per_1k: 20,
                    out_per_1k: 60,
                },
                attestation_tiers: BTreeMap::from([("T1".to_owned(), 1)]),
                caps: ModelCaps {
                    tools: true,
                    json: true,
                    ctx: 8192,
                    vision: false,
                },
                source: "test".to_owned(),
                route_candidates: Vec::new(),
            },
        }
    }

    fn test_chat_request(model_id: &str) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: model_id.to_owned(),
            messages: vec![ChatMessage {
                role: "user".to_owned(),
                content: json!("hello mayhem"),
                name: None,
                extra: BTreeMap::new(),
            }],
            stream: false,
            stream_options: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            temperature: None,
            top_p: None,
            seed: None,
            stop: None,
            max_tokens: None,
        }
    }

    fn test_chat_output() -> ChatOutput {
        ChatOutput {
            content: Some("receipt ok".to_owned()),
            tool_call: None,
            finish_reason: "stop".to_owned(),
            usage: Usage {
                prompt_tokens: 2,
                completion_tokens: 3,
                total_tokens: 5,
            },
        }
    }

    fn test_provider_receipt(
        model: &GatewayModel,
        request: &ChatCompletionRequest,
        output: &ChatOutput,
        invocation: &GatewaySessionInvocation,
    ) -> ProviderSignedReceipt {
        let usage = ReceiptUsage {
            in_tokens: output.usage.prompt_tokens,
            out_tokens: output.usage.completion_tokens,
        };
        let body = ReceiptBody {
            schema_version: SESSION_RECEIPT_SCHEMA_VERSION,
            session_id: invocation.session_id.clone(),
            seq: 1,
            final_receipt: true,
            user: invocation.user_pubkey.clone(),
            provider: invocation.provider_pubkey.clone().unwrap(),
            enclave_id: invocation.enclave_id.clone(),
            model_id: model.id.clone(),
            price_ver: invocation.price_ver,
            rules_ver: invocation.rules_ver,
            usage: usage.clone(),
            mu_owed_cum: calculate_mu_owed(&model.mayhem.price_ref_mu, &usage),
            prompt_hash: blake3_hex(chat_prompt_text(request).as_bytes()),
            ts: 123,
        };
        ProviderSignedReceipt {
            enclave_sig: sign_hex(&test_enclave_seed(), &receipt_signing_bytes(&body).unwrap()),
            body,
            enclave_pubkey: verifying_key_hex(&test_enclave_seed()),
        }
    }

    fn test_identity() -> mayhem_proto::CatalogEnclaveIdentity {
        mayhem_proto::CatalogEnclaveIdentity {
            admin_pubkey: "33".repeat(32),
            model_id: "mayhem/test-model@q4".to_owned(),
            artifact_root: "aa".repeat(32),
            manifest_hash: "bb".repeat(32),
            binary_hash: "cc".repeat(32),
        }
    }

    fn test_accept_frame(invocation: &GatewaySessionInvocation) -> Value {
        let mut frame = json!({
            "t": "s.accept",
            "v": 1,
            "session_id": invocation.session_id,
            "open_head": test_open_head(),
            "att_nonce": test_att_nonce(),
            "att_report": test_attestation_report(invocation),
            "engine": { "ctx": 8192 },
            "ts": 123,
            "nonce": "66".repeat(32),
        });
        sign_accept_frame(&mut frame);
        frame
    }

    fn test_attestation_report(invocation: &GatewaySessionInvocation) -> Value {
        test_attestation_report_with_nonce(invocation, test_att_nonce())
    }

    fn test_attestation_report_with_nonce(
        invocation: &GatewaySessionInvocation,
        nonce_u: String,
    ) -> Value {
        test_attestation_report_with_mutation(invocation, nonce_u, |_| {})
    }

    fn test_attestation_report_with_mutation<F>(
        invocation: &GatewaySessionInvocation,
        nonce_u: String,
        mutate: F,
    ) -> Value
    where
        F: FnOnce(&mut AttestationReport),
    {
        let contract = &invocation
            .attestation
            .as_ref()
            .expect("test invocation has attestation")
            .contract;
        let mut report = AttestationReport {
            schema_version: ATTESTATION_SCHEMA_VERSION,
            alg: ATTESTATION_ALG.to_owned(),
            enclave_id: invocation.enclave_id.clone(),
            enclave_pubkey: verifying_key_hex(&test_enclave_seed()),
            provider_pubkey: invocation.provider_pubkey.clone().unwrap(),
            manifest_hash: contract.manifest_hash.clone(),
            binary_hash: contract.binary_hash.clone(),
            att_tier: contract.att_tier,
            hw_quote: None,
            boot_epoch: 1,
            report_ts: 2,
            nonce_u,
            sig_enclave: String::new(),
            sig_provider: String::new(),
        };
        mutate(&mut report);
        let body = report.body();
        report.sig_enclave = sign_hex(
            &test_enclave_seed(),
            &attestation_signing_bytes(&body, AttestationSigner::Enclave).unwrap(),
        );
        report.sig_provider = sign_hex(
            &test_provider_seed(),
            &attestation_signing_bytes(&body, AttestationSigner::Provider).unwrap(),
        );
        serde_json::to_value(report).unwrap()
    }

    fn sign_accept_frame(frame: &mut Value) {
        let sig = sign_hex(
            &test_provider_seed(),
            &session_accept_signing_bytes(frame).unwrap(),
        );
        frame["sig"] = json!(sig);
    }

    fn test_provider_seed() -> [u8; 32] {
        [7; 32]
    }

    fn test_user_seed() -> [u8; 32] {
        [41; 32]
    }

    fn test_enclave_seed() -> [u8; 32] {
        [8; 32]
    }

    fn test_open_head() -> String {
        "77".repeat(32)
    }

    fn test_att_nonce() -> String {
        "88".repeat(32)
    }

    fn test_now_ts() -> u64 {
        210
    }
}
