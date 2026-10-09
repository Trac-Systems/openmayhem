use super::*;
use mayhem_proxy::{
    capacity::{self, probes::Budget},
    execution::probes,
    health,
    supervisor::RefreshPolicy,
};

fn setup(f: &Fixture) -> (Arc<capacity::Authority>, health::Monitor) {
    let a = Arc::new(
        capacity::Authority::open(
            f._store.path().join("probe-capacity"),
            Identity {
                network_id: "918".into(),
                msb_bootstrap: d(1),
                subnet_bootstrap: d(2),
                controller_pubkey: d(3),
            },
            capacity::Limits {
                max_groups: 4,
                max_routes: 8,
                max_leases: 8,
                max_evidence_age: Duration::from_secs(60),
            },
        )
        .unwrap(),
    );
    a.configure_group(d(10), 2).unwrap();
    a.configure_allocation_group(d(11), 2).unwrap();
    a.configure_route_with_constraints(
        capacity::Route {
            id: d(20),
            group: d(10),
            lane: capacity::Lane::Proxy,
            max_concurrency: 2,
        },
        vec![d(11)],
    )
    .unwrap();
    a.configure_probe_budget(
        &d(10),
        Budget {
            max_attempts: 3,
            max_cost_microusd: 30,
            per_attempt_cost_microusd: 10,
        },
    )
    .unwrap();
    let m = health::Monitor::new(
        health::Policy {
            max_routes: 8,
            max_classes_per_route: 8,
            evidence_ttl_ms: 60_000,
            successes_to_increase: 2,
            bad_samples_to_reduce: 2,
            latency_baseline_samples: 3,
            latency_multiplier: 4,
            latency_increase_ms: 1000,
            min_native_tok_s: 5,
            recovery: RefreshPolicy {
                interval_ms: 10_000,
                page_pause_ms: 1,
                retry_initial_ms: 10,
                retry_max_ms: 100,
                jitter_percent: 0,
            },
        },
        2,
        1,
    )
    .unwrap();
    m.register(d(20), 2, f.adapter.endpoint() != ProxyEndpoint::Decisions)
        .unwrap();
    a.bind_live(capacity::Scope::Group(d(10)), m.connection_source())
        .unwrap();
    a.bind_live(
        capacity::Scope::Route(d(20)),
        m.route_source(&d(20)).unwrap(),
    )
    .unwrap();
    (a, m)
}
fn controller(
    f: &Fixture,
    a: Arc<capacity::Authority>,
    m: health::Monitor,
    body: &[u8],
    streaming: bool,
    timeout: Duration,
) -> Result<probes::Controller, probes::ProbeError> {
    let pool = Arc::new(
        Pool::new(
            env!("CARGO_BIN_EXE_mayhem-proxy-worker"),
            f._work.path(),
            PoolLimits {
                max_children: 2,
                max_buffer_bytes: 64 * 1024 * 1024,
                startup_timeout: Duration::from_secs(5),
                processing_timeout: Duration::from_secs(3),
            },
        )
        .unwrap(),
    );
    probes::Controller::new(
        f.connection.clone(),
        f.adapter.clone(),
        pool,
        a,
        m,
        d(20),
        d(10),
        body,
        streaming,
        probes::Limits {
            max_request_bytes: 16 * 1024,
            max_response_bytes: 32 * 1024,
            max_output_tokens: 128,
            timeout,
            storage_workers: 2,
        },
    )
}
fn bounded(mut body: Value, endpoint: ProxyEndpoint) -> Vec<u8> {
    if endpoint != ProxyEndpoint::Decisions {
        body[if endpoint == ProxyEndpoint::Responses {
            "max_output_tokens"
        } else {
            "max_tokens"
        }] = json!(64);
    }
    serde_json::to_vec(&body).unwrap()
}
fn probe_counts(a: &capacity::Authority, expected: u32) {
    for group in [10, 11] {
        assert_eq!(a.group_status(&d(group)).unwrap().occupied, expected);
    }
    assert_eq!(a.status(&d(20)).unwrap().route_occupied, expected);
}

pub(super) fn vllm_refusal(prefill: bool) -> Value {
    json!({"error":{"message":if prefill {
        "The engine has reached its prefill token backlog limit. Please try again later or on a different instance."
    } else {
        "The engine is currently busy and cannot accept new requests. Please try again later or on a different instance."
    },"type":"Service Unavailable","param":null,"code":503}})
}

#[tokio::test]
async fn audited_refusal_releases_probe_allocation_then_recovers_on_json_and_sse() {
    for streaming in [false, true] {
        for endpoint in [ProxyEndpoint::Chat, ProxyEndpoint::Completions] {
            let mut body = if endpoint == ProxyEndpoint::Chat {
                serde_json::from_slice(&chat()).unwrap()
            } else {
                json!({"model":"public-model","prompt":"hello"})
            };
            if streaming {
                body["stream"] = json!(true);
            }
            let reply = if endpoint == ProxyEndpoint::Chat {
                answer()
            } else {
                json!({"id":"u","choices":[{"index":0,"text":"hello","finish_reason":"stop"}]})
            };
            let output = if streaming {
                if endpoint == ProxyEndpoint::Chat {
                    sse(
                        &[
                            json!({"id":"u","choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":"stop"}]}),
                        ],
                        true,
                    )
                } else {
                    sse(&[reply], true)
                }
            } else {
                serde_json::to_vec(&reply).unwrap()
            };
            let b = backend_sequence(
                vec![
                    (
                        503,
                        serde_json::to_vec(&vllm_refusal(streaming)).unwrap(),
                        "application/json",
                    ),
                    (
                        200,
                        output,
                        if streaming {
                            "text/event-stream"
                        } else {
                            "application/json"
                        },
                    ),
                ],
                Duration::ZERO,
            )
            .await;
            let f =
                Fixture::with_profile(&b.base, endpoint, 128 * 1024 * 1024, "vllm_admission_v1");
            let (a, m) = setup(&f);
            m.register(d(21), 2, true).unwrap();
            m.observe_request(
                &d(21),
                health::Class::new(16, health::Thinking::Disabled, false),
            )
            .unwrap()
            .success(None);
            assert_eq!(m.snapshot(&d(21)).unwrap().state, health::State::Ready);
            let c = controller(
                &f,
                a.clone(),
                m.clone(),
                &bounded(body, endpoint),
                streaming,
                Duration::from_secs(5),
            )
            .unwrap();
            assert!(
                matches!(c.run().await, Err(probes::ProbeError::Execution(Error::Upstream(e))) if e.execution == Execution::Rejected)
            );
            assert_eq!(b.calls.load(Ordering::SeqCst), 1);
            probe_counts(&a, 0);
            assert!(a.probe_for_group(&d(10)).unwrap().is_none());
            assert_eq!(m.snapshot(&d(20)).unwrap().state, health::State::Busy);
            assert_eq!(m.snapshot(&d(21)).unwrap().state, health::State::Busy);
            assert_eq!(m.snapshot(&d(20)).unwrap().allowance, 0);
            let budget = a.probe_budget(&d(10)).unwrap().unwrap();
            assert_eq!(budget.used_attempts, 1);
            assert_eq!(budget.allocated_cost_microusd, 10);
            assert!(budget.last_completed.is_some());
            // A refused probe is completed, but never an automatic Ready/retry.
            tokio::time::sleep(Duration::from_millis(20)).await;
            c.run().await.unwrap();
            assert_eq!(b.calls.load(Ordering::SeqCst), 2);
            probe_counts(&a, 0);
            assert_eq!(m.snapshot(&d(20)).unwrap().allowance, 1);
            assert!(!m.snapshot(&d(20)).unwrap().meets_native_floor(5));
            assert_eq!(a.probe_budget(&d(10)).unwrap().unwrap().used_attempts, 2);
            assert!(f
                .journal
                .recovery_page(None, 64)
                .unwrap()
                .records
                .is_empty());
        }
    }
}

#[tokio::test]
async fn refusal_profile_is_explicit_and_cannot_clear_ambiguous_or_batched_probe_work() {
    let mut partial = vllm_refusal(false);
    partial["id"] = json!("already-started");
    for (profile, endpoint, body, reply, content_type) in [
        (
            "open_ai",
            ProxyEndpoint::Chat,
            serde_json::from_slice(&chat()).unwrap(),
            vllm_refusal(false),
            "application/json",
        ),
        (
            "vllm_admission_v1",
            ProxyEndpoint::Completions,
            json!({"model":"public-model","prompt":["a","b"]}),
            vllm_refusal(false),
            "application/json",
        ),
        (
            "vllm_admission_v1",
            ProxyEndpoint::Chat,
            json!({"model":"public-model","messages":[{"role":"user","content":"hi"}],"n":2}),
            vllm_refusal(false),
            "application/json",
        ),
        (
            "vllm_admission_v1",
            ProxyEndpoint::Chat,
            serde_json::from_slice(&chat()).unwrap(),
            partial,
            "application/json",
        ),
        (
            "vllm_admission_v1",
            ProxyEndpoint::Chat,
            serde_json::from_slice(&chat()).unwrap(),
            vllm_refusal(false),
            "text/html",
        ),
    ] {
        let b = backend_raw(
            503,
            serde_json::to_vec(&reply).unwrap(),
            content_type,
            Duration::ZERO,
        )
        .await;
        let f = Fixture::with_profile(&b.base, endpoint, 128 * 1024 * 1024, profile);
        let (a, m) = setup(&f);
        let invalid_batch = (endpoint == ProxyEndpoint::Completions && body["prompt"].is_array())
            || body["n"].as_u64() == Some(2);
        let c = controller(
            &f,
            a.clone(),
            m,
            &bounded(body, endpoint),
            false,
            Duration::from_secs(5),
        );
        if invalid_batch {
            assert!(
                matches!(c, Err(probes::ProbeError::Execution(Error::Endpoint(mayhem_proxy::endpoint::Error::Request(e)))) if e.execution == Execution::NotDispatched)
            );
            probe_counts(&a, 0);
            assert_eq!(b.calls.load(Ordering::SeqCst), 0);
            continue;
        }
        let c = c.unwrap();
        assert!(
            matches!(c.run().await, Err(probes::ProbeError::Execution(Error::Upstream(e))) if e.execution == Execution::Unknown)
        );
        probe_counts(&a, 1);
        assert_eq!(b.calls.load(Ordering::SeqCst), 1);
        assert!(a
            .probe_budget(&d(10))
            .unwrap()
            .unwrap()
            .last_completed
            .is_none());
    }
}

#[tokio::test]
async fn upstream_refusal_observation_does_not_mint_customer_waivers_or_resubmit_paid_work() {
    let b = backend(503, vllm_refusal(false), Duration::ZERO).await;
    let f = Fixture::with_profile(
        &b.base,
        ProxyEndpoint::Chat,
        128 * 1024 * 1024,
        "vllm_admission_v1",
    );
    for (i, rail) in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap]
        .into_iter()
        .enumerate()
    {
        let r = f.prepare(800 + i as u64, &chat(), rail);
        assert!(
            matches!(f.executor.execute_json(&r.invocation, &chat(), &Cancellation::default()).await,
            Err(Error::Upstream(e)) if e.execution == Execution::Rejected)
        );
        let saved = f.journal.recover(&r.invocation, r.attempt).unwrap();
        assert_eq!(saved.record.phase, Phase::Resolved);
        assert_eq!(
            saved.record.last_failure.unwrap().execution,
            Execution::Rejected
        );
        assert!(
            matches!(
                saved.record.resolution,
                Some(attempts::Resolution::NotExecuted { .. })
            ) && saved.result.is_none()
        );
        assert!(f
            .journal
            .waiver(&r.invocation, r.attempt)
            .unwrap()
            .is_none());
        assert!(f
            .executor
            .execute_json(&r.invocation, &chat(), &Cancellation::default())
            .await
            .is_err());
    }
    assert_eq!(b.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn real_json_probes_verify_all_endpoints_without_customer_finance_or_invented_speed() {
    for (endpoint, body, response) in [
        (
            ProxyEndpoint::Chat,
            serde_json::from_slice(&chat()).unwrap(),
            answer(),
        ),
        (
            ProxyEndpoint::Completions,
            json!({"model":"public-model","prompt":"hi"}),
            json!({"id":"u","choices":[{"index":0,"text":"hello","finish_reason":"stop"}]}),
        ),
        (
            ProxyEndpoint::Responses,
            json!({"model":"public-model","input":"hi"}),
            json!({"id":"u","status":"completed","output":[{"id":"i","type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}]}),
        ),
        (
            ProxyEndpoint::Decisions,
            json!({"model":"public-model","state":"hi","questions":{"q":{"type":"noul","instructions":"hello?"}}}),
            json!({"id":"u","answers":{"q":{"type":"noul","noul":0.7}}}),
        ),
    ] {
        let backend = backend(200, response, Duration::ZERO).await;
        let f = Fixture::new(&backend.base, endpoint);
        let (a, m) = setup(&f);
        let body = bounded(body, endpoint);
        let c = controller(
            &f,
            a.clone(),
            m.clone(),
            &body,
            false,
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(
            a.status(&d(20)).unwrap().state,
            capacity::Readiness::Checking
        );
        let result = c.run().await.unwrap();
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.bodies.lock().unwrap()[0]["model"], "upstream-model");
        probe_counts(&a, 0);
        assert_eq!(a.status(&d(20)).unwrap().available, 1);
        assert!(!m.snapshot(&d(20)).unwrap().meets_native_floor(5));
        assert!(a.probe_for_group(&d(10)).unwrap().is_none());
        assert_eq!(
            a.probe_budget(&d(10))
                .unwrap()
                .unwrap()
                .last_completed
                .unwrap()
                .evidence,
            result.evidence
        );
        assert!(f
            .journal
            .recovery_page(None, 64)
            .unwrap()
            .records
            .is_empty());
        assert!(f.journal.get(&result.probe).unwrap().is_none());
        assert!(matches!(
            c.run().await,
            Err(probes::ProbeError::Health(health::Error::RecoveryBusy))
        ));
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn real_streaming_probes_share_tool_schema_and_terminal_validation_on_all_llm_endpoints() {
    for (endpoint, body, answer) in [
        (
            ProxyEndpoint::Chat,
            serde_json::from_slice(&stream_request()).unwrap(),
            sse(&[delta("hi", json!(null)), delta("", json!("stop"))], true),
        ),
        (
            ProxyEndpoint::Completions,
            json!({"model":"public-model","prompt":"hi","stream":true}),
            sse(
                &[json!({"id":"c","choices":[{"index":0,"text":"hello","finish_reason":"stop"}]})],
                true,
            ),
        ),
        (
            ProxyEndpoint::Responses,
            serde_json::from_slice(&response_fixture::body()).unwrap(),
            sse(&response_fixture::flow(r#"{"city":"Paris"}"#), false),
        ),
    ] {
        let backend = backend_raw(200, answer, "text/event-stream", Duration::ZERO).await;
        let f = Fixture::new(&backend.base, endpoint);
        let (a, m) = setup(&f);
        let c = controller(
            &f,
            a.clone(),
            m.clone(),
            &bounded(body, endpoint),
            true,
            Duration::from_secs(5),
        )
        .unwrap();
        c.run().await.unwrap();
        probe_counts(&a, 0);
        assert_eq!(m.snapshot(&d(20)).unwrap().allowance, 1);
        assert!(!m.snapshot(&d(20)).unwrap().meets_native_floor(5));
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        assert!(f
            .journal
            .recovery_page(None, 64)
            .unwrap()
            .records
            .is_empty());
    }
}

#[tokio::test]
async fn probe_timeout_retains_unknown_dispatch_and_cannot_send_again_after_backoff_or_restart() {
    let backend = backend(200, answer(), Duration::from_secs(2)).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let (a, m) = setup(&f);
    let bytes = bounded(
        serde_json::from_slice(&chat()).unwrap(),
        ProxyEndpoint::Chat,
    );
    let c = controller(
        &f,
        a.clone(),
        m.clone(),
        &bytes,
        false,
        Duration::from_millis(100),
    )
    .unwrap();
    assert!(
        matches!(c.run().await, Err(probes::ProbeError::Execution(Error::Upstream(f))) if f.code == Code::UpstreamTimeout)
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    probe_counts(&a, 1);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(matches!(
        c.run().await,
        Err(probes::ProbeError::Execution(Error::Capacity(
            capacity::Error::InUse
        )))
    ));
    drop(c);
    drop(a);
    let (a, m) = setup(&f);
    assert_eq!(
        a.probe_for_group(&d(10)).unwrap().unwrap().phase,
        capacity::probes::ProbePhase::Uncertain
    );
    let c = controller(&f, a.clone(), m, &bytes, false, Duration::from_secs(5)).unwrap();
    assert!(matches!(
        c.run().await,
        Err(probes::ProbeError::Execution(Error::Capacity(
            capacity::Error::InUse
        )))
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert_eq!(a.probe_budget(&d(10)).unwrap().unwrap().used_attempts, 1);
}

#[tokio::test]
async fn http_success_with_invalid_output_is_not_recovery_and_ambiguous_refusal_does_not_free_work()
{
    for (status, response) in [
        (200, json!({"not":"an answer"})),
        (
            429,
            json!({"error":{"code":"rate_limit_exceeded","message":"private"}}),
        ),
    ] {
        let backend = backend(status, response, Duration::ZERO).await;
        let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let (a, m) = setup(&f);
        let bytes = bounded(
            serde_json::from_slice(&chat()).unwrap(),
            ProxyEndpoint::Chat,
        );
        let c = controller(
            &f,
            a.clone(),
            m.clone(),
            &bytes,
            false,
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(c.run().await.is_err());
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        probe_counts(&a, 1);
        assert_eq!(m.snapshot(&d(20)).unwrap().allowance, 0);
        assert!(a
            .probe_budget(&d(10))
            .unwrap()
            .unwrap()
            .last_completed
            .is_none());
        assert!(f
            .journal
            .recovery_page(None, 64)
            .unwrap()
            .records
            .is_empty());
    }
}

#[tokio::test]
async fn concurrent_demand_coalesces_to_one_operator_probe_and_missing_budget_never_sends() {
    let backend = backend(200, answer(), Duration::from_millis(100)).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let (a, m) = setup(&f);
    let bytes = bounded(
        serde_json::from_slice(&chat()).unwrap(),
        ProxyEndpoint::Chat,
    );
    let c = controller(
        &f,
        a.clone(),
        m.clone(),
        &bytes,
        false,
        Duration::from_secs(5),
    )
    .unwrap();
    a.configure_probe_budget(
        &d(10),
        Budget {
            max_attempts: 0,
            max_cost_microusd: 0,
            per_attempt_cost_microusd: 10,
        },
    )
    .unwrap();
    assert!(matches!(
        c.run().await,
        Err(probes::ProbeError::Execution(Error::Capacity(
            capacity::Error::ProbeBudget
        )))
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    a.configure_probe_budget(
        &d(10),
        Budget {
            max_attempts: 3,
            max_cost_microusd: 30,
            per_attempt_cost_microusd: 10,
        },
    )
    .unwrap();
    let (one, two) = tokio::join!(c.run(), c.run());
    assert!(one.is_ok() ^ two.is_ok());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert_eq!(a.probe_budget(&d(10)).unwrap().unwrap().used_attempts, 1);
    assert!(controller(&f, a, m, &chat(), false, Duration::from_secs(5)).is_err());
}

#[tokio::test]
async fn vllm_error_inside_a_started_stream_is_not_http_admission_evidence() {
    let bytes = sse(&[vllm_refusal(false)], true);
    let b = backend_raw(200, bytes, "text/event-stream", Duration::ZERO).await;
    let f = Fixture::with_profile(
        &b.base,
        ProxyEndpoint::Chat,
        128 * 1024 * 1024,
        "vllm_admission_v1",
    );
    let (a, m) = setup(&f);
    let mut body: Value = serde_json::from_slice(&chat()).unwrap();
    body["stream"] = json!(true);
    let c = controller(
        &f,
        a.clone(),
        m,
        &bounded(body, ProxyEndpoint::Chat),
        true,
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(
        matches!(c.run().await, Err(probes::ProbeError::Execution(Error::Decoder(mayhem_proxy::worker::Error::Upstream(e)))) if e.execution == Execution::Unknown)
    );
    probe_counts(&a, 1);
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
}
