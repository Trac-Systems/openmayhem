use std::{
    collections::BTreeMap,
    convert::Infallible,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
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
use ed25519_dalek::{Signer, SigningKey};
use futures_util::stream;
use mayhem_proto::{
    receipt_signing_bytes, spend_voucher_signing_bytes, CheckpointPolicy, ReceiptAck, ReceiptBody,
    ReceiptUsage, SessionReceipt, SpendVoucher, SpendVoucherBody, SESSION_RECEIPT_SCHEMA_VERSION,
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

#[derive(Clone, Debug)]
struct ChatOutput {
    content: Option<String>,
    tool_call: Option<ToolCallOutput>,
    finish_reason: &'static str,
    usage: Usage,
}

#[derive(Clone, Debug)]
struct ToolCallOutput {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Clone, Debug, Serialize)]
struct Usage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
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
            },
        }])
    }

    fn with_models(models: Vec<GatewayModel>) -> Self {
        Self {
            models: Arc::new(models),
            receipts: Arc::new(Mutex::new(Vec::new())),
            paused_sessions: Arc::new(Mutex::new(Vec::new())),
            receipt_config: ReceiptConfig::default(),
        }
    }

    pub fn with_receipt_cosign_enabled(mut self, enabled: bool) -> Self {
        self.receipt_config.cosign_enabled = enabled;
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
    match build_chat_completion(&state, request) {
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
        "backend": "local-openai-shape",
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

fn build_chat_completion(
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
    let output = deterministic_chat_output(&model, &request);
    let receipt = state.meter_chat_session(&model, &request, &output)?;
    if request.stream {
        Ok(ChatResponse::Sse(chat_stream_chunks(
            &id,
            created,
            &model.id,
            &output,
            &receipt,
            request
                .stream_options
                .as_ref()
                .is_some_and(|options| options.include_usage),
        )))
    } else {
        Ok(ChatResponse::Json(chat_response_value(
            &id, created, &model, &output, &receipt,
        )))
    }
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
    fn meter_chat_session(
        &self,
        model: &GatewayModel,
        request: &ChatCompletionRequest,
        output: &ChatOutput,
    ) -> Result<StoredReceipt, ApiError> {
        let prompt_text = request
            .messages
            .iter()
            .map(message_to_text)
            .collect::<Vec<_>>()
            .join("\n");
        let usage = ReceiptUsage {
            in_tokens: output.usage.prompt_tokens,
            out_tokens: output.usage.completion_tokens,
        };
        self.meter_session(model, &prompt_text, usage)
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
            finish_reason: "stop",
            usage: usage_for(&prompt_text, &tool_result),
        }
    } else if let Some(name) = requested_tool_name(request) {
        let tool_call = ToolCallOutput {
            id: make_id("call"),
            name,
            arguments: "{}".to_owned(),
        };
        ChatOutput {
            content: None,
            tool_call: Some(tool_call),
            finish_reason: "tool_calls",
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
            finish_reason: "stop",
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
            "backend": "local-openai-shape",
            "direct_session": false,
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
        Some(output.finish_reason),
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
            "mayhem": { "receipt": receipt_summary(receipt) },
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
