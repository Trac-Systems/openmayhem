use axum::{
    body::{to_bytes, Body},
    http::{HeaderMap, Method, Request, StatusCode},
    Router,
};
use mayhem_gateway::openai::{
    openai_router, ChatCompletionRequest, ChatOutput, GatewayModel, GatewayRouteCandidate,
    GatewaySessionBackend, GatewaySessionError, GatewaySessionFuture, GatewaySessionInvocation,
    GatewaySessionResult, GatewayState, MayhemModelInfo, ModelCaps, PriceRefMu, Usage,
};
use mayhem_proto::{catalog_enclave_id, CatalogEnclaveIdentity};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tower::ServiceExt;

#[derive(Debug)]
struct TestDirectSessionBackend;

impl GatewaySessionBackend for TestDirectSessionBackend {
    fn name(&self) -> &str {
        "test-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            let prompt_tokens = request.messages.len() as u64;
            let completion_tokens = 4;
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: Some(format!(
                        "direct session response from {} via {}",
                        model.id, invocation.session_id
                    )),
                    tool_call: None,
                    finish_reason: "stop".to_owned(),
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens: prompt_tokens + completion_tokens,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
            })
        })
    }
}

#[derive(Debug)]
struct RetryThenDirectSessionBackend {
    retry_provider: String,
    calls: Arc<Mutex<Vec<String>>>,
}

impl GatewaySessionBackend for RetryThenDirectSessionBackend {
    fn name(&self) -> &str {
        "test-retry-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            let provider = invocation
                .provider_pubkey
                .clone()
                .unwrap_or_else(|| "<none>".to_owned());
            self.calls
                .lock()
                .expect("calls lock")
                .push(provider.clone());
            if provider == self.retry_provider {
                return Err(GatewaySessionError::retryable(
                    "simulated direct open timeout before spend",
                ));
            }
            let prompt_tokens = request.messages.len() as u64;
            let completion_tokens = 3;
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: Some(format!(
                        "direct retry response from {} via {}",
                        model.id, provider
                    )),
                    tool_call: None,
                    finish_reason: "stop".to_owned(),
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens: prompt_tokens + completion_tokens,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
            })
        })
    }
}

fn test_app() -> Router {
    openai_router(GatewayState::from_embedded_catalog())
}

fn test_state_and_app() -> (GatewayState, Router) {
    let state = GatewayState::from_embedded_catalog();
    let app = openai_router(state.clone());
    (state, app)
}

async fn json_request(app: Router, method: Method, uri: &str, body: Value) -> (StatusCode, Value) {
    let (status, headers, bytes) = raw_request(app, method, uri, Some(body)).await;
    assert!(headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .contains("application/json"));
    let json = serde_json::from_slice(&bytes).expect("response body is JSON");
    (status, json)
}

async fn raw_request(
    app: Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = if let Some(body) = body {
        builder = builder.header("content-type", "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    let response = app
        .oneshot(builder.body(body).expect("request builds"))
        .await
        .expect("router response");
    let (parts, body) = response.into_parts();
    let bytes = to_bytes(body, usize::MAX)
        .await
        .expect("response body bytes")
        .to_vec();
    (parts.status, parts.headers, bytes)
}

async fn first_model_id() -> String {
    let (status, body) = json_request(test_app(), Method::GET, "/v1/models", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "list");
    body["data"][0]["id"].as_str().expect("model id").to_owned()
}

fn is_hex(value: &str) -> bool {
    value.chars().all(|ch| ch.is_ascii_hexdigit())
}

#[tokio::test]
async fn models_endpoint_returns_openai_list_shape_with_mayhem_extension() {
    let (status, body) = json_request(test_app(), Method::GET, "/v1/models", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "list");
    assert!(body["data"].as_array().expect("model data").len() >= 2);
    assert_eq!(body["data"][0]["object"], "model");
    assert_eq!(body["data"][0]["owned_by"], "mayhem");
    assert_eq!(body["data"][0]["mayhem"]["price_ref_mu"]["denom"], "mu_usd");
    assert_eq!(body["data"][0]["mayhem"]["price_ref_mu"]["ver"], 1);
    assert_eq!(body["data"][0]["mayhem"]["caps"]["tools"], true);
}

#[tokio::test]
async fn models_endpoint_surfaces_tier2_attestation_counts_from_catalog() {
    let catalog = json!({
        "models": [{
            "model_id": "mayhem/tier2-model",
            "caps": { "tools": true, "json": true, "ctx_max": 4096, "vision": false },
            "price_ref_mu": { "denom": "mu_usd", "ver": 1, "in_per_1k": 10, "out_per_1k": 30 },
            "attestation_tiers": { "T1": 1, "T2": 2 }
        }]
    });
    let state = GatewayState::from_catalog_json(&catalog.to_string()).expect("catalog parses");
    let app = openai_router(state);

    let (status, body) = json_request(app, Method::GET, "/v1/models", Value::Null).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"][0]["id"], "mayhem/tier2-model");
    assert_eq!(body["data"][0]["mayhem"]["attestation_tiers"]["T1"], 1);
    assert_eq!(body["data"][0]["mayhem"]["attestation_tiers"]["T2"], 2);
}

#[tokio::test]
async fn chat_completion_returns_tool_call_and_accepts_tool_result_followup() {
    let model = first_model_id().await;
    let tool_request = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Use the weather tool." }],
        "tools": [{
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get weather",
                "parameters": { "type": "object", "properties": {} }
            }
        }]
    });
    let (status, body) = json_request(
        test_app(),
        Method::POST,
        "/v1/chat/completions",
        tool_request,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(
        body["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
        "get_weather"
    );
    assert_eq!(
        body["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        "{}"
    );

    let tool_call_id = body["choices"][0]["message"]["tool_calls"][0]["id"]
        .as_str()
        .expect("tool call id");
    let followup = json!({
        "model": first_model_id().await,
        "messages": [
            { "role": "user", "content": "Use the weather tool." },
            { "role": "assistant", "content": null, "tool_calls": body["choices"][0]["message"]["tool_calls"] },
            { "role": "tool", "tool_call_id": tool_call_id, "content": "{\"temperature_c\":21}" }
        ]
    });
    let (status, body) =
        json_request(test_app(), Method::POST, "/v1/chat/completions", followup).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .expect("assistant content")
        .contains("temperature_c"));
}

#[tokio::test]
async fn chat_completion_can_use_direct_session_backend() {
    let state = GatewayState::from_models(vec![routed_test_model()])
        .with_session_backend(Arc::new(TestDirectSessionBackend));
    let app = openai_router(state.clone());
    let model = "mayhem/routed-test".to_owned();
    let request = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Use a direct session." }]
    });

    let (status, body) =
        json_request(app.clone(), Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mayhem"]["backend"], "test-direct-session");
    assert_eq!(body["mayhem"]["direct_session"], true);
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .expect("assistant content")
        .contains("direct session response"));
    assert_eq!(state.receipts().len(), 1);
    assert_eq!(state.receipts()[0].receipt.body.usage.out_tokens, 4);
    assert_eq!(state.receipts()[0].receipt.body.provider, "55".repeat(32));
    assert_eq!(
        state.receipts()[0].receipt.body.enclave_id,
        catalog_enclave_id(&routed_test_identity())
    );
    assert_eq!(state.receipts()[0].receipt.body.price_ver, 7);
    assert_eq!(
        state.receipts()[0].voucher.body.session_id,
        state.receipts()[0].receipt.body.session_id
    );

    let (status, body) = json_request(app, Method::GET, "/mayhem/status", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["backend"], "test-direct-session");
}

#[tokio::test]
async fn chat_completion_retries_retryable_direct_session_route_before_metering() {
    let first_provider = "55".repeat(32);
    let second_provider = "66".repeat(32);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = GatewayState::from_models(vec![routed_test_model_with_providers(&[
        first_provider.clone(),
        second_provider.clone(),
    ])])
    .with_session_backend(Arc::new(RetryThenDirectSessionBackend {
        retry_provider: first_provider.clone(),
        calls: calls.clone(),
    }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Retry a direct session." }]
    });

    let (status, body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mayhem"]["backend"], "test-retry-direct-session");
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .expect("assistant content")
        .contains(&second_provider));
    assert_eq!(
        calls.lock().expect("calls lock").clone(),
        vec![first_provider, second_provider.clone()]
    );
    assert_eq!(state.receipts().len(), 1);
    assert_eq!(state.receipts()[0].receipt.body.provider, second_provider);
}

fn routed_test_model() -> GatewayModel {
    routed_test_model_with_providers(&["55".repeat(32)])
}

fn routed_test_model_with_providers(providers: &[String]) -> GatewayModel {
    let mut tiers = BTreeMap::new();
    tiers.insert("T1".to_owned(), 1);
    GatewayModel {
        id: "mayhem/routed-test".to_owned(),
        created: 1_782_950_400,
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
            attestation_tiers: tiers,
            caps: ModelCaps {
                tools: true,
                json: true,
                ctx: 8192,
                vision: false,
            },
            source: "contract".to_owned(),
            route_candidates: providers
                .iter()
                .enumerate()
                .map(|(idx, provider)| routed_test_candidate(provider, idx))
                .collect(),
        },
    }
}

fn routed_test_candidate(provider: &str, idx: usize) -> GatewayRouteCandidate {
    let identity = routed_test_identity();
    GatewayRouteCandidate {
        provider: provider.to_owned(),
        enclave_id: catalog_enclave_id(&identity),
        room_id: format!("room-{idx}"),
        price_ver: 7,
        att_tier: 1,
        admin_pubkey: identity.admin_pubkey,
        artifact_root: identity.artifact_root,
        manifest_hash: identity.manifest_hash,
        binary_hash: identity.binary_hash,
    }
}

fn routed_test_identity() -> CatalogEnclaveIdentity {
    CatalogEnclaveIdentity {
        admin_pubkey: "44".repeat(32),
        model_id: "mayhem/routed-test".to_owned(),
        artifact_root: "aa".repeat(32),
        manifest_hash: "bb".repeat(32),
        binary_hash: "cc".repeat(32),
    }
}

#[tokio::test]
async fn chat_completion_streams_openai_sse_chunks_with_usage() {
    let model = first_model_id().await;
    let request = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Stream a short answer." }],
        "stream": true,
        "stream_options": { "include_usage": true }
    });
    let (status, headers, bytes) = raw_request(
        test_app(),
        Method::POST,
        "/v1/chat/completions",
        Some(request),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .starts_with("text/event-stream"));
    let body = String::from_utf8(bytes).expect("SSE body is utf8");
    assert!(body.contains("data: {"));
    assert!(body.contains("\"object\":\"chat.completion.chunk\""));
    assert!(body.contains("\"choices\":[]"));
    assert!(body.contains("\"mayhem\":{"));
    assert!(body.contains("\"backend\":\"local-openai-shape\""));
    assert!(body.contains("\"direct_session\":false"));
    assert!(body.contains("\"receipt\":{"));
    assert!(body.contains("data: [DONE]"));
}

#[tokio::test]
async fn streaming_chat_persists_dual_signed_receipt() {
    let (state, app) = test_state_and_app();
    let model = first_model_id().await;
    let request = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Stream with receipt accounting." }],
        "stream": true,
        "stream_options": { "include_usage": true }
    });

    let (status, headers, bytes) = raw_request(
        app.clone(),
        Method::POST,
        "/v1/chat/completions",
        Some(request),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .starts_with("text/event-stream"));
    let body = String::from_utf8(bytes).expect("SSE body is utf8");
    assert!(body.contains("\"mayhem\":{"));
    assert!(body.contains("\"receipt\":{"));

    let receipts = state.receipts();
    assert_eq!(receipts.len(), 1);
    let stored = &receipts[0];
    assert!(stored.receipt.body.final_receipt);
    assert_eq!(stored.receipt.body.price_ver, stored.voucher.body.price_ver);
    assert_eq!(stored.receipt.body.price_ver, 1);
    assert_eq!(
        stored.receipt.body.session_id,
        stored.voucher.body.session_id
    );
    assert_eq!(
        stored.receipt_ack.session_id,
        stored.receipt.body.session_id
    );
    assert_eq!(stored.receipt_ack.seq, stored.receipt.body.seq);
    assert_eq!(stored.receipt_ack.user_sig, stored.receipt.user_sig);
    assert!(stored.receipt.body.mu_owed_cum > 0);
    assert!(stored.receipt.body.usage.out_tokens > 0);
    assert_eq!(stored.receipt.body.prompt_hash.len(), 64);
    assert!(is_hex(&stored.receipt.body.prompt_hash));
    assert_eq!(stored.voucher.user_sig.len(), 128);
    assert!(is_hex(&stored.voucher.user_sig));
    assert_eq!(stored.receipt.enclave_sig.len(), 128);
    assert!(is_hex(&stored.receipt.enclave_sig));
    assert_eq!(stored.receipt.user_sig.len(), 128);
    assert!(is_hex(&stored.receipt.user_sig));

    let (status, body) = json_request(app, Method::GET, "/mayhem/receipts", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().expect("receipt list").len(), 1);
    assert_eq!(body["paused"].as_array().expect("paused list").len(), 0);
}

#[tokio::test]
async fn refused_receipt_cosign_pauses_session_without_storing_receipt() {
    let state = GatewayState::from_embedded_catalog().with_receipt_cosign_enabled(false);
    let app = openai_router(state.clone());
    let model = first_model_id().await;
    let request = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "This should pause." }],
        "stream": true,
        "stream_options": { "include_usage": true }
    });

    let (status, body) =
        json_request(app.clone(), Method::POST, "/v1/chat/completions", request).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("session paused"));
    assert!(state.receipts().is_empty());
    let paused = state.paused_sessions();
    assert_eq!(paused.len(), 1);
    assert!(paused[0].reason.contains("co-signing refused"));

    let (status, body) =
        json_request(app.clone(), Method::GET, "/mayhem/status", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sessions_paused"], 1);
    assert_eq!(body["receipts"], 0);

    let (status, body) = json_request(app, Method::GET, "/mayhem/receipts", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().expect("receipt list").len(), 0);
    assert_eq!(body["paused"].as_array().expect("paused list").len(), 1);
}

#[tokio::test]
async fn response_format_json_object_returns_parseable_json_content() {
    let model = first_model_id().await;
    let request = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Return JSON." }],
        "response_format": { "type": "json_object" }
    });
    let (status, body) =
        json_request(test_app(), Method::POST, "/v1/chat/completions", request).await;
    assert_eq!(status, StatusCode::OK);
    let content = body["choices"][0]["message"]["content"]
        .as_str()
        .expect("json content");
    let parsed: Value = serde_json::from_str(content).expect("assistant content is JSON");
    assert_eq!(parsed["ok"], true);
}

#[tokio::test]
async fn legacy_completions_return_text_completion_shape_and_stream() {
    let (state, app) = test_state_and_app();
    let model = first_model_id().await;
    let request = json!({ "model": model, "prompt": "Hello", "max_tokens": 8 });
    let (status, body) = json_request(app.clone(), Method::POST, "/v1/completions", request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "text_completion");
    assert!(body["choices"][0]["text"]
        .as_str()
        .expect("completion text")
        .contains("Mayhem completion"));
    assert_eq!(body["mayhem"]["receipt"]["final"], true);

    let request = json!({ "model": first_model_id().await, "prompt": "Hello", "stream": true });
    let (status, headers, bytes) =
        raw_request(app, Method::POST, "/v1/completions", Some(request)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .starts_with("text/event-stream"));
    let body = String::from_utf8(bytes).expect("SSE body is utf8");
    assert!(body.contains("\"object\":\"text_completion\""));
    assert!(body.contains("\"mayhem\":{\"receipt\""));
    assert!(body.contains("data: [DONE]"));
    assert_eq!(state.receipts().len(), 2);
}

#[tokio::test]
async fn mayhem_local_endpoints_report_status_receipts_and_balance() {
    let (status, body) = json_request(test_app(), Method::GET, "/mayhem/status", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert_eq!(body["backend"], "local-openai-shape");

    let (status, body) =
        json_request(test_app(), Method::GET, "/mayhem/receipts", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "list");

    let (status, body) =
        json_request(test_app(), Method::GET, "/mayhem/balance", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["denom"], "mu_usd");
}
