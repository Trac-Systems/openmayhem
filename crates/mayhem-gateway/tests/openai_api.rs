use axum::{
    body::{to_bytes, Body},
    http::{HeaderMap, Method, Request, StatusCode},
    Router,
};
use mayhem_gateway::openai::{
    openai_router, validate_loopback_dashboard_bind, ChatCompletionRequest, ChatMessage,
    ChatOutput, GatewayArtifactOutput, GatewayCanaryModelConfig, GatewayCanaryProbePolicy,
    GatewayCanaryPrompt, GatewayCanaryRegistry, GatewayModel, GatewayRouteCandidate,
    GatewaySessionBackend, GatewaySessionError, GatewaySessionFuture, GatewaySessionInvocation,
    GatewaySessionResult, GatewayState, MayhemModelInfo, ModelCaps, PriceRefMu, ShapeAdapterInfo,
    ToolCallOutput, Usage,
};
use mayhem_gateway::{
    aggregate_canary_fingerprints, text_generation_rate_map, token_fingerprint, ReputationEventKind,
};
use mayhem_proto::{catalog_enclave_id, CatalogEnclaveIdentity, DEFAULT_MODEL_CLASS};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
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
                    artifacts: Vec::new(),
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
                token_ids: vec![1, 2, 3, 4],
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct ToolCallDirectSessionBackend;

impl GatewaySessionBackend for ToolCallDirectSessionBackend {
    fn name(&self) -> &str {
        "test-tool-call-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            let prompt_tokens = request.messages.len() as u64;
            let completion_tokens = 1;
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: None,
                    tool_call: Some(ToolCallOutput {
                        id: "call-normalized".to_owned(),
                        name: "write".to_owned(),
                        arguments: r#"{"filePath":"ok.txt"}"#.to_owned(),
                    }),
                    artifacts: Vec::new(),
                    finish_reason: "tool_calls".to_owned(),
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens: prompt_tokens + completion_tokens,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                token_ids: vec![1],
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct ArtifactDirectSessionBackend;

impl GatewaySessionBackend for ArtifactDirectSessionBackend {
    fn name(&self) -> &str {
        "test-artifact-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            let prompt_tokens = request.messages.len() as u64;
            let image = b"\x89PNG mayhem artifact".to_vec();
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: Some(String::new()),
                    tool_call: None,
                    artifacts: vec![GatewayArtifactOutput {
                        id: "image-1".to_owned(),
                        content_type: "image/png".to_owned(),
                        blake3: blake3::hash(&image).to_hex().to_string(),
                        bytes: image,
                    }],
                    finish_reason: "stop".to_owned(),
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens: 0,
                        total_tokens: prompt_tokens,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                token_ids: Vec::new(),
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct VisionInspectBackend {
    seen_content: Arc<Mutex<Vec<Value>>>,
}

impl GatewaySessionBackend for VisionInspectBackend {
    fn name(&self) -> &str {
        "test-vision-inspect"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        _invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            self.seen_content
                .lock()
                .expect("seen content lock")
                .push(request.messages[0].content.clone());
            let prompt_tokens = request.messages.len() as u64;
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: Some("vision ok".to_owned()),
                    tool_call: None,
                    artifacts: Vec::new(),
                    finish_reason: "stop".to_owned(),
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens: 2,
                        total_tokens: prompt_tokens + 2,
                    },
                },
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                token_ids: vec![70, 71],
                quality: None,
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
                    artifacts: Vec::new(),
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
                token_ids: vec![2, 3, 4],
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct RetryFirstDirectSessionBackend {
    calls: Arc<Mutex<Vec<String>>>,
}

impl GatewaySessionBackend for RetryFirstDirectSessionBackend {
    fn name(&self) -> &str {
        "test-retry-first-direct-session"
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
            let attempt = {
                let mut calls = self.calls.lock().expect("calls lock");
                calls.push(provider.clone());
                calls.len()
            };
            if attempt == 1 {
                return Err(GatewaySessionError::retryable(
                    "simulated first direct open timeout before spend",
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
                    artifacts: Vec::new(),
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
                token_ids: vec![2, 3, 4],
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct AlwaysRetryDirectSessionBackend {
    calls: Arc<Mutex<Vec<String>>>,
}

impl GatewaySessionBackend for AlwaysRetryDirectSessionBackend {
    fn name(&self) -> &str {
        "test-always-retry-direct-session"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            let provider = invocation
                .provider_pubkey
                .clone()
                .unwrap_or_else(|| "<none>".to_owned());
            self.calls.lock().expect("calls lock").push(provider);
            Err(GatewaySessionError::retryable(
                "simulated direct open timeout before spend",
            ))
        })
    }
}

#[derive(Debug)]
struct HedgeInspectBackend {
    invocations: Arc<Mutex<Vec<HedgeInvocationRecord>>>,
    probes: Arc<Mutex<Vec<String>>>,
    probe_delays_ms: BTreeMap<String, u64>,
}

type HedgeInvocationRecord = (String, bool, usize, usize, Option<String>);

impl GatewaySessionBackend for HedgeInspectBackend {
    fn name(&self) -> &str {
        "test-hedge-inspect"
    }

    fn hedge_probe<'a>(
        &'a self,
        _model: &'a GatewayModel,
        _request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> mayhem_gateway::openai::GatewayHedgeProbeFuture<'a> {
        Box::pin(async move {
            let provider = invocation
                .provider_pubkey
                .clone()
                .unwrap_or_else(|| "<none>".to_owned());
            self.probes
                .lock()
                .expect("probes lock")
                .push(provider.clone());
            let delay = self.probe_delays_ms.get(&provider).copied().unwrap_or(1);
            tokio::time::sleep(Duration::from_millis(delay)).await;
            Ok(mayhem_gateway::openai::GatewayHedgeProbeResult {
                provider,
                ttft_ms: delay.max(1),
            })
        })
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
            self.invocations.lock().expect("invocations lock").push((
                provider.clone(),
                invocation.hedge.requested,
                invocation.hedge.planned_probe_count,
                invocation.hedge.actual_probe_count,
                invocation.hedge.winner_provider.clone(),
            ));
            let prompt_tokens = request.messages.len() as u64;
            let completion_tokens = 2;
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: Some(format!("hedge inspected for {} via {}", model.id, provider)),
                    tool_call: None,
                    artifacts: Vec::new(),
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
                token_ids: vec![5, 6],
                quality: None,
            })
        })
    }
}

#[derive(Debug)]
struct CanarySubstitutionBackend {
    calls: Arc<Mutex<Vec<String>>>,
}

impl GatewaySessionBackend for CanarySubstitutionBackend {
    fn name(&self) -> &str {
        "test-canary-substitution"
    }

    fn run_chat<'a>(
        &'a self,
        model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            let prompt = request
                .messages
                .iter()
                .map(|message| message.content.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            self.calls.lock().expect("calls lock").push(prompt.clone());
            let is_canary = prompt.contains("fixed canary");
            let token_ids = if is_canary {
                vec![9, 9, 9]
            } else {
                vec![1, 2, 3]
            };
            let content = if is_canary {
                "substituted canary output".to_owned()
            } else {
                format!(
                    "normal direct session response from {} via {}",
                    model.id, invocation.session_id
                )
            };
            let prompt_tokens = request.messages.len() as u64;
            let completion_tokens = token_ids.len() as u64;
            Ok(GatewaySessionResult {
                output: ChatOutput {
                    content: Some(content),
                    tool_call: None,
                    artifacts: Vec::new(),
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
                token_ids,
                quality: None,
            })
        })
    }
}

fn test_app() -> Router {
    openai_router(GatewayState::from_embedded_catalog().with_dev_session_shim())
}

fn test_state_and_app() -> (GatewayState, Router) {
    let state = GatewayState::from_embedded_catalog().with_dev_session_shim();
    let app = openai_router(state.clone());
    (state, app)
}

async fn json_request(app: Router, method: Method, uri: &str, body: Value) -> (StatusCode, Value) {
    json_request_with_headers(app, method, uri, body, &[]).await
}

async fn json_request_with_headers(
    app: Router,
    method: Method,
    uri: &str,
    body: Value,
    request_headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let (status, headers, bytes) =
        raw_request_with_headers(app, method, uri, Some(body), request_headers).await;
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
    raw_request_with_headers(app, method, uri, body, &[]).await
}

async fn raw_request_with_headers(
    app: Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    request_headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in request_headers {
        builder = builder.header(*name, *value);
    }
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

async fn raw_bytes_request_with_headers(
    app: Router,
    method: Method,
    uri: &str,
    body: Vec<u8>,
    request_headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in request_headers {
        builder = builder.header(*name, *value);
    }
    let response = app
        .oneshot(builder.body(Body::from(body)).expect("request builds"))
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

#[tokio::test]
async fn production_gateway_without_live_provider_refuses_local_chat_shim() {
    let state = GatewayState::from_embedded_catalog();
    let app = openai_router(state.clone());
    let (status, models) = json_request(app.clone(), Method::GET, "/v1/models", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let model = models["data"][0]["id"].as_str().expect("model id");
    let request = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Do not fabricate a local answer." }]
    });

    let (status, body) =
        json_request(app.clone(), Method::POST, "/v1/chat/completions", request).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("no provider available"));
    assert!(state.receipts().is_empty());

    let request = json!({ "model": model, "prompt": "No deterministic completions either." });
    let (status, body) = json_request(app.clone(), Method::POST, "/v1/completions", request).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("local dev shim"));
    assert!(state.receipts().is_empty());

    let (status, body) = json_request(app, Method::GET, "/mayhem/status", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["backend"], "no-live-provider");
    assert_eq!(body["dev_session_shim"], false);
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
    assert_eq!(
        body["data"][0]["mayhem"]["price_ref_mu"]["rate_map"][0]["unit"],
        "input_token"
    );
    assert_eq!(
        body["data"][0]["mayhem"]["price_ref_mu"]["rate_map"][1]["unit"],
        "output_token"
    );
    assert_eq!(body["data"][0]["mayhem"]["caps"]["tools"], true);
    assert_eq!(
        body["data"][0]["mayhem"]["adapter"]["tool_call_strategy"],
        "mayhem_json"
    );
}

#[tokio::test]
async fn models_endpoint_surfaces_tier2_attestation_counts_from_catalog() {
    let catalog = json!({
        "models": [{
            "model_id": "mayhem/tier2-model",
            "model_class": "embedding",
            "caps": { "tools": true, "json": true, "ctx_max": 4096, "vision": false },
            "price_ref_mu": {
                "denom": "mu_usd",
                "ver": 1,
                "rate_map": [
                    { "unit": "input_token", "per_unit_mu": 10, "granularity": 1000 },
                    { "unit": "output_token", "per_unit_mu": 30, "granularity": 1000 }
                ]
            },
            "attestation_tiers": { "T1": 1, "T2": 2 }
        }]
    });
    let state = GatewayState::from_catalog_json(&catalog.to_string()).expect("catalog parses");
    let app = openai_router(state);

    let (status, body) = json_request(app, Method::GET, "/v1/models", Value::Null).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"][0]["id"], "mayhem/tier2-model");
    assert_eq!(body["data"][0]["mayhem"]["model_class"], "embedding");
    assert_eq!(body["data"][0]["mayhem"]["attestation_tiers"]["T1"], 1);
    assert_eq!(body["data"][0]["mayhem"]["attestation_tiers"]["T2"], 2);
    assert!(body["data"][0]["mayhem"]["attestation_tier_labels"]["T2"]
        .as_str()
        .expect("tier 2 label")
        .contains("Apple App Attest strong / NVIDIA GB10 device medium"));
}

#[tokio::test]
async fn embeddings_endpoint_requires_real_engine_and_records_zero_charge() {
    let catalog = json!({
        "models": [{
            "model_id": "admin/embed-fixture",
            "model_class": "embedding",
            "caps": {
                "tools": false,
                "json": false,
                "ctx_max": 8192,
                "vision": false,
                "output_modality": "embedding",
                "output_modalities": ["embedding"]
            },
            "adapter": {
                "request_shape_family": "openai_chat",
                "chat_template_id": "generic_chatml",
                "tool_call_strategy": "none",
                "reasoning_passthrough": "strip",
                "modality_set": ["embedding"],
                "response_normalization": "openai_chat"
            },
            "price_ref_mu": {
                "denom": "mu_usd",
                "ver": 3,
                "rate_map": [
                    { "unit": "input_token", "per_unit_mu": 10, "granularity": 1000 }
                ]
            },
            "attestation_tiers": { "T1": 1 }
        }]
    });
    let state = GatewayState::from_catalog_json(&catalog.to_string()).expect("catalog parses");
    let app = openai_router(state.clone());
    let request = json!({
        "model": "admin/embed-fixture",
        "input": ["alpha", "beta"],
        "dimensions": 8,
        "encoding_format": "float"
    });

    let (status, body) = json_request(app, Method::POST, "/v1/embeddings", request).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["param"], "model");
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("embeddings is not served"));
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("no charge recorded"));
    assert!(state.receipts().is_empty());
}

#[tokio::test]
async fn embeddings_endpoint_rejects_non_embedding_model() {
    let model = first_model_id().await;
    let (status, body) = json_request(
        test_app(),
        Method::POST,
        "/v1/embeddings",
        json!({ "model": model, "input": "hello" }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["param"], "model");
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("does not support embeddings"));
}

#[tokio::test]
async fn image_generation_endpoint_requires_real_engine_and_records_zero_charge() {
    let catalog = json!({
        "models": [{
            "model_id": "admin/image-fixture",
            "model_class": "image-generation",
            "caps": {
                "tools": false,
                "json": false,
                "ctx_max": 4096,
                "vision": false,
                "image": true,
                "output_modality": "image",
                "output_modalities": ["image"]
            },
            "adapter": {
                "request_shape_family": "openai_chat",
                "chat_template_id": "generic_chatml",
                "tool_call_strategy": "none",
                "reasoning_passthrough": "strip",
                "modality_set": ["image"],
                "response_normalization": "openai_chat"
            },
            "price_ref_mu": {
                "denom": "mu_usd",
                "ver": 4,
                "rate_map": [
                    { "unit": "image", "per_unit_mu": 500, "granularity": 1 },
                    { "unit": "step", "per_unit_mu": 2, "granularity": 1 }
                ]
            },
            "attestation_tiers": { "T1": 1 }
        }]
    });
    let state = GatewayState::from_catalog_json(&catalog.to_string()).expect("catalog parses");
    let app = openai_router(state.clone());
    let request = json!({
        "model": "admin/image-fixture",
        "prompt": "a red cube",
        "n": 2,
        "size": "512x512",
        "steps": 30,
        "response_format": "b64_json"
    });

    let (status, body) = json_request(app, Method::POST, "/v1/images/generations", request).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["param"], "model");
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("image generation is not served"));
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("no charge recorded"));
    assert!(state.receipts().is_empty());
}

#[tokio::test]
async fn audio_speech_endpoint_requires_real_engine_and_records_zero_charge() {
    let catalog = json!({
        "models": [{
            "model_id": "admin/tts-fixture",
            "model_class": "tts",
            "caps": {
                "tools": false,
                "json": false,
                "ctx_max": 4096,
                "vision": false,
                "audio": true,
                "output_modality": "audio",
                "output_modalities": ["audio"]
            },
            "adapter": {
                "request_shape_family": "openai_chat",
                "chat_template_id": "generic_chatml",
                "tool_call_strategy": "none",
                "reasoning_passthrough": "strip",
                "modality_set": ["audio"],
                "response_normalization": "openai_chat"
            },
            "price_ref_mu": {
                "denom": "mu_usd",
                "ver": 5,
                "rate_map": [
                    { "unit": "input_character", "per_unit_mu": 1, "granularity": 1 },
                    { "unit": "audio_second", "per_unit_mu": 100, "granularity": 1 }
                ]
            },
            "attestation_tiers": { "T1": 1 }
        }]
    });
    let state = GatewayState::from_catalog_json(&catalog.to_string()).expect("catalog parses");
    let app = openai_router(state.clone());
    let input = "hello speech";
    let request = json!({
        "model": "admin/tts-fixture",
        "input": input,
        "voice": "alloy",
        "response_format": "wav"
    });

    let (status, headers, bytes) =
        raw_request(app, Method::POST, "/v1/audio/speech", Some(request)).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!headers.contains_key("x-mayhem-receipt-session-id"));
    let body: Value = serde_json::from_slice(&bytes).expect("speech error JSON");
    assert_eq!(body["error"]["param"], "model");
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("audio speech is not served"));
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("no charge recorded"));
    assert!(state.receipts().is_empty());
}

#[tokio::test]
async fn audio_transcription_endpoint_requires_real_engine_and_records_zero_charge() {
    let catalog = json!({
        "models": [{
            "model_id": "admin/stt-fixture",
            "model_class": "stt",
            "caps": {
                "tools": false,
                "json": false,
                "ctx_max": 4096,
                "vision": false,
                "audio": true,
                "output_modality": "text",
                "output_modalities": ["text"]
            },
            "adapter": {
                "request_shape_family": "openai_chat",
                "chat_template_id": "generic_chatml",
                "tool_call_strategy": "none",
                "reasoning_passthrough": "strip",
                "modality_set": ["audio", "text"],
                "response_normalization": "openai_chat"
            },
            "price_ref_mu": {
                "denom": "mu_usd",
                "ver": 6,
                "rate_map": [
                    { "unit": "audio_second", "per_unit_mu": 250, "granularity": 1 }
                ]
            },
            "attestation_tiers": { "T1": 1 }
        }]
    });
    let state = GatewayState::from_catalog_json(&catalog.to_string()).expect("catalog parses");
    let app = openai_router(state.clone());
    let boundary = "mayhem-test-boundary";
    let audio = vec![7_u8; 16_000];
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nadmin/stt-fixture\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\nverbose_json\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"clip.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let (status, headers, bytes) = raw_bytes_request_with_headers(
        app,
        Method::POST,
        "/v1/audio/transcriptions",
        body,
        &[(
            "content-type",
            &format!("multipart/form-data; boundary={boundary}"),
        )],
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .contains("application/json"));
    let body: Value = serde_json::from_slice(&bytes).expect("transcription error JSON");
    assert_eq!(body["error"]["param"], "model");
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("audio transcription is not served"));
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("no charge recorded"));
    assert!(state.receipts().is_empty());
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
    assert_eq!(state.receipts()[0].receipt.body.usage.output_tokens(), 4);
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
async fn chat_completion_rejects_image_content_for_non_vision_model() {
    let state = GatewayState::from_models(vec![routed_test_model()])
        .with_session_backend(Arc::new(TestDirectSessionBackend));
    let app = openai_router(state);
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": "describe this" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,aW1hZ2U=" } }
            ]
        }]
    });

    let (status, body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["param"], "messages");
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("does not support image_url"));
}

#[tokio::test]
async fn chat_completion_preserves_image_content_for_vision_direct_session() {
    let mut model = routed_test_model();
    model.mayhem.caps.vision = true;
    let seen_content = Arc::new(Mutex::new(Vec::new()));
    let state = GatewayState::from_models(vec![model]).with_session_backend(Arc::new(
        VisionInspectBackend {
            seen_content: seen_content.clone(),
        },
    ));
    let app = openai_router(state);
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": "describe this" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,aW1hZ2U=" } }
            ]
        }]
    });

    let (status, body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["choices"][0]["message"]["content"], "vision ok");
    assert_eq!(
        seen_content.lock().expect("seen content")[0][1]["image_url"]["url"],
        "data:image/png;base64,aW1hZ2U="
    );
}

#[tokio::test]
async fn chat_completion_exposes_direct_session_artifact_summary() {
    let state = GatewayState::from_models(vec![routed_test_model()])
        .with_session_backend(Arc::new(ArtifactDirectSessionBackend));
    let app = openai_router(state);
    let image = b"\x89PNG mayhem artifact".to_vec();
    let expected_hash = blake3::hash(&image).to_hex().to_string();
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Generate an image." }]
    });

    let (status, body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mayhem"]["backend"], "test-artifact-direct-session");
    assert_eq!(body["mayhem"]["artifacts"][0]["id"], "image-1");
    assert_eq!(body["mayhem"]["artifacts"][0]["content_type"], "image/png");
    assert_eq!(body["mayhem"]["artifacts"][0]["bytes"], image.len());
    assert_eq!(body["mayhem"]["artifacts"][0]["blake3"], expected_hash);
}

#[tokio::test]
async fn automatic_canary_probe_catches_substituted_served_enclave() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = GatewayState::from_models(vec![routed_test_model()])
        .with_canary_registry(test_canary_registry(&[1, 2, 3]))
        .with_canary_probe_policy(GatewayCanaryProbePolicy::every_session_for_tests())
        .with_session_backend(Arc::new(CanarySubstitutionBackend {
            calls: calls.clone(),
        }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Use a direct session." }]
    });

    let (status, body) =
        json_request(app.clone(), Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mayhem"]["backend"], "test-canary-substitution");
    assert_eq!(state.receipts().len(), 2);
    assert_eq!(calls.lock().expect("calls lock").len(), 2);

    let probes = state.probes();
    assert_eq!(probes.len(), 1);
    let probe = &probes[0];
    assert!(!probe.pass);
    assert_eq!(probe.match_bps, 0);
    assert_eq!(probe.reputation_event_kind, ReputationEventKind::ProbeFail);
    assert_eq!(probe.probe_command["op"], "probe_result");
    assert_eq!(probe.probe_command["probe_kind"], "canary");
    assert_eq!(
        probe.probe_command["verification_method"],
        "token_fingerprint"
    );
    assert_eq!(probe.probe_command["pass"], false);
    assert_eq!(probe.verification_method, "token_fingerprint");
    assert_eq!(probe.probe_command["provider"], "55".repeat(32));
    assert_eq!(
        probe.probe_command["enclave_id"],
        catalog_enclave_id(&routed_test_identity())
    );
    assert!(
        probe.evidence["evidence"]["prompts"][0]["token_count"]
            .as_u64()
            .expect("token count")
            > 0
    );
    assert_eq!(
        probe.evidence["evidence"]["catalog_expected_token_prefixes"]["fixed-probe"],
        json!([1, 2, 3])
    );
    assert_eq!(
        probe.evidence["evidence"]["prompts"][0]["token_ids"],
        json!([9, 9, 9])
    );

    let (status, body) = json_request(app, Method::GET, "/mayhem/probes", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().expect("probe list").len(), 1);
    assert_eq!(body["data"][0]["pass"], false);
    assert_eq!(
        body["data"][0]["reputation_event_kind"]["kind"],
        "probe_fail"
    );
}

#[tokio::test]
async fn automatic_canary_probe_accepts_exact_catalog_token_prefix() {
    let state = GatewayState::from_models(vec![routed_test_model()])
        .with_canary_registry(test_canary_registry(&[9, 9, 9]))
        .with_canary_probe_policy(GatewayCanaryProbePolicy::every_session_for_tests())
        .with_session_backend(Arc::new(CanarySubstitutionBackend {
            calls: Arc::new(Mutex::new(Vec::new())),
        }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Use a direct session." }]
    });

    let (status, _body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::OK);
    let probes = state.probes();
    assert_eq!(probes.len(), 1);
    assert!(probes[0].pass);
    assert_eq!(probes[0].match_bps, 10_000);
    assert_eq!(
        probes[0].reputation_event_kind,
        ReputationEventKind::ProbeOk
    );
}

#[tokio::test]
async fn contract_model_with_noncanonical_route_is_unavailable() {
    let mut model = routed_test_model();
    model.mayhem.route_candidates[0].room_id = "provider-local-only".to_owned();
    let state = GatewayState::from_models(vec![model])
        .with_session_backend(Arc::new(TestDirectSessionBackend));
    let app = openai_router(state);
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "This should not route." }]
    });

    let (status, body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("not available"));
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
    .with_session_backend(Arc::new(RetryFirstDirectSessionBackend {
        calls: calls.clone(),
    }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Retry a direct session." }]
    });

    let (status, body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mayhem"]["backend"], "test-retry-first-direct-session");
    let calls = calls.lock().expect("calls lock").clone();
    assert_eq!(calls.len(), 2);
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .expect("assistant content")
        .contains(calls[1].as_str()));
    assert_ne!(calls[0], calls[1]);
    assert!(calls.iter().all(
        |provider| [first_provider.as_str(), second_provider.as_str()].contains(&provider.as_str())
    ));
    assert_eq!(state.receipts().len(), 1);
    assert_eq!(state.receipts()[0].receipt.body.provider, calls[1]);
}

#[tokio::test]
async fn chat_completion_caps_retryable_direct_session_routes_at_four_without_receipts() {
    let providers = (1..=5)
        .map(|idx| format!("{idx:02x}").repeat(32))
        .collect::<Vec<_>>();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = GatewayState::from_models(vec![routed_test_model_with_providers(&providers)])
        .with_session_backend(Arc::new(AlwaysRetryDirectSessionBackend {
            calls: calls.clone(),
        }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Every direct open times out." }]
    });

    let (status, body) = json_request(app, Method::POST, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("all 4 route attempt(s) failed before spend"));
    let calls = calls.lock().expect("calls lock").clone();
    assert_eq!(calls.len(), 4);
    let unique_calls = calls.iter().cloned().collect::<BTreeSet<_>>();
    assert_eq!(unique_calls.len(), 4);
    assert!(calls.iter().all(|provider| providers.contains(provider)));
    assert!(state.receipts().is_empty());
}

#[tokio::test]
async fn chat_completion_binds_x_mayhem_hedge_to_direct_session_invocation() {
    let first_provider = "55".repeat(32);
    let second_provider = "66".repeat(32);
    let invocations = Arc::new(Mutex::new(Vec::new()));
    let probes = Arc::new(Mutex::new(Vec::new()));
    let probe_delays_ms =
        BTreeMap::from([(first_provider.clone(), 25), (second_provider.clone(), 1)]);
    let state = GatewayState::from_models(vec![routed_test_model_with_providers(&[
        first_provider.clone(),
        second_provider.clone(),
    ])])
    .with_session_backend(Arc::new(HedgeInspectBackend {
        invocations: invocations.clone(),
        probes: probes.clone(),
        probe_delays_ms,
    }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Hedge this direct session." }]
    });

    let (status, body) = json_request_with_headers(
        app,
        Method::POST,
        "/v1/chat/completions",
        request,
        &[("X-Mayhem-Hedge", "1")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mayhem"]["backend"], "test-hedge-inspect");
    assert_eq!(body["mayhem"]["hedge"]["requested"], true);
    assert_eq!(body["mayhem"]["hedge"]["planned_probe_count"], 2);
    assert_eq!(body["mayhem"]["hedge"]["actual_probe_count"], 2);
    assert_eq!(body["mayhem"]["hedge"]["winner_provider"], second_provider);
    assert_eq!(body["mayhem"]["hedge"]["winner_ttft_ms"], 1);
    let probes = probes.lock().expect("probes lock").clone();
    assert_eq!(probes.iter().cloned().collect::<BTreeSet<_>>().len(), 2);
    assert!(probes.contains(&first_provider));
    assert!(probes.contains(&second_provider));
    let invocations = invocations.lock().expect("invocations lock").clone();
    assert_eq!(invocations.len(), 1);
    assert_eq!(
        invocations[0],
        (
            second_provider.clone(),
            true,
            2,
            2,
            Some(second_provider.clone())
        )
    );
    assert_eq!(state.receipts().len(), 1);
    assert_eq!(state.receipts()[0].receipt.body.provider, invocations[0].0);
}

#[tokio::test]
async fn invalid_x_mayhem_hedge_header_is_rejected_before_session_start() {
    let invocations = Arc::new(Mutex::new(Vec::new()));
    let state = GatewayState::from_models(vec![routed_test_model_with_providers(&[
        "55".repeat(32),
        "66".repeat(32),
    ])])
    .with_session_backend(Arc::new(HedgeInspectBackend {
        invocations: invocations.clone(),
        probes: Arc::new(Mutex::new(Vec::new())),
        probe_delays_ms: BTreeMap::new(),
    }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "This header should fail." }]
    });

    let (status, body) = json_request_with_headers(
        app,
        Method::POST,
        "/v1/chat/completions",
        request,
        &[("X-Mayhem-Hedge", "true")],
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["param"], "X-Mayhem-Hedge");
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("must be 1"));
    assert!(invocations.lock().expect("invocations lock").is_empty());
    assert!(state.receipts().is_empty());
}

#[tokio::test]
async fn invalid_x_mayhem_min_att_tier_header_is_rejected_before_session_start() {
    let invocations = Arc::new(Mutex::new(Vec::new()));
    let state = GatewayState::from_models(vec![routed_test_model_with_providers(&[
        "55".repeat(32),
        "66".repeat(32),
    ])])
    .with_session_backend(Arc::new(HedgeInspectBackend {
        invocations: invocations.clone(),
        probes: Arc::new(Mutex::new(Vec::new())),
        probe_delays_ms: BTreeMap::new(),
    }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "This tier header should fail." }]
    });

    let (status, body) = json_request_with_headers(
        app,
        Method::POST,
        "/v1/chat/completions",
        request,
        &[("X-Mayhem-Min-Att-Tier", "5")],
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["param"], "X-Mayhem-Min-Att-Tier");
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("between 1 and 4"));
    assert!(invocations.lock().expect("invocations lock").is_empty());
    assert!(state.receipts().is_empty());
}

#[tokio::test]
async fn chat_completion_min_att_tier_filters_route_candidates() {
    let first_provider = "55".repeat(32);
    let second_provider = "66".repeat(32);
    let mut model =
        routed_test_model_with_providers(&[first_provider.clone(), second_provider.clone()]);
    model.mayhem.route_candidates[0].att_tier = 1;
    model.mayhem.route_candidates[1].att_tier = 3;
    model.mayhem.attestation_tiers = BTreeMap::from([("T1".to_owned(), 1), ("T3".to_owned(), 1)]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = GatewayState::from_models(vec![model]).with_session_backend(Arc::new(
        RetryThenDirectSessionBackend {
            retry_provider: "ff".repeat(32),
            calls: calls.clone(),
        },
    ));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Use Tier 3 only." }]
    });

    let (status, body) = json_request_with_headers(
        app,
        Method::POST,
        "/v1/chat/completions",
        request,
        &[("X-Mayhem-Min-Att-Tier", "3")],
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        calls.lock().expect("calls lock").clone(),
        vec![second_provider.clone()]
    );
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .expect("assistant content")
        .contains(&second_provider));
    assert_eq!(state.receipts().len(), 1);
    assert_eq!(state.receipts()[0].receipt.body.provider, second_provider);
}

#[tokio::test]
async fn chat_completion_min_att_tier_rejects_when_no_route_meets_pin() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let state = GatewayState::from_models(vec![routed_test_model_with_providers(&[
        "55".repeat(32),
        "66".repeat(32),
    ])])
    .with_session_backend(Arc::new(RetryThenDirectSessionBackend {
        retry_provider: "ff".repeat(32),
        calls: calls.clone(),
    }));
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Need Tier 3." }]
    });

    let (status, body) = json_request_with_headers(
        app,
        Method::POST,
        "/v1/chat/completions",
        request,
        &[("X-Mayhem-Min-Att-Tier", "3")],
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["param"], "X-Mayhem-Min-Att-Tier");
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("no provider route"));
    assert!(calls.lock().expect("calls lock").is_empty());
    assert!(state.receipts().is_empty());
}

fn routed_test_model() -> GatewayModel {
    routed_test_model_with_providers(&["55".repeat(32)])
}

fn test_canary_registry(expected_tokens: &[i32]) -> GatewayCanaryRegistry {
    let prompt_fingerprint = token_fingerprint(expected_tokens.iter().copied()).digest;
    let expected_fingerprint =
        aggregate_canary_fingerprints([("fixed-probe", prompt_fingerprint.as_str())]);
    GatewayCanaryRegistry {
        models: BTreeMap::from([(
            "mayhem/routed-test".to_owned(),
            GatewayCanaryModelConfig {
                canary_set: "canary-test-v1".to_owned(),
                match_min_bps: 9_000,
                verification_method: "token_fingerprint".to_owned(),
                verification_tolerance_bps: None,
                prompts: vec![GatewayCanaryPrompt {
                    id: "fixed-probe".to_owned(),
                    messages: vec![ChatMessage {
                        role: "user".to_owned(),
                        content: json!("fixed canary prompt"),
                        name: None,
                        extra: BTreeMap::new(),
                    }],
                    tools: None,
                    max_tokens: 8,
                }],
                fingerprints_by_artifact_root: BTreeMap::from([(
                    "aa".repeat(32),
                    expected_fingerprint,
                )]),
                token_prefixes_by_artifact_root: BTreeMap::from([(
                    "aa".repeat(32),
                    BTreeMap::from([("fixed-probe".to_owned(), expected_tokens.to_vec())]),
                )]),
                perceptual_hashes_by_artifact_root: BTreeMap::new(),
                default_fingerprint: None,
                default_token_prefixes: None,
                default_perceptual_hashes: None,
            },
        )]),
    }
}

fn routed_test_model_with_providers(providers: &[String]) -> GatewayModel {
    let mut tiers = BTreeMap::new();
    tiers.insert("T1".to_owned(), 1);
    GatewayModel {
        id: "mayhem/routed-test".to_owned(),
        created: 1_782_950_400,
        owned_by: "mayhem".to_owned(),
        mayhem: MayhemModelInfo {
            model_class: DEFAULT_MODEL_CLASS.to_owned(),
            providers_online: 1,
            rooms: 1,
            price_ref_mu: PriceRefMu {
                denom: "mu_usd".to_owned(),
                ver: 7,
                rate_map: text_generation_rate_map(20, 60),
            },
            attestation_tiers: tiers,
            attestation_tier_labels: BTreeMap::from([(
                "T1".to_owned(),
                "Tier 1 - software self-attestation; economic/trust only".to_owned(),
            )]),
            min_app_version: None,
            caps: ModelCaps {
                tools: true,
                json: true,
                ctx: 8192,
                vision: false,
                image: false,
                video: false,
                audio: false,
                output_modality: Some("text".to_owned()),
                output_modalities: vec!["text".to_owned()],
            },
            adapter: ShapeAdapterInfo::default(),
            failover: mayhem_gateway::openai::GatewayFailoverPolicyConfig::default(),
            source: "contract".to_owned(),
            kyb_identities: Vec::new(),
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
    let room_id = format!("{:02x}", idx + 160).repeat(16);
    GatewayRouteCandidate {
        provider: provider.to_owned(),
        accepted_rails: vec!["fiat".to_owned(), "tap".to_owned(), "tnk".to_owned()],
        enclave_id: catalog_enclave_id(&identity),
        room_id,
        price_ver: 7,
        att_tier: 1,
        admin_pubkey: identity.admin_pubkey,
        artifact_root: identity.artifact_root,
        manifest_hash: identity.manifest_hash,
        binary_hash: identity.binary_hash,
        kyb: None,
        caps: serde_json::json!({}),
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
    assert!(body.contains("\"billable\":false"));
    assert!(body.contains("\"dev_session\":true"));
    assert!(body.contains("\"receipt\":null"));
    assert!(body.contains("data: [DONE]"));
}

#[tokio::test]
async fn chat_completion_streams_normalized_tool_call_delta() {
    let mut model = routed_test_model();
    model.mayhem.adapter = ShapeAdapterInfo {
        tool_call_strategy: "openai_tool_calls".to_owned(),
        ..ShapeAdapterInfo::default()
    };
    let state = GatewayState::from_models(vec![model])
        .with_session_backend(Arc::new(ToolCallDirectSessionBackend));
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{ "role": "user", "content": "Write a file." }],
        "tools": [{
            "type": "function",
            "function": { "name": "write", "parameters": { "type": "object" } }
        }],
        "stream": true
    });
    let (status, headers, bytes) = raw_request(
        openai_router(state),
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
    assert!(body.contains("\"tool_calls\":["));
    assert!(body.contains("\"id\":\"call-normalized\""));
    assert!(body.contains("\"type\":\"function\""));
    assert!(body.contains("\"name\":\"write\""));
    assert!(body.contains("\"arguments\":\"{\\\"filePath\\\":\\\"ok.txt\\\"}\""));
    assert!(body.contains("\"finish_reason\":\"tool_calls\""));
    assert!(body.contains("data: [DONE]"));
}

#[tokio::test]
async fn streaming_dev_chat_is_unbillable_and_stores_no_receipt() {
    let (state, app) = test_state_and_app();
    let model = first_model_id().await;
    let request = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Stream without billable accounting." }],
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
    assert!(body.contains("\"billable\":false"));
    assert!(body.contains("\"dev_session\":true"));
    assert!(body.contains("\"receipt\":null"));
    assert!(state.receipts().is_empty());

    let (status, body) = json_request(app, Method::GET, "/mayhem/receipts", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().expect("receipt list").len(), 0);
    assert_eq!(body["paused"].as_array().expect("paused list").len(), 0);
}

#[tokio::test]
async fn refused_receipt_cosign_pauses_session_without_storing_receipt() {
    let state = GatewayState::from_models(vec![routed_test_model()])
        .with_session_backend(Arc::new(TestDirectSessionBackend))
        .with_receipt_cosign_enabled(false);
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
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
    assert_eq!(body["mayhem"]["billable"], false);
    assert_eq!(body["mayhem"]["dev_session"], true);
    assert_eq!(body["mayhem"]["receipt"], Value::Null);

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
    assert!(body.contains("\"billable\":false"));
    assert!(body.contains("\"dev_session\":true"));
    assert!(body.contains("\"receipt\":null"));
    assert!(body.contains("data: [DONE]"));
    assert!(state.receipts().is_empty());
}

#[tokio::test]
async fn mayhem_local_endpoints_report_status_receipts_and_balance() {
    let (status, body) = json_request(test_app(), Method::GET, "/mayhem/status", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert_eq!(body["backend"], "local-openai-shape");
    assert_eq!(body["dev_session_shim"], true);

    let (status, body) =
        json_request(test_app(), Method::GET, "/mayhem/receipts", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "list");

    let (status, body) =
        json_request(test_app(), Method::GET, "/mayhem/balance", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["denom"], "mu_usd");
}

#[tokio::test]
async fn dashboard_requires_token_sets_csp_and_serves_no_external_assets() {
    let state = GatewayState::from_embedded_catalog();
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let query = dashboard_path
        .strip_prefix("/mayhem/dashboard")
        .expect("dashboard path");
    let app = openai_router(state);

    let (status, headers, bytes) =
        raw_request(app.clone(), Method::GET, "/mayhem/dashboard", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let csp = headers
        .get("content-security-policy")
        .and_then(|value| value.to_str().ok())
        .expect("dashboard CSP header");
    assert!(csp.contains("connect-src 'self' http://127.0.0.1:*"));
    assert!(!csp.contains("https:"));
    assert!(!csp.contains("http://") || csp.contains("http://127.0.0.1:*"));
    assert_eq!(
        headers
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let locked = String::from_utf8(bytes).expect("locked dashboard html");
    assert_no_external_urls(&locked);

    let (status, headers, bytes) =
        raw_request(app.clone(), Method::GET, dashboard_path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("set-cookie").is_some());
    let body = String::from_utf8(bytes).expect("dashboard html");
    assert!(body.contains("Runs entirely on this machine. No external network calls."));
    assert_no_external_urls(&body);

    let cookie = headers
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .expect("dashboard session cookie")
        .to_owned();
    let (status, _, _) = raw_request_with_headers(
        app.clone(),
        Method::GET,
        "/mayhem/dashboard",
        None,
        &[("cookie", &cookie)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let session_path = format!("/mayhem/dashboard/session{query}");
    let (status, body) = json_request(app, Method::GET, &session_path, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    let expires = body["expires_in_seconds"].as_u64().expect("expiry seconds");
    assert!(expires > 0);
    assert!(expires <= 900);
}

#[tokio::test]
async fn dashboard_component_gallery_uses_local_design_system_and_font_asset() {
    let state = GatewayState::from_embedded_catalog();
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let component_path =
        dashboard_path.replacen("/mayhem/dashboard", "/mayhem/dashboard/components", 1);
    let app = openai_router(state);

    let (status, headers, bytes) =
        raw_request(app.clone(), Method::GET, &component_path, None).await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("dashboard html");
    for expected in [
        "@font-face",
        "/mayhem/dashboard/assets/exo-latin.woff2",
        "--surface-card",
        "class=\"wordmark\"",
        "class=\"status-dot\"",
        "class=\"copy-chip\"",
        "class=\"count-chip\"",
        "class=\"empty-state\"",
        "class=\"chart-shell\"",
    ] {
        assert!(body.contains(expected), "missing {expected}");
    }
    assert_no_external_urls(&body);

    let cookie = headers
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .expect("dashboard session cookie")
        .to_owned();
    let (status, headers, bytes) = raw_request_with_headers(
        app,
        Method::GET,
        "/mayhem/dashboard/assets/exo-latin.woff2",
        None,
        &[("cookie", &cookie)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("font/woff2")
    );
    assert!(bytes.len() > 10_000);
}

#[tokio::test]
async fn user_dashboard_renders_live_gateway_data() {
    let state = GatewayState::from_embedded_catalog().with_dev_session_shim();
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let app = openai_router(state);
    let model = first_model_id().await;
    let request = json!({
        "model": model,
        "messages": [{"role": "user", "content": "hello"}]
    });
    let (status, _) =
        json_request(app.clone(), Method::POST, "/v1/chat/completions", request).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, bytes) = raw_request_with_headers(
        app,
        Method::GET,
        dashboard_path,
        None,
        &[("host", "127.0.0.1:11435")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("dashboard html");
    assert!(body.contains("User dashboard"));
    assert!(body.contains("$1.00"));
    assert!(body.contains("TAP rate not loaded"));
    assert!(body.contains("http://127.0.0.1:11435/v1"));
    assert!(body.contains("OPENAI_BASE_URL=http://127.0.0.1:11435/v1"));
    assert!(body.contains("Sessions"));
    assert!(body.contains("Models"));
    assert!(body.contains("Spend"));
    assert!(body.contains("Only Tier 3 keeps prompts private"));
    assert!(body.contains("Tier 4 can still read prompts"));
    assert!(body.contains("not a privacy ladder"));
    assert!(body.contains(&model));
    assert!(!body.contains("1,240.00 TAP"));
    assert_no_external_urls(&body);
}

#[tokio::test]
async fn provider_dashboard_renders_routes_receipts_and_earnings() {
    let provider = "55".repeat(32);
    let state = GatewayState::from_models(vec![routed_test_model_with_providers(
        std::slice::from_ref(&provider),
    )])
    .with_provider_earnings(vec![json!({
        "provider": provider,
        "denom": "mu_usd",
        "total_mu": 2_500_000_u64,
        "held_mu": 500_000_u64,
        "paid_cum_mu": 250_000_u64,
        "released_mu": 1_750_000_u64,
        "claimable_mu": 1_750_000_u64,
        "claim_model": "tap_non_custodial_claim",
        "holdbacks": [{"epoch": 7, "mu": 500_000_u64}],
        "updated_epoch": 9_u64
    })])
    .with_session_backend(Arc::new(TestDirectSessionBackend));
    let dashboard_url = state.dashboard_url("http://127.0.0.1:11435");
    let dashboard_path = dashboard_url
        .strip_prefix("http://127.0.0.1:11435")
        .expect("dashboard url is rooted at gateway");
    let query = dashboard_path
        .strip_prefix("/mayhem/dashboard?")
        .expect("dashboard token query");
    let provider_path = format!("/mayhem/dashboard/provider?{query}&provider={}", provider);
    let app = openai_router(state.clone());
    let request = json!({
        "model": "mayhem/routed-test",
        "messages": [{"role": "user", "content": "serve this provider session"}]
    });
    let (status, _) =
        json_request(app.clone(), Method::POST, "/v1/chat/completions", request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state.receipts().len(), 1);
    assert_eq!(state.receipts()[0].receipt.body.provider, provider);

    let (status, _, bytes) = raw_request_with_headers(
        app,
        Method::GET,
        &provider_path,
        None,
        &[("host", "127.0.0.1:11435")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(bytes).expect("provider dashboard html");
    assert!(body.contains("Provider dashboard"));
    assert!(body.contains("matches mayhem earnings"));
    assert!(body.contains("$2.50"));
    assert!(body.contains("$1.75"));
    assert!(body.contains("$0.25"));
    assert!(body.contains("mayhem/routed-test"));
    assert!(body.contains("Enclaves"));
    assert!(body.contains("Live sessions"));
    assert!(body.contains("Earnings"));
    assert!(body.contains("Reputation / Holdback"));
    assert!(body.contains("Hardware / Health"));
    assert!(body.contains("mayhem earnings --provider"));
    assert!(body.contains("mayhem withdraw --claim-proof"));
    assert!(!body.contains("ledger earnings not loaded"));
    assert_no_external_urls(&body);
}

#[test]
fn dashboard_bind_refuses_unspecified_and_lan_addresses() {
    assert!(
        validate_loopback_dashboard_bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 11_435))).is_ok()
    );
    assert!(validate_loopback_dashboard_bind(SocketAddr::from(([0, 0, 0, 0], 11_435))).is_err());
    assert!(
        validate_loopback_dashboard_bind(SocketAddr::from(([192, 168, 1, 20], 11_435))).is_err()
    );
}

fn assert_no_external_urls(html: &str) {
    assert!(!html.contains("https://"));
    for (index, _) in html.match_indices("http://") {
        assert!(
            html[index..].starts_with("http://127.0.0.1"),
            "unexpected non-local URL in dashboard HTML: {}",
            &html[index..html.len().min(index + 80)]
        );
    }
}
