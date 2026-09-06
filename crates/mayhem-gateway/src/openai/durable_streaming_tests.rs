use super::tests::{
    test_chat_output, test_chat_request, test_invocation, test_model, test_provider_receipt,
    test_provider_receipt_with_finality, test_receipt_settlement_feature,
};
use super::*;
use tower::ServiceExt;

const SURFACES: [(&str, &str); 3] = [
    (
        "/v1/chat/completions",
        mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
    ),
    ("/v1/completions", mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS),
    ("/v1/responses", mayhem_proto::ENDPOINT_OPENAI_RESPONSES),
];

fn model() -> GatewayModel {
    let mut model = test_model();
    model.mayhem.adapter.endpoint_families = SURFACES
        .iter()
        .map(|(_, family)| mayhem_proto::endpoint_family_contract_template(family).unwrap())
        .collect();
    model
}

fn request(model: &str, family: &str) -> Value {
    let mut request = json!({"model": model, "stream": true});
    match family {
        mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS => {
            request["messages"] = json!([{"role": "user", "content": "Hello"}])
        }
        mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS => request["prompt"] = json!("Hello"),
        _ => request["input"] = json!("Hello"),
    }
    request
}

fn headers(key: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("idempotency-key", HeaderValue::from_str(key).unwrap());
    headers
}

async fn start(
    state: &GatewayState,
    family: &str,
    raw: &Value,
    headers: &HeaderMap,
) -> GatewayJobHandle {
    match prepare_gateway_job(
        state,
        headers,
        family,
        raw["model"].as_str().unwrap(),
        raw,
        &state.authorize_gateway_request(headers, None).unwrap(),
    )
    .await
    .unwrap()
    {
        PreparedGatewayJob::Started(job) => job,
        _ => panic!("new key must start once"),
    }
}

async fn body(response: Response) -> Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn lookup_request(family: &str, headers: &HeaderMap) -> axum::http::Request<Body> {
    let mut request = axum::http::Request::builder()
        .uri(format!("/v1/jobs/lookup?endpoint_family={family}"))
        .body(Body::empty())
        .unwrap();
    *request.headers_mut() = headers.clone();
    request
}

fn adapt(events: SseEventStream, family: &str, model: String) -> ChatResponse {
    match family {
        mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS => {
            completion_response_from_chat_response(ChatResponse::SseStream(events)).unwrap()
        }
        mayhem_proto::ENDPOINT_OPENAI_RESPONSES => {
            ChatResponse::SseStream(response_stream::from_chat(events, model))
        }
        _ => ChatResponse::SseStream(events),
    }
}

#[tokio::test]
async fn content_precedes_receipt_and_terminal_waits_for_durable_ack_on_all_surfaces() {
    for (_, family) in SURFACES {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("jobs");
        let model = model();
        let state = GatewayState::from_models(vec![model.clone()])
            .with_job_store_dir(directory.clone())
            .unwrap();
        let raw = request(&model.id, family);
        let headers = headers("live-order");
        let job = start(&state, family, &raw, &headers).await;
        let mut invocation = test_invocation();
        invocation.transport_peer = Some("ab".repeat(32));
        invocation.job = Some(job.clone());
        let output = test_chat_output();
        let receipt =
            test_provider_receipt(&model, &test_chat_request(&model.id), &output, &invocation);
        let ack = receipt_ack_for_body(&invocation.receipt_user_seed, &receipt.body).unwrap();
        let receipt_gate = Arc::new(Notify::new());
        let ack_gate = Arc::new(Notify::new());
        let staged = Arc::new(Notify::new());
        let producer_receipt_gate = receipt_gate.clone();
        let producer_ack_gate = ack_gate.clone();
        let producer_staged = staged.clone();
        let expected_receipt = receipt.clone();
        let expected_ack = ack.clone();
        let receipts = invocation.receipt_recorder.receipts.clone();
        let stream = owned_live_sse_stream(Some(job.clone()), 1, move |tx| async move {
            assert!(
                send_sse_value(
                    &tx,
                    chat_chunk(
                        "chat_live",
                        1,
                        &model.id,
                        json!({"content": "live text"}),
                        None,
                        None
                    )
                )
                .await
            );
            producer_receipt_gate.notified().await;
            reconcile_and_persist_completed_invocation_job_with_ack(
                &invocation,
                chat_job_result(&output),
                &[],
                &receipt,
                &ack,
                async {
                    producer_staged.notify_one();
                    producer_ack_gate.notified().await;
                    Ok(())
                },
            )
            .await
            .map_err(|error| provider_session_api_error(&error))?;
            send_sse_value(
                &tx,
                chat_chunk("chat_live", 1, &model.id, json!({}), Some("stop"), None),
            )
            .await;
            send_sse_value(&tx, json!({"id": "chat_live", "object": "chat.completion.chunk", "choices": [], "usage": {"prompt_tokens": 2, "completion_tokens": 2, "total_tokens": 4}})).await;
            Ok(())
        });
        let response = gateway_stream_job_response(job.clone(), async move {
            Ok(adapt(
                stream,
                family,
                raw["model"].as_str().unwrap().to_owned(),
            ))
        })
        .await;
        assert_eq!(response.headers()["x-mayhem-job-id"], job.id);
        assert!(!response.headers().contains_key("x-mayhem-session-id"));
        let mut chunks = response.into_body().into_data_stream();
        let first = tokio::time::timeout(Duration::from_secs(2), async {
            let mut text = String::new();
            while !text.contains("live text") {
                text.push_str(std::str::from_utf8(&chunks.next().await.unwrap().unwrap()).unwrap());
            }
            text
        })
        .await
        .unwrap();
        assert!(!first.contains("[DONE]"));
        assert!(!first.contains("response.completed"));
        assert!(job.is_active());
        assert!(state
            .jobs
            .lock_recover("jobs")
            .get(&job.id, now_secs())
            .unwrap()
            .is_none());
        let lookup = openai_router(state.clone())
            .oneshot(lookup_request(family, &headers))
            .await
            .unwrap();
        assert_eq!(lookup.status(), StatusCode::ACCEPTED);
        assert_eq!(body(lookup).await["id"], job.id);
        receipt_gate.notify_one();
        tokio::time::timeout(Duration::from_secs(2), staged.notified())
            .await
            .unwrap();
        let pending = state
            .jobs
            .lock_recover("jobs")
            .get(&job.id, now_secs())
            .unwrap()
            .unwrap();
        assert_eq!(pending.status, GatewayJobStatus::ReconciliationPending);
        let durable: GatewayJobSettledReceipt =
            serde_json::from_value(pending.receipt.unwrap()).unwrap();
        assert_eq!(durable.body, expected_receipt.body);
        assert_eq!(durable.enclave_sig, expected_receipt.enclave_sig);
        assert_eq!(durable.receipt_ack, expected_ack);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), chunks.next())
                .await
                .is_err()
        );
        ack_gate.notify_one();
        let tail = tokio::time::timeout(Duration::from_secs(2), async {
            let mut text = String::new();
            while let Some(chunk) = chunks.next().await {
                text.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
            }
            text
        })
        .await
        .unwrap();
        if family == mayhem_proto::ENDPOINT_OPENAI_RESPONSES {
            assert!(tail.contains("event: response.completed"));
            assert!(!tail.contains("[DONE]"));
        } else {
            assert!(tail.contains("[DONE]"));
        }
        assert!(!job.is_active());
        receipts.lock_recover("receipt history").clear();
        let restarted = GatewayState::from_models(vec![self::model()])
            .with_job_store_dir(directory)
            .unwrap();
        let recovered = openai_router(restarted.clone())
            .oneshot(lookup_request(family, &headers))
            .await
            .unwrap();
        let recovered = body(recovered).await;
        assert_eq!(recovered["status"], "completed");
        assert_eq!(recovered["receipt"]["body"], json!(expected_receipt.body));
        assert_eq!(recovered["receipt"]["receipt_ack"], json!(expected_ack));
        let raw = request(&self::model().id, family);
        assert!(matches!(
            prepare_gateway_job(
                &restarted,
                &headers,
                family,
                raw["model"].as_str().unwrap(),
                &raw,
                &None
            )
            .await
            .unwrap(),
            PreparedGatewayJob::Existing(_)
        ));
    }
}

#[derive(Debug)]
struct CountingBackend(Arc<AtomicU64>);

impl GatewaySessionBackend for CountingBackend {
    fn name(&self) -> &str {
        "unit-counting"
    }

    fn run_chat<'a>(
        &'a self,
        _model: &'a GatewayModel,
        request: &'a ChatCompletionRequest,
        invocation: &'a GatewaySessionInvocation,
    ) -> GatewaySessionFuture<'a> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            assert!(request.stream);
            assert!(invocation.job.is_some());
            Ok(GatewaySessionResult {
                output: test_chat_output(),
                backend: self.name().to_owned(),
                direct_session: true,
                provider_receipt: None,
                token_ids: Vec::new(),
                quality: None,
            })
        })
    }
}

#[tokio::test]
async fn post_replay_never_dispatches_twice_and_preserves_all_endpoint_semantics() {
    for (path, family) in SURFACES {
        let model = model();
        let count = Arc::new(AtomicU64::new(0));
        let state = GatewayState::from_models(vec![model.clone()])
            .with_dev_session_shim()
            .with_session_backend(Arc::new(CountingBackend(count.clone())));
        let app = openai_router(state.clone());
        let raw = request(&model.id, family);
        let post = |raw: &Value| {
            axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .header("idempotency-key", "one-inference")
                .header("prefer", "respond-async")
                .body(Body::from(raw.to_string()))
                .unwrap()
        };
        let first = app.clone().oneshot(post(&raw)).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK, "{}", body(first).await);
        let id = first.headers()["x-mayhem-job-id"].clone();
        let bytes = axum::body::to_bytes(first.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        if family == mayhem_proto::ENDPOINT_OPENAI_RESPONSES {
            assert!(text.contains("response.output_text.delta"));
            assert!(text.contains("response.completed"));
        } else if family == mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS {
            assert!(text.contains("text_completion"));
            assert!(text.contains("\"usage\""));
        }
        let replay = app.clone().oneshot(post(&raw)).await.unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(replay.headers()["x-mayhem-job-id"], id);
        assert_eq!(body(replay).await["object"], "mayhem.job");
        let mut changed = raw.clone();
        changed["stream"] = json!(false);
        assert_eq!(
            app.clone().oneshot(post(&changed)).await.unwrap().status(),
            StatusCode::CONFLICT
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let contract = catalog_endpoint_contract(&state, &model.id, family).unwrap();
        let normalized = normalize_endpoint_request_for_provider(&contract, &raw)
            .unwrap()
            .normalized_request;
        let chat = match family {
            mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS => completion_chat_request(
                serde_json::from_value(normalized.clone()).unwrap(),
                normalized.clone(),
            )
            .unwrap(),
            mayhem_proto::ENDPOINT_OPENAI_RESPONSES => responses_chat_request(
                serde_json::from_value(normalized.clone()).unwrap(),
                normalized.clone(),
            )
            .unwrap(),
            _ => continue,
        };
        assert!(chat.stream);
        assert_eq!(chat.endpoint_request.as_ref(), Some(&normalized));
        assert_eq!(chat.endpoint_family.as_deref(), Some(family));
        assert!(chat.stream_options.unwrap().include_usage.unwrap());
    }
}

#[tokio::test]
async fn checkpoint_evidence_survives_failure_restart_and_history_rotation() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("jobs");
    let state = GatewayState::from_models(vec![model()])
        .with_job_store_dir(directory.clone())
        .unwrap();
    let raw = request(&model().id, SURFACES[0].1);
    let headers = headers("checkpoint");
    let job = start(&state, SURFACES[0].1, &raw, &headers).await;
    let mut invocation = test_invocation();
    invocation.transport_peer = Some("ab".repeat(32));
    invocation.job = Some(job.clone());
    let receipt = test_provider_receipt_with_finality(
        &model(),
        &test_chat_request(&model().id),
        &test_chat_output(),
        &invocation,
        1,
        false,
    );
    let ack = receipt_ack_for_body(&invocation.receipt_user_seed, &receipt.body).unwrap();
    record_direct_session_receipt(&invocation, &receipt, &ack).unwrap();
    let feature = test_receipt_settlement_feature(&receipt, &ack);
    job.persist_reconciliation_settlement_feature(feature.clone())
        .await
        .unwrap();
    record_direct_session_receipt(&invocation, &receipt, &ack).unwrap();
    let active = body(
        openai_router(state.clone())
            .oneshot(lookup_request(SURFACES[0].1, &headers))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(active["status"], "in_progress");
    assert_eq!(active["receipt"]["body"], json!(receipt.body));
    job.persist_failure_if_active(&ApiError::bad_gateway("stream failed", None))
        .await;
    invocation
        .receipt_recorder
        .receipts
        .lock_recover("receipts")
        .clear();
    let restarted = GatewayState::from_models(vec![model()])
        .with_job_store_dir(directory)
        .unwrap();
    let recovered = body(
        openai_router(restarted.clone())
            .oneshot(lookup_request(SURFACES[0].1, &headers))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(recovered["status"], "reconciliation_pending");
    assert_eq!(recovered["receipt"]["body"]["final"], false);
    assert_eq!(recovered["receipt"]["enclave_sig"], receipt.enclave_sig);
    assert_eq!(recovered["receipt"]["receipt_ack"], json!(ack));
    assert_eq!(
        recovered["receipt"]["reconciliation"]["settlement_feature"],
        feature
    );
    let pending = restarted
        .jobs
        .lock_recover("jobs")
        .get(&job.id, now_secs())
        .unwrap()
        .unwrap();
    assert!(
        !parse_gateway_job_receipt_recovery(&pending)
            .unwrap()
            .body
            .final_receipt
    );
    let publisher = Arc::new(TestPublisher::default());
    let restarted = restarted.with_receipt_settlement_publisher(publisher.clone());
    reconcile_pending_gateway_job_once(&restarted, &job.id, &NoDelivery)
        .await
        .unwrap();
    reconcile_pending_gateway_job_once(&restarted, &job.id, &NoDelivery)
        .await
        .unwrap();
    assert_eq!(
        publisher.0.lock_recover("published features").as_slice(),
        &[feature]
    );
    assert_eq!(
        restarted
            .jobs
            .lock_recover("jobs")
            .get(&job.id, now_secs())
            .unwrap()
            .unwrap()
            .status,
        GatewayJobStatus::Cancelled
    );
}

#[derive(Debug, Default)]
struct TestPublisher(Mutex<Vec<Value>>);

impl GatewayReceiptSettlementPublisher for TestPublisher {
    fn admission_available(&self) -> Result<bool, String> {
        Ok(true)
    }
    fn queue(&self, feature: &Value) -> Result<(), String> {
        self.0
            .lock_recover("published features")
            .push(feature.clone());
        Ok(())
    }
}

#[derive(Debug)]
struct NoDelivery;

impl GatewayReceiptAckRecoveryTransport for NoDelivery {
    fn deliver<'a>(
        &'a self,
        _recovery: &'a GatewayJobSettledReceipt,
    ) -> GatewayReceiptAckRecoveryFuture<'a> {
        Box::pin(async { panic!("persisted settlement handoff must not contact a provider") })
    }
}

#[tokio::test]
async fn ack_failure_is_an_sse_error_with_recoverable_signed_receipt_on_every_surface() {
    for (_, family) in SURFACES {
        let state = GatewayState::from_models(vec![model()]);
        let raw = request(&model().id, family);
        let headers = headers("ack-failure");
        let job = start(&state, family, &raw, &headers).await;
        let mut invocation = test_invocation();
        invocation.transport_peer = Some("ab".repeat(32));
        invocation.job = Some(job.clone());
        let receipt = test_provider_receipt(
            &model(),
            &test_chat_request(&model().id),
            &test_chat_output(),
            &invocation,
        );
        let ack = receipt_ack_for_body(&invocation.receipt_user_seed, &receipt.body).unwrap();
        let expected = receipt.clone();
        let stream = owned_live_sse_stream(Some(job.clone()), 1, move |tx| async move {
            send_sse_value(
                &tx,
                chat_chunk(
                    "chat_error",
                    1,
                    &model().id,
                    json!({"content": "visible"}),
                    None,
                    None,
                ),
            )
            .await;
            reconcile_and_persist_completed_invocation_job_with_ack(
                &invocation,
                chat_job_result(&test_chat_output()),
                &[],
                &receipt,
                &ack,
                async {
                    Err(GatewaySessionError::retryable(
                        "receipt acknowledgement failed",
                    ))
                },
            )
            .await
            .map_err(|error| provider_session_api_error(&error))
        });
        let response = gateway_stream_job_response(job.clone(), async move {
            Ok(adapt(stream, family, model().id))
        })
        .await;
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.contains("visible"));
        assert!(text.contains("\"error\""));
        assert!(!text.contains("response.completed"));
        if family == mayhem_proto::ENDPOINT_OPENAI_RESPONSES {
            assert!(text.contains("event: response.failed"));
        } else {
            assert!(text.contains("[DONE]"));
        }
        let pending = state
            .jobs
            .lock_recover("jobs")
            .get(&job.id, now_secs())
            .unwrap()
            .unwrap();
        assert_eq!(pending.status, GatewayJobStatus::ReconciliationPending);
        assert_eq!(pending.receipt.unwrap()["body"], json!(expected.body));
        assert!(matches!(
            prepare_gateway_job(&state, &headers, family, &model().id, &raw, &None)
                .await
                .unwrap(),
            PreparedGatewayJob::Existing(_)
        ));
    }
}

#[tokio::test]
async fn disconnect_before_headers_keeps_detached_owner_and_never_restarts_the_key() {
    let state = GatewayState::from_models(vec![model()]);
    let family = SURFACES[0].1;
    let raw = request(&model().id, family);
    let headers = headers("before-headers");
    let job = start(&state, family, &raw, &headers).await;
    let started = Arc::new(Notify::new());
    let task_started = started.clone();
    let cancellation = job.cancellation();
    let outer = tokio::spawn(gateway_stream_job_response(job.clone(), async move {
        task_started.notify_one();
        cancellation.cancelled().await;
        Err(ApiError::bad_gateway(
            "cancelled during stream preparation",
            None,
        ))
    }));
    started.notified().await;
    assert!(matches!(
        prepare_gateway_job(&state, &headers, family, &model().id, &raw, &None)
            .await
            .unwrap(),
        PreparedGatewayJob::InProgress(_)
    ));
    outer.abort();
    let _ = outer.await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while job.is_active() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let response = openai_router(state.clone())
        .oneshot(lookup_request(family, &headers))
        .await
        .unwrap();
    assert_eq!(body(response).await["status"], "cancelled");
    assert!(matches!(
        prepare_gateway_job(&state, &headers, family, &model().id, &raw, &None)
            .await
            .unwrap(),
        PreparedGatewayJob::Existing(_)
    ));
}

#[tokio::test]
async fn dropped_stream_still_publishes_checkpoint_evidence_and_unregisters_owner() {
    let state = GatewayState::from_models(vec![model()]);
    let raw = request(&model().id, SURFACES[0].1);
    let job = start(&state, SURFACES[0].1, &raw, &headers("drop-stream")).await;
    let mut invocation = test_invocation();
    invocation.transport_peer = Some("ab".repeat(32));
    invocation.job = Some(job.clone());
    let receipt = test_provider_receipt_with_finality(
        &model(),
        &test_chat_request(&model().id),
        &test_chat_output(),
        &invocation,
        1,
        false,
    );
    let ack = receipt_ack_for_body(&invocation.receipt_user_seed, &receipt.body).unwrap();
    let task_job = job.clone();
    let mut stream = owned_live_sse_stream(Some(job.clone()), 1, move |tx| async move {
        send_sse_value(&tx, json!({"visible": "partial"})).await;
        tx.closed().await;
        record_direct_session_receipt(&invocation, &receipt, &ack).unwrap();
        task_job
            .persist_cancelled_if_active("client_disconnect")
            .await;
        Ok(())
    });
    assert!(stream.next().await.unwrap().is_some());
    drop(stream);
    tokio::time::timeout(Duration::from_secs(2), async {
        while job.is_active()
            || state
                .active_job_cancellations
                .lock_recover("owners")
                .contains_key(&job.id)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let pending = state
        .jobs
        .lock_recover("jobs")
        .get(&job.id, now_secs())
        .unwrap()
        .unwrap();
    assert_eq!(pending.status, GatewayJobStatus::ReconciliationPending);
    assert!(pending.receipt.unwrap()["receipt_ack"]["user_sig"].is_string());
}

#[tokio::test]
async fn lost_headers_lookup_and_restart_replay_are_owner_scoped_and_read_only() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("jobs");
    let token = |id: &str| GatewayTokenRecord {
        name: id.to_owned(),
        token_hash: gateway_token_hash(id),
        token_id: id.to_owned(),
        created_at: 1,
        expires_at: None,
        budget_au: None,
        budget_period: None,
        spent_total_au: 0,
        spent_period_au: 0,
        period_started_at: Some(1),
        max_rate_per_minute: None,
        models: Vec::new(),
        last_used_at: None,
        revoked_at: None,
    };
    let access = || {
        GatewayAccessControl::new(
            true,
            GatewayTokenStore {
                version: 1,
                tokens: vec![token("owner"), token("other")],
            },
            None,
        )
    };
    let state = GatewayState::from_models(vec![model()])
        .with_access_control(access())
        .with_job_store_dir(directory.clone())
        .unwrap();
    let family = SURFACES[0].1;
    let mut owner = headers("lost-headers");
    owner.insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer owner"),
    );
    let mut other = owner.clone();
    other.insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer other"),
    );
    let raw = request(&model().id, family);
    let job = start(&state, family, &raw, &owner).await;
    let app = openai_router(state.clone());
    let first = app
        .clone()
        .oneshot(lookup_request(family, &owner))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    assert_eq!(body(first).await["id"], job.id);
    let missing = app
        .clone()
        .oneshot(lookup_request(family, &other))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let missing_body = body(missing).await;
    let mut direct = axum::http::Request::builder()
        .uri(format!("/v1/jobs/{}", job.id))
        .body(Body::empty())
        .unwrap();
    *direct.headers_mut() = other.clone();
    let hidden = app.clone().oneshot(direct).await.unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    assert_eq!(body(hidden).await, missing_body);
    for invalid in ["", "responses", "../../jobs", "unknown"] {
        assert_eq!(
            app.clone()
                .oneshot(lookup_request(invalid, &owner))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    for key in ["".to_owned(), "x".repeat(256)] {
        let mut invalid = owner.clone();
        invalid.insert("idempotency-key", HeaderValue::from_str(&key).unwrap());
        assert_eq!(
            app.clone()
                .oneshot(lookup_request(family, &invalid))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let mut missing_key = owner.clone();
    missing_key.remove("idempotency-key");
    assert_eq!(
        app.clone()
            .oneshot(lookup_request(family, &missing_key))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.clone()
            .oneshot(lookup_request(family, &headers("lost-headers")))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let other_job = start(&state, family, &raw, &other).await;
    assert_ne!(job.id, other_job.id);
    assert!(job.is_active());
    drop(app);
    drop(job);
    drop(other_job);
    drop(state);
    let restarted = GatewayState::from_models(vec![model()])
        .with_access_control(access())
        .with_job_store_dir(directory)
        .unwrap();
    let recovered = body(
        openai_router(restarted.clone())
            .oneshot(lookup_request(family, &owner))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(recovered["status"], "failed");
    assert_eq!(
        recovered["error_info"]["code"],
        "gateway_execution_interrupted"
    );
    assert_eq!(recovered["error_info"]["retryable"], false);
    assert!(recovered["receipt"].is_null());
    let replay = prepare_gateway_job(
        &restarted,
        &owner,
        family,
        &model().id,
        &raw,
        &restarted.authorize_gateway_request(&owner, None).unwrap(),
    )
    .await
    .unwrap();
    assert!(matches!(replay, PreparedGatewayJob::Existing(_)));
}

#[tokio::test]
async fn status_advertises_the_three_live_surfaces_in_both_capability_locations() {
    let response = mayhem_status(State(Arc::new(GatewayState::fixture())), HeaderMap::new()).await;
    let value = body(response).await;
    assert_eq!(value["durable_streaming_jobs"], true);
    assert_eq!(value["capabilities"]["durable_streaming_jobs"], true);
    for (_, family) in SURFACES {
        assert!(value["durable_streaming_endpoint_families"]
            .as_array()
            .unwrap()
            .contains(&json!(family)));
    }
}
