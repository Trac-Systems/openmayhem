use super::*;
use mayhem_proxy::{
    health::{Monitor, Policy, Reason, State},
    supervisor::RefreshPolicy,
};

fn observe(mut fixture: Fixture) -> (Fixture, Monitor) {
    let monitor = Monitor::new(
        Policy {
            max_routes: 2,
            max_classes_per_route: 8,
            evidence_ttl_ms: 60_000,
            successes_to_increase: 3,
            bad_samples_to_reduce: 2,
            latency_baseline_samples: 3,
            latency_multiplier: 4,
            latency_increase_ms: 1000,
            min_native_tok_s: 5,
            recovery: RefreshPolicy {
                interval_ms: 10_000,
                page_pause_ms: 10,
                retry_initial_ms: 2000,
                retry_max_ms: 30_000,
                jitter_percent: 0,
            },
        },
        4,
        1,
    )
    .unwrap();
    monitor
        .register(
            d(90),
            4,
            fixture.adapter.endpoint() != ProxyEndpoint::Decisions,
        )
        .unwrap();
    fixture.executor = fixture
        .executor
        .with_observations(monitor.clone(), d(90))
        .unwrap();
    (fixture, monitor)
}

#[tokio::test]
async fn health_observes_validated_json_for_every_endpoint_without_extra_requests_or_native_rate_claims(
) {
    for (endpoint, request, response) in [
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
        let (fixture, monitor) = observe(Fixture::new(&backend.base, endpoint));
        let body = serde_json::to_vec(&request).unwrap();
        let record = fixture.prepare(1, &body, ProxyRail::Fiat);
        fixture
            .executor
            .execute_json(&record.invocation, &body, &Cancellation::default())
            .await
            .unwrap();
        for _ in 0..20 {
            let view = monitor.snapshot(&d(90)).unwrap();
            assert_eq!(view.state, State::Ready);
            assert_eq!(view.allowance, 1);
            assert!(view.native_speed.is_none());
            assert!(!view.meets_native_floor(5));
            let measurement = view.last_measurement.unwrap();
            assert!(measurement.headers_ms.is_some());
            assert!(measurement.first_output_ms.is_none());
            assert!(measurement.native_tok_s.is_none());
        }
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        let retained = fixture.journal.get(&record.invocation).unwrap().unwrap();
        assert_eq!(retained.phase, Phase::Dispatched);
        assert!(retained.closure.is_none());
    }
}

#[tokio::test]
async fn health_observes_stream_output_but_never_uses_reported_usage_as_generation_speed() {
    let chunks = [
        delta("Hello", Value::Null),
        delta(" again", Value::Null),
        delta("", json!("stop")),
        json!({"id":"stream_1","choices":[],"usage":{"prompt_tokens":2,"completion_tokens":9000,"total_tokens":9002}}),
    ];
    let backend = backend_raw(200, sse(&chunks, true), "text/event-stream", Duration::ZERO).await;
    let (fixture, monitor) = observe(Fixture::new(&backend.base, ProxyEndpoint::Chat));
    let body = stream_request();
    let record = fixture.prepare_kind(1, &body, ProxyRail::Tnk, true);
    let mut received = Vec::new();
    let reply = fixture
        .executor
        .execute_stream(&record.invocation, &body, &Cancellation::default(), |v| {
            received.push(v);
            async { Ok(()) }
        })
        .await
        .unwrap();
    assert_eq!(
        reply.reply.body["choices"][0]["message"]["content"],
        "Hello again"
    );
    assert!(!received.is_empty());
    let view = monitor.snapshot(&d(90)).unwrap();
    assert_eq!(view.state, State::Ready);
    assert!(view.native_speed.is_none());
    let measurement = view.last_measurement.unwrap();
    assert!(measurement.first_output_ms.is_some());
    assert_eq!(measurement.meaningful_updates, 2);
    assert_eq!(measurement.reported_output_tokens, Some(9000));
    assert!(measurement.native_tok_s.is_none());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn health_real_upstream_faults_withdraw_admissions_without_retry_refund_or_release() {
    for (status, response, reason) in [
        (
            401,
            json!({"error":{"code":"invalid_api_key"}}),
            Reason::Authentication,
        ),
        (
            429,
            json!({"error":{"code":"rate_limit_exceeded"}}),
            Reason::RateLimited,
        ),
        (
            200,
            json!({"error":{"code":"insufficient_quota"}}),
            Reason::PaymentRequired,
        ),
        (
            200,
            json!({"choices":[{"index":0,"message":{"role":"assistant","content":"incomplete"},"finish_reason":null}]}),
            Reason::InvalidResponse,
        ),
    ] {
        let backend = backend(status, response, Duration::ZERO).await;
        let (fixture, monitor) = observe(Fixture::new(&backend.base, ProxyEndpoint::Chat));
        let body = chat();
        let record = fixture.prepare(1, &body, ProxyRail::Tap);
        assert!(fixture
            .executor
            .execute_json(&record.invocation, &body, &Cancellation::default())
            .await
            .is_err());
        let view = monitor.snapshot(&d(90)).unwrap();
        assert_eq!(view.allowance, 0);
        assert_eq!(view.reason, reason);
        assert!(view.recovery_after_ms > 0);
        let current = fixture.journal.get(&record.invocation).unwrap().unwrap();
        assert_eq!(current.phase, Phase::Dispatched);
        assert!(current.closure.is_none() && current.resolution.is_none());
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn health_consumer_cancel_and_invalid_request_do_not_poison_backend() {
    let backend = backend_raw(
        200,
        sse(
            &[delta("hello", Value::Null), delta("", json!("stop"))],
            true,
        ),
        "text/event-stream",
        Duration::ZERO,
    )
    .await;
    let (fixture, monitor) = observe(Fixture::new(&backend.base, ProxyEndpoint::Chat));
    let body = stream_request();
    let record = fixture.prepare_kind(1, &body, ProxyRail::Tap, true);
    assert!(fixture
        .executor
        .execute_json(&record.invocation, b"{}", &Cancellation::default())
        .await
        .is_err());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    let result = fixture
        .executor
        .execute_stream(
            &record.invocation,
            &body,
            &Cancellation::default(),
            |_| async { Err(()) },
        )
        .await;
    assert!(matches!(result, Err(Error::Cancelled)));
    let view = monitor.snapshot(&d(90)).unwrap();
    assert_eq!(view.state, State::Checking);
    assert_eq!(view.reason, Reason::NoEvidence);
    assert_eq!(view.recovery_after_ms, 0);
    assert!(view.last_measurement.is_none());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn health_malformed_stream_frames_are_backend_faults_not_local_worker_failures() {
    for case in 0..3 {
        let mut chunks = vec![delta("partial", Value::Null), delta("", json!("stop"))];
        match case {
            0 => chunks[1]["id"] = json!("changed_response"),
            1 => chunks[1]["choices"][0]["finish_reason"] = Value::Null,
            _ => (),
        }
        let backend = backend_raw(
            200,
            sse(&chunks, case != 2),
            "text/event-stream",
            Duration::ZERO,
        )
        .await;
        let (fixture, monitor) = observe(Fixture::new(&backend.base, ProxyEndpoint::Chat));
        let body = stream_request();
        let record = fixture.prepare_kind(1, &body, ProxyRail::Tnk, true);
        let result = fixture
            .executor
            .execute_stream(
                &record.invocation,
                &body,
                &Cancellation::default(),
                |_| async { Ok(()) },
            )
            .await;
        assert!(result.is_err());
        let view = monitor.snapshot(&d(90)).unwrap();
        assert_eq!(view.reason, Reason::InvalidResponse, "case {case}");
        assert_eq!(view.allowance, 0);
        let record = fixture.journal.get(&record.invocation).unwrap().unwrap();
        assert!(record.resolution.is_none() && record.closure.is_none());
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    }
}
