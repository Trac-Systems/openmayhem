use super::*;
use futures_util::StreamExt;

impl Fixture {
    fn stream_body(&self) -> Value {
        let mut body = self.body();
        body["stream"] = json!(true);
        body
    }
    async fn stream_post(&self, key: &str) -> Response {
        self.stream_post_body(key, self.stream_body()).await
    }
    async fn stream_post_body(&self, key: &str, body: Value) -> Response {
        self.router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(support::endpoint_path(self.harness.adapter.endpoint()))
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer owner-fixture-key")
                    .header("idempotency-key", key)
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
}
fn payloads(bytes: &[u8]) -> Vec<Value> {
    std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|line| *line != "[DONE]")
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
async fn collect(body: Body) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(20), to_bytes(body, 2 * 1024 * 1024))
        .await
        .unwrap()
        .unwrap()
        .to_vec()
}
fn job_id(response: &Response) -> String {
    response.headers()["x-mayhem-job-id"]
        .to_str()
        .unwrap()
        .into()
}

#[tokio::test]
async fn proxy_sse_three_endpoints_all_rails_complete_then_replay_retained_json_once() {
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
    ] {
        for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
            let mut f = Fixture::start_with(endpoint, rail).await;
            let response = f.stream_post("stream-original").await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["content-type"], "text/event-stream");
            assert_eq!(response.headers()["cache-control"], "private, no-store");
            let id = job_id(&response);
            let bytes = collect(response.into_body()).await;
            let events = payloads(&bytes);
            assert!(events.len() > 1, "{endpoint:?}/{rail:?}: {events:?}");
            assert!(
                events.iter().all(|event| event.get("error").is_none()),
                "{events:?}"
            );
            let job = f
                .state
                .jobs
                .lock()
                .unwrap()
                .get(&id, now_secs())
                .unwrap()
                .unwrap();
            assert_eq!(job.status, GatewayJobStatus::Completed, "{events:?}");
            assert_eq!(
                f.state
                    .access_control
                    .pending_key_budgets(None, 64)
                    .unwrap()
                    .len(),
                0
            );
            let result = job.result.unwrap();
            if endpoint == ProxyEndpoint::Responses {
                assert_eq!(events.last().unwrap()["type"], "response.completed");
                assert_eq!(events.last().unwrap()["response"], result);
                for (i, event) in events.iter().enumerate() {
                    assert_eq!(event["sequence_number"], i);
                }
                assert!(events
                    .iter()
                    .any(|e| e["type"] == "response.output_text.delta"));
                assert!(events
                    .iter()
                    .any(|e| e["type"] == "response.output_item.done"));
                assert!(!bytes.windows(6).any(|v| v == b"[DONE]"));
            } else {
                assert!(std::str::from_utf8(&bytes)
                    .unwrap()
                    .ends_with("data: [DONE]\n\n"));
                assert!(events[..events.len() - 1]
                    .iter()
                    .all(|e| e["choices"][0]["finish_reason"].is_null()));
                assert_eq!(
                    events.last().unwrap()["choices"][0]["finish_reason"],
                    "stop"
                );
                assert_eq!(events.last().unwrap()["id"], result["id"]);
                assert_eq!(events.last().unwrap()["usage"], result["usage"]);
            }
            let replay = f.stream_post("stream-original").await;
            assert_eq!(replay.status(), StatusCode::OK);
            assert_eq!(replay.headers()["content-type"], "application/json");
            assert_eq!(job_id(&replay), id);
            assert_eq!(
                serde_json::from_slice::<Value>(&collect(replay.into_body()).await).unwrap(),
                result
            );
            let (changed, _, _) = f
                .post("stream-original", f.body(), "owner-fixture-key", false)
                .await;
            assert_eq!(changed, StatusCode::CONFLICT);
            assert_eq!(
                f.get(
                    &format!("/v1/jobs/{id}/proxy-evidence"),
                    "other-fixture-key"
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
            assert_eq!(f.harness.backend_calls(), 1);
            assert_eq!(f.harness.status().await["publications"], 2);
            assert_eq!(f.runtime.streams.available_permits(), 2);
            f.stop().await;
        }
    }
}

#[tokio::test]
async fn proxy_sse_delta_precedes_upstream_tail_and_canonical_closure_precedes_terminal() {
    let mut f = Fixture::start().await;
    f.harness
        .stream_backend
        .paused
        .store(true, Ordering::SeqCst);
    let response = f.stream_post("early-original").await;
    let id = job_id(&response);
    let mut body = response.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(10), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        payloads(&first)[0]["choices"][0]["delta"]["content"],
        "hello"
    );
    assert_eq!(f.harness.status().await["publications"], 1);
    let (_, evidence) = f
        .get(
            &format!("/v1/jobs/{id}/proxy-evidence"),
            "owner-fixture-key",
        )
        .await;
    assert_eq!(evidence["financial"]["kind"], "pending");
    assert_eq!(evidence["result_verified"], false);
    assert_eq!(
        f.stream_post("early-original").await.status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(f.harness.backend_calls(), 1);
    f.harness.stream_backend.release.add_permits(1);
    let mut tail = vec![];
    while let Some(frame) = tokio::time::timeout(Duration::from_secs(15), body.next())
        .await
        .unwrap()
    {
        tail.extend_from_slice(&frame.unwrap());
    }
    assert!(std::str::from_utf8(&tail).unwrap().contains("[DONE]"));
    let (_, evidence) = f
        .get(
            &format!("/v1/jobs/{id}/proxy-evidence"),
            "owner-fixture-key",
        )
        .await;
    assert_eq!(evidence["financial"]["outcome"]["kind"], "paid");
    assert_eq!(evidence["financial"]["budget_settled"], true);
    drop(body);
    f.stop().await;
}

#[tokio::test]
async fn proxy_sse_pending_receipt_has_no_success_terminal_and_recovers_original_result() {
    let mut f = Fixture::start().await;
    f.harness
        .stream_backend
        .paused
        .store(true, Ordering::SeqCst);
    let response = f.stream_post("closure-original").await;
    let id = job_id(&response);
    let mut body = response.into_body().into_data_stream();
    tokio::time::timeout(Duration::from_secs(10), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    f.harness.command("publish_pending").await;
    f.harness.stream_backend.release.add_permits(1);
    let mut tail = vec![];
    while let Some(frame) = tokio::time::timeout(Duration::from_secs(15), body.next())
        .await
        .unwrap()
    {
        tail.extend_from_slice(&frame.unwrap());
    }
    assert!(!std::str::from_utf8(&tail).unwrap().contains("[DONE]"));
    assert_eq!(
        payloads(&tail).last().unwrap()["error"]["code"],
        "proxy_recovery_required"
    );
    let (_, evidence) = f
        .get(
            &format!("/v1/jobs/{id}/proxy-evidence"),
            "owner-fixture-key",
        )
        .await;
    assert_eq!(evidence["result_verified"], true);
    assert_eq!(evidence["financial"]["kind"], "pending");
    let pending = f.stream_post("closure-original").await;
    assert_eq!(pending.status(), StatusCode::ACCEPTED);
    assert_eq!(job_id(&pending), id);
    f.harness.command("flush_publication").await;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let replay = f.stream_post("closure-original").await;
            if replay.status() == StatusCode::OK {
                assert_eq!(replay.headers()["content-type"], "application/json");
                assert_eq!(
                    serde_json::from_slice::<Value>(&collect(replay.into_body()).await).unwrap()
                        ["choices"][0]["message"]["content"],
                    "hello"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.harness.backend_calls(), 1);
    assert_eq!(f.harness.status().await["publications"], 2);
    drop(body);
    f.stop().await;
}

#[tokio::test]
async fn proxy_sse_disconnected_and_backpressured_observers_join_without_new_execute() {
    for mode in ["disconnect", "shutdown", "cancel"] {
        let mut f = Fixture::start().await;
        f.harness
            .stream_backend
            .paused
            .store(true, Ordering::SeqCst);
        f.harness.stream_backend.pieces.store(128, Ordering::SeqCst);
        let response = f.stream_post("cancel-original").await;
        let id = job_id(&response);
        let mut body = response.into_body().into_data_stream();
        tokio::time::timeout(Duration::from_secs(10), body.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if mode == "disconnect" {
            drop(body);
            f.harness.stream_backend.release.add_permits(1);
        } else {
            // Stop polling. The gateway's eight-event channel fills; shutdown
            // must cancel and join without relying on the reader to drain it.
            f.harness.stream_backend.release.add_permits(1);
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(f.runtime.streams.available_permits(), 1);
            if mode == "shutdown" {
                f.stopped.send_replace(true);
                tokio::time::timeout(Duration::from_secs(15), async {
                    while f.runtime.running.load(Ordering::Acquire) {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
            } else {
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
                // Cancellation must release owned execution while the HTTP
                // observer remains alive and backpressured.
                tokio::time::timeout(Duration::from_secs(15), async {
                    while !f.runtime.active.lock().unwrap().is_empty() {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                })
                .await
                .unwrap();
            }
            drop(body);
        }
        tokio::time::timeout(Duration::from_secs(15), async {
            while !f.runtime.active.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(f.runtime.streams.available_permits(), 2);
        assert_eq!(f.harness.buyer.active_sessions(), 0);
        let replay = f.stream_post("cancel-original").await;
        assert_eq!(replay.status(), StatusCode::ACCEPTED);
        assert_eq!(job_id(&replay), id);
        assert_eq!(f.harness.backend_calls(), 1);
        assert_eq!(f.harness.status().await["publications"], 1);
        assert_eq!(
            f.state
                .access_control
                .pending_key_budgets(None, 64)
                .unwrap()
                .len(),
            1
        );
        f.stop().await;
    }
}

#[tokio::test]
async fn proxy_sse_completed_unread_bodies_retain_bounded_observer_slots() {
    let f = Fixture::start().await;
    let first = f.stream_post("unread-first").await;
    let first_id = job_id(&first);
    wait_completed(&f, &first_id).await;
    assert_eq!(f.runtime.streams.available_permits(), 1);
    let second = f.stream_post("unread-second").await;
    let second_id = job_id(&second);
    wait_completed(&f, &second_id).await;
    assert_eq!(f.runtime.streams.available_permits(), 0);
    let refused = f.stream_post("observer-cap").await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(refused.headers().get("x-mayhem-job-id").is_none());
    assert_eq!(f.harness.backend_calls(), 2);
    drop(first);
    assert_eq!(f.runtime.streams.available_permits(), 1);
    let admitted = f.stream_post("observer-cap").await;
    assert_eq!(admitted.status(), StatusCode::OK);
    let bytes = collect(admitted.into_body()).await;
    assert!(std::str::from_utf8(&bytes).unwrap().contains("[DONE]"));
    drop(second);
    assert_eq!(f.runtime.streams.available_permits(), 2);
    assert_eq!(f.harness.backend_calls(), 3);
    f.stop().await;
}
async fn wait_completed(f: &Fixture, id: &str) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let status = f
                .state
                .jobs
                .lock()
                .unwrap()
                .get(id, now_secs())
                .unwrap()
                .unwrap()
                .status;
            if status == GatewayJobStatus::Completed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn proxy_sse_large_valid_single_delta_respects_declared_response_and_output_limits() {
    let mut f = Fixture::start().await;
    const BYTES: usize = 96 * 1024;
    f.harness
        .stream_backend
        .chunk_bytes
        .store(BYTES, Ordering::SeqCst);
    let mut body = f.stream_body();
    body["proxy"]["output_units"] = json!(BYTES / 4);
    let response = f.stream_post_body("large-event", body.clone()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let id = job_id(&response);
    let bytes = collect(response.into_body()).await;
    let events = payloads(&bytes);
    assert_eq!(
        events[0]["choices"][0]["delta"]["content"]
            .as_str()
            .unwrap()
            .len(),
        BYTES
    );
    assert!(std::str::from_utf8(&bytes)
        .unwrap()
        .ends_with("data: [DONE]\n\n"));
    let job = f
        .state
        .jobs
        .lock()
        .unwrap()
        .get(&id, now_secs())
        .unwrap()
        .unwrap();
    assert_eq!(job.status, GatewayJobStatus::Completed);
    let terms = job.proxy.as_ref().unwrap().terms().unwrap();
    assert_eq!(terms.max_usage["output_token"], (BYTES / 4) as u64);
    assert_eq!(
        job.result.as_ref().unwrap()["choices"][0]["message"]["content"]
            .as_str()
            .unwrap()
            .len(),
        BYTES
    );
    let (status, _, replay) = f
        .post("large-event", body, "owner-fixture-key", false)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(Some(replay), job.result);
    assert_eq!(f.harness.backend_calls(), 1);
    assert_eq!(f.harness.status().await["publications"], 2);
    f.stop().await;
}
