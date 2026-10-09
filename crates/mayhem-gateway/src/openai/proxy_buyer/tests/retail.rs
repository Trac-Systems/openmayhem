use super::*;
use axum::{routing::post, Json};
use std::sync::Mutex;

struct Callback {
    records: Mutex<Vec<Value>>,
    state: Mutex<Option<GatewayState>>,
    mode: &'static str,
    release: Semaphore,
}
struct Server {
    callback: Arc<Callback>,
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    async fn start(mode: &'static str) -> Self {
        let callback = Arc::new(Callback {
            records: Mutex::new(vec![]),
            state: Mutex::new(None),
            mode,
            release: Semaphore::new(0),
        });
        let captured = callback.clone();
        let app = axum::Router::new().route("/hold", post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let callback = captured.clone();
            async move {
                assert_eq!(headers["authorization"], "Bearer fixture-retail-machine-key");
                let state = callback.state.lock().unwrap().clone().unwrap();
                let id = body["job_id"].as_str().unwrap();
                let job = state.jobs.lock().unwrap().get(id, now_secs()).unwrap().unwrap();
                assert!(job.proxy.as_ref().unwrap().terms().is_some(), "Core terms precede callback");
                assert!(job.result.is_none());
                assert_eq!(state.access_control.pending_key_budgets(None,64).unwrap().len(), 1);
                // Exercise the real owner-authenticated evidence route during
                // the gate. No Core job/budget lock may span this round trip.
                let evidence = openai_router(state).oneshot(Request::builder()
                    .uri(format!("/v1/jobs/{id}/proxy-evidence"))
                    .header("authorization", "Bearer owner-fixture-key").body(Body::empty()).unwrap()).await.unwrap();
                assert_eq!(evidence.status(), StatusCode::OK);
                let evidence: Value = serde_json::from_slice(&to_bytes(evidence.into_body(),1024*1024).await.unwrap()).unwrap();
                assert_eq!(evidence["terms_hash"], body["terms_hash"]);
                assert_eq!(evidence["financial"]["kind"], "pending");
                assert_eq!(body.as_object().unwrap().len(),5);
                callback.records.lock().unwrap().push(body.clone());
                match callback.mode {
                    "decline" => return StatusCode::PAYMENT_REQUIRED.into_response(),
                    "oversize" => return "x".repeat(8193).into_response(),
                    "redirect" => return (StatusCode::TEMPORARY_REDIRECT, [("location", "/hold")]).into_response(),
                    "timeout" => tokio::time::sleep(Duration::from_secs(2)).await,
                    "blocked" => callback.release.acquire().await.unwrap().forget(),
                    _ => (),
                }
                let mut reply = json!({"schema_version":1,"job_id":body["job_id"],"terms_hash":body["terms_hash"],"authorized":true});
                match callback.mode {
                    "wrong_job" => reply["job_id"] = json!("another-job"),
                    "wrong_terms" => reply["terms_hash"] = json!("0".repeat(64)),
                    "false" => reply["authorized"] = json!(false),
                    "extra" => reply["ignore"] = json!(true),
                    _ => (),
                }
                Json(reply).into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hold", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            callback,
            url,
            task,
        }
    }
    fn config(&self) -> RetailAuthorizationConfig {
        RetailAuthorizationConfig {
            url: self.url.clone(),
            credential: "fixture-retail-machine-key".into(),
            owner_token_ids: vec!["owner".into()],
            timeout_ms: if self.callback.mode == "blocked" {
                5000
            } else {
                1000
            },
        }
    }
    async fn fixture(&self) -> Fixture {
        let fixture =
            Fixture::start_with_retail(ProxyEndpoint::Chat, ProxyRail::Tnk, Some(self.config()))
                .await;
        *self.callback.state.lock().unwrap() = Some(fixture.state.clone());
        fixture
    }
    fn count(&self) -> usize {
        self.callback.records.lock().unwrap().len()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[test]
fn proxy_selection_rejections_have_scoped_non_admission_codes() {
    for (error, status, code) in [
        (
            proxy_request::Error::Price,
            StatusCode::PAYMENT_REQUIRED,
            "proxy_price_limit_exceeded",
        ),
        (
            proxy_request::Error::Constraints,
            StatusCode::BAD_REQUEST,
            "proxy_constraints_not_met",
        ),
        (
            proxy_request::Error::Invalid,
            StatusCode::BAD_REQUEST,
            "proxy_request_invalid",
        ),
    ] {
        let error = selection_error(error);
        assert_eq!(error.status, status);
        assert_eq!(error.public_code, code);
        assert_eq!(error.category, "proxy_admission");
        assert!(!error.retryable);
    }
    // Invalid is defensive in selection_error: the current resolver produces
    // it only during earlier parsing. Do not broaden that generic error or any
    // error which can also occur after an owner job exists.
    assert_eq!(invalid().public_code, "invalid_request");
    assert_eq!(invalid().category, "request_validation");
    assert_eq!(
        selection_error(proxy_request::Error::Busy).category,
        "proxy"
    );
    assert_eq!(unavailable().category, "proxy");
}

#[tokio::test]
async fn proxy_http_selection_rejections_create_no_job_spend_or_callback() {
    let server = Server::start("accept").await;
    let mut f = server.fixture().await;
    for (case, status, code, category) in [
        (
            "price",
            StatusCode::PAYMENT_REQUIRED,
            "proxy_price_limit_exceeded",
            "proxy_admission",
        ),
        (
            "constraints",
            StatusCode::BAD_REQUEST,
            "proxy_constraints_not_met",
            "proxy_admission",
        ),
        (
            "invalid-parse",
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "request_validation",
        ),
    ] {
        let mut body = f.body();
        match case {
            "price" => body["proxy"]["prices"]["rates"][0]["per_unit_au"] = json!("0"),
            "constraints" => body["proxy"]["minimum_context"] = json!(u32::MAX),
            "invalid-parse" => body["proxy"]["minimum_context"] = json!(0),
            _ => unreachable!(),
        }
        let (actual_status, headers, response) =
            f.post(case, body, "owner-fixture-key", false).await;
        assert_eq!(actual_status, status, "{case}: {response}");
        assert_eq!(response["error"]["code"], code, "{case}");
        assert_eq!(response["error"]["category"], category, "{case}");
        assert_eq!(response["error"]["retryable"], false, "{case}");
        assert!(headers.get("x-mayhem-job-id").is_none());
        let id = gateway_job_id(
            f.harness.buyer_seed,
            Some("owner"),
            "openai_chat_completions",
            Some(case),
        )
        .unwrap();
        assert!(f
            .state
            .jobs
            .lock()
            .unwrap()
            .get(&id, now_secs())
            .unwrap()
            .is_none());
        assert!(f
            .state
            .access_control
            .pending_key_budgets(None, 64)
            .unwrap()
            .is_empty());
        assert_eq!(
            f.state.access_control.summary()["tokens"][0]["spent_total_au"],
            "0"
        );
        assert_eq!(server.count(), 0);
        assert_eq!(f.harness.backend_calls(), 0);
        assert_eq!(f.harness.status().await["publications"], 0);
    }
    f.stop().await;
}

#[tokio::test]
async fn proxy_http_retail_gate_pins_core_before_callback_and_calls_once_before_execute() {
    let server = Server::start("accept").await;
    let mut f = server.fixture().await;
    let no_key = f
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("authorization", "Bearer owner-fixture-key")
                .body(Body::from(serde_json::to_vec(&f.body()).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(no_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(server.count(), 0);
    let body = f.body();
    let (status, headers, result) = f
        .post(
            "retail-request-one",
            body.clone(),
            "owner-fixture-key",
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let mut provider_body = body.clone();
    provider_body.as_object_mut().unwrap().remove("proxy");
    let recorded = server.callback.records.lock().unwrap()[0].clone();
    assert_eq!(recorded["request_id"], "retail-request-one");
    assert_eq!(
        recorded["job_id"],
        headers["x-mayhem-job-id"].to_str().unwrap()
    );
    assert_eq!(
        recorded["request_content_digest"],
        retail_request_content_digest(&provider_body).unwrap()
    );
    let (status, _, replay) = f
        .post(
            "retail-request-one",
            body.clone(),
            "owner-fixture-key",
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, result);
    assert_eq!(server.count(), 1);
    assert_eq!(f.harness.backend_calls(), 1);
    // Removing callback participation cannot reinterpret the saved request;
    // replay detects a different fingerprint before another owner can claim it.
    let direct = Arc::new(
        Runtime::new(
            f.harness.buyer.clone(),
            f.runtime.policy.clone(),
            f.harness.policy.clone(),
            2,
        )
        .unwrap(),
    );
    let direct = openai_router(f.state.clone().with_proxy_buyer(direct).unwrap());
    let bypass = direct
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("authorization", "Bearer owner-fixture-key")
                .header("idempotency-key", "retail-request-one")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bypass.status(), StatusCode::CONFLICT);
    let mut changed = body.clone();
    changed["messages"][0]["content"] = json!("different content, same cost");
    assert_eq!(
        f.post("retail-request-one", changed, "owner-fixture-key", false)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(server.count(), 1);
    assert_eq!(
        f.post("direct-other-owner", body, "other-fixture-key", false)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        server.count(),
        1,
        "allowlist never applies the callback to another owner"
    );
    f.stop().await;
}

#[tokio::test]
async fn proxy_http_retail_decline_ambiguous_and_malformed_replies_fence_original_without_execute()
{
    for mode in [
        "decline",
        "wrong_job",
        "wrong_terms",
        "false",
        "extra",
        "oversize",
        "redirect",
        "timeout",
    ] {
        let server = Server::start(mode).await;
        let mut f = server.fixture().await;
        let (status, headers, body) = f
            .post("retail-denied", f.body(), "owner-fixture-key", false)
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{mode}: {body}");
        let id = headers["x-mayhem-job-id"].to_str().unwrap();
        let (_, evidence) = f
            .get(
                &format!("/v1/jobs/{id}/proxy-evidence"),
                "owner-fixture-key",
            )
            .await;
        assert_eq!(
            evidence["financial"]["kind"], "non_admission",
            "{mode}: {evidence}"
        );
        assert_eq!(evidence["financial"]["budget_settled"], true);
        assert!(
            !evidence["terms"].is_null(),
            "a possible retail hold retains the exact terms"
        );
        assert_eq!(
            f.state
                .access_control
                .pending_key_budgets(None, 64)
                .unwrap()
                .len(),
            0
        );
        assert_eq!(f.harness.backend_calls(), 0);
        assert_eq!(f.harness.status().await["publications"], 0);
        assert_eq!(server.count(), 1, "redirects and retries are forbidden");
        assert_eq!(
            f.post("retail-denied", f.body(), "owner-fixture-key", false)
                .await
                .0,
            StatusCode::CONFLICT
        );
        assert_eq!(server.count(), 1);
        f.stop().await;
    }
}

#[tokio::test]
async fn proxy_http_retail_recovery_never_requests_another_hold_or_execute() {
    let server = Server::start("accept").await;
    let mut f = server.fixture().await;
    f.harness.command("publish_pending").await;
    let (status, headers, _) = f
        .post("retail-pending", f.body(), "owner-fixture-key", false)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let id = headers["x-mayhem-job-id"].to_str().unwrap().to_owned();
    assert_eq!(server.count(), 1);
    f.harness.command("flush_publication").await;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let job = f
                .state
                .jobs
                .lock()
                .unwrap()
                .get(&id, now_secs())
                .unwrap()
                .unwrap();
            if job.proxy.as_ref().unwrap().authorization().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        f.post("retail-pending", f.body(), "owner-fixture-key", false)
            .await
            .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(server.count(), 1);
    assert_eq!(f.harness.backend_calls(), 0);
    f.stop().await;
}

#[tokio::test]
async fn proxy_http_retail_cancel_joins_acknowledgment_then_fences_before_signing() {
    let server = Server::start("blocked").await;
    let mut f = server.fixture().await;
    let (status, headers, _) = f
        .post("retail-cancel", f.body(), "owner-fixture-key", true)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let id = headers["x-mayhem-job-id"].to_str().unwrap().to_owned();
    tokio::time::timeout(Duration::from_secs(10), async {
        while server.count() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let cancelled = f
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/jobs/{id}/cancel"))
                .header("authorization", "Bearer owner-fixture-key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancelled.status(), StatusCode::ACCEPTED);
    assert_eq!(
        f.state
            .access_control
            .pending_key_budgets(None, 64)
            .unwrap()
            .len(),
        1
    );
    server.callback.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let job = f
                .state
                .jobs
                .lock()
                .unwrap()
                .get(&id, now_secs())
                .unwrap()
                .unwrap();
            if job.status == GatewayJobStatus::Failed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let (_, evidence) = f
        .get(
            &format!("/v1/jobs/{id}/proxy-evidence"),
            "owner-fixture-key",
        )
        .await;
    assert_eq!(evidence["financial"]["kind"], "non_admission");
    assert_eq!(evidence["financial"]["budget_settled"], true);
    assert_eq!(server.count(), 1);
    assert_eq!(f.harness.backend_calls(), 0);
    assert_eq!(f.harness.status().await["publications"], 0);
    f.stop().await;
}

#[tokio::test]
async fn proxy_http_required_retail_hook_fails_before_job_or_spend_when_missing_or_other_owner() {
    for missing in [true, false] {
        let server = Server::start("accept").await;
        let mut f = if missing {
            Fixture::start().await
        } else {
            server.fixture().await
        };
        let (token, owner) = if missing {
            ("owner-fixture-key", "owner")
        } else {
            ("other-fixture-key", "other")
        };
        for (header, status) in [
            ("1", StatusCode::SERVICE_UNAVAILABLE),
            ("0", StatusCode::BAD_REQUEST),
            ("true", StatusCode::BAD_REQUEST),
        ] {
            let response = f
                .router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/chat/completions")
                        .header("content-type", "application/json")
                        .header("authorization", format!("Bearer {token}"))
                        .header("idempotency-key", "required-hook")
                        .header("x-mayhem-require-retail-authorization", header)
                        .body(Body::from(serde_json::to_vec(&f.body()).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            assert!(response.headers().get("x-mayhem-job-id").is_none());
        }
        let id = gateway_job_id(
            f.harness.buyer_seed,
            Some(owner),
            "openai_chat_completions",
            Some("required-hook"),
        )
        .unwrap();
        assert!(f
            .state
            .jobs
            .lock()
            .unwrap()
            .get(&id, now_secs())
            .unwrap()
            .is_none());
        assert!(f
            .state
            .access_control
            .pending_key_budgets(None, 64)
            .unwrap()
            .is_empty());
        assert_eq!(server.count(), 0);
        assert_eq!(f.harness.backend_calls(), 0);
        assert_eq!(f.harness.status().await["publications"], 0);
        f.stop().await;
    }
}
