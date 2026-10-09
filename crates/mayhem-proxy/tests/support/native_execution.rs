use super::*;
use mayhem_proxy::health::{self, native};

pub(crate) fn source(f: &Fixture) -> Arc<native::Source> {
    let data=serde_json::to_vec(&json!({"version":"1.0","truncation":null,"padding":null,"added_tokens":[],
        "normalizer":null,"pre_tokenizer":{"type":"WhitespaceSplit"},"post_processor":null,"decoder":null,
        "model":{"type":"WordLevel","vocab":{"[UNK]":0,"one":1,"two":2,"three":3,"four":4,"five":5},"unk_token":"[UNK]"}})).unwrap();
    Arc::new(
        native::Source::from_bytes(
            &data,
            Digest::new(blake3::hash(&data).to_hex().as_str()).unwrap(),
            f.connection.fingerprint().clone(),
            f.adapter.recipe_hash().clone(),
            native::Limits {
                artifact_bytes: 1024 * 1024,
                output_bytes: 1024 * 1024,
                channels: 16,
                workers: 2,
                minimum_tokens: 2,
            },
        )
        .unwrap(),
    )
}

/// Real independent HTTP writes. Each length-delimited chunk is observable before
/// the next delay; buffering all SSE frames in one response cannot prove timing.
pub(crate) async fn paced(pieces: Vec<(Duration, Vec<u8>)>) -> Backend {
    paced_runs(vec![pieces]).await
}
async fn paced_runs(runs: Vec<Vec<(Duration, Vec<u8>)>>) -> Backend {
    assert!(!runs.is_empty());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1/", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let received = bodies.clone();
    let handle = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket.set_nodelay(true).unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0u8; 1024];
            let boundary = loop {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    break None;
                }
                bytes.extend_from_slice(&buf[..n]);
                assert!(bytes.len() < 2 * 1024 * 1024);
                if let Some(i) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                    break Some(i + 4);
                }
            };
            let Some(boundary) = boundary else { continue };
            let len = String::from_utf8_lossy(&bytes[..boundary])
                .lines()
                .find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|s| s.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            assert!(len < 2 * 1024 * 1024);
            while bytes.len() < boundary + len {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
            }
            received
                .lock()
                .unwrap()
                .push(serde_json::from_slice(&bytes[boundary..boundary + len]).unwrap());
            let index = count.fetch_add(1, Ordering::SeqCst).min(runs.len() - 1);
            if socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.is_err(){continue;}
            for (delay, piece) in &runs[index] {
                tokio::time::sleep(*delay).await;
                let header = format!("{:x}\r\n", piece.len());
                let mut frame = header.into_bytes();
                frame.extend(piece);
                frame.extend(b"\r\n");
                if socket.write_all(&frame).await.is_err() {
                    break;
                }
            }
            let _ = socket.write_all(b"0\r\n\r\n").await;
        }
    });
    Backend {
        base,
        calls,
        bodies,
        handle,
    }
}
pub(crate) fn chat_pieces(gap: Duration) -> Vec<(Duration, Vec<u8>)> {
    vec![
        (Duration::ZERO, sse(&[delta("one ", Value::Null)], false)),
        (
            gap,
            sse(
                &[
                    delta("two three four", json!("stop")),
                    json!({"id":"stream_1","choices":[],"usage":{"prompt_tokens":1,"completion_tokens":999999,"total_tokens":1000000}}),
                ],
                true,
            ),
        ),
    ]
}
fn measured(f: Fixture) -> (Fixture, health::Monitor) {
    let s = source(&f);
    let (mut f, m) = health_execution::observe(f);
    f.executor = f.executor.with_tokenizer(s).unwrap();
    (f, m)
}

#[tokio::test]
async fn native_transport_measures_each_stream_endpoint_using_local_tokens_without_extra_posts() {
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
    ] {
        let (body, pieces) = match endpoint {
            ProxyEndpoint::Chat => (stream_request(), chat_pieces(Duration::from_millis(100))),
            ProxyEndpoint::Completions => (
                serde_json::to_vec(&json!({"model":"public-model","prompt":"hi","stream":true}))
                    .unwrap(),
                vec![
                    (
                        Duration::ZERO,
                        sse(
                            &[
                                json!({"id":"u","choices":[{"index":0,"text":"one ","finish_reason":null}]}),
                            ],
                            false,
                        ),
                    ),
                    (
                        Duration::from_millis(100),
                        sse(
                            &[
                                json!({"id":"u","choices":[{"index":0,"text":"two three four","finish_reason":"stop"}]}),
                            ],
                            true,
                        ),
                    ),
                ],
            ),
            _ => {
                let events = response_fixture::text_flow("one two three four", false);
                // First four character deltas contain "one "; both groups retain
                // their internally consistent Responses sequence and final object.
                (
                    response_fixture::body(),
                    vec![
                        (Duration::ZERO, sse(&events[..7], false)),
                        (Duration::from_millis(100), sse(&events[7..], false)),
                    ],
                )
            }
        };
        let b = paced(pieces).await;
        let (f, m) = measured(Fixture::new(&b.base, endpoint));
        let r = f.prepare_kind(1, &body, ProxyRail::Fiat, true);
        f.executor
            .execute_stream(&r.invocation, &body, &Cancellation::default(), |_| async {
                Ok(())
            })
            .await
            .unwrap();
        let v = m.snapshot(&d(90)).unwrap();
        let rate = v.last_measurement.as_ref().unwrap().native_tok_s.unwrap();
        assert_eq!(
            v.last_measurement.as_ref().unwrap().native_interval_tokens,
            Some(3)
        );
        assert!((5.0..60.0).contains(&rate), "{endpoint:?}: {rate}");
        assert!(v.meets_native_floor(5));
        assert_eq!(b.calls.load(Ordering::SeqCst), 1);
        assert!(
            f.journal
                .get(&r.invocation)
                .unwrap()
                .unwrap()
                .output_may_have_been_delivered
        );
    }
}

#[tokio::test]
async fn native_transport_slow_consumer_does_not_turn_local_backpressure_into_backend_fault() {
    let b = paced(chat_pieces(Duration::from_millis(50))).await;
    let (f, m) = measured(Fixture::new(&b.base, ProxyEndpoint::Chat));
    let body = stream_request();
    let r = f.prepare_kind(1, &body, ProxyRail::Tap, true);
    let reply = f
        .executor
        .execute_stream(&r.invocation, &body, &Cancellation::default(), |_| async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        reply.reply.body["choices"][0]["message"]["content"],
        "one two three four"
    );
    let v = m.snapshot(&d(90)).unwrap();
    assert_eq!(v.state, health::State::Ready);
    assert!(v.native_speed.is_none());
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn native_transport_requires_valid_final_output_before_publishing_measurement() {
    let mut pieces = chat_pieces(Duration::from_millis(50));
    pieces[1].1 = sse(&[delta("two three four", Value::Null)], true);
    let b = paced(pieces).await;
    let (f, m) = measured(Fixture::new(&b.base, ProxyEndpoint::Chat));
    let body = stream_request();
    let r = f.prepare_kind(1, &body, ProxyRail::Tnk, true);
    assert!(f
        .executor
        .execute_stream(&r.invocation, &body, &Cancellation::default(), |_| async {
            Ok(())
        })
        .await
        .is_err());
    let v = m.snapshot(&d(90)).unwrap();
    assert!(v.native_speed.is_none());
    assert_eq!(v.reason, health::Reason::InvalidResponse);
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn native_transport_json_and_short_labels_remain_unknown_and_binding_is_enforced() {
    let b = backend(200, answer(), Duration::ZERO).await;
    let (f, m) = measured(Fixture::new(&b.base, ProxyEndpoint::Chat));
    let body = chat();
    let r = f.prepare(1, &body, ProxyRail::Fiat);
    f.executor
        .execute_json(&r.invocation, &body, &Cancellation::default())
        .await
        .unwrap();
    assert!(m.snapshot(&d(90)).unwrap().native_speed.is_none());
    let other = Fixture::new("http://127.0.0.1:1/v1/", ProxyEndpoint::Chat);
    assert!(matches!(
        other.executor.with_tokenizer(source(&f)),
        Err(Error::Configuration)
    ));
    let decisions = Fixture::new(&b.base, ProxyEndpoint::Decisions);
    let s = source(&decisions);
    assert!(matches!(
        decisions.executor.with_tokenizer(s),
        Err(Error::Configuration)
    ));
    let b = paced(vec![
        (Duration::ZERO, sse(&[delta("one ", Value::Null)], false)),
        (
            Duration::from_millis(50),
            sse(&[delta("two", json!("stop"))], true),
        ),
    ])
    .await;
    let (f, m) = measured(Fixture::new(&b.base, ProxyEndpoint::Chat));
    let body = stream_request();
    let r = f.prepare_kind(1, &body, ProxyRail::Fiat, true);
    f.executor
        .execute_stream(&r.invocation, &body, &Cancellation::default(), |_| async {
            Ok(())
        })
        .await
        .unwrap();
    assert!(m.snapshot(&d(90)).unwrap().native_speed.is_none());
}

#[tokio::test]
async fn native_operator_probe_uses_same_source_and_budget_without_customer_financial_records() {
    let b = paced(chat_pieces(Duration::from_millis(100))).await;
    let f = Fixture::new(&b.base, ProxyEndpoint::Chat);
    let (a, m) = probe_execution::setup(&f);
    let body = probe_execution::bounded(
        serde_json::from_slice(&stream_request()).unwrap(),
        ProxyEndpoint::Chat,
    );
    let c = probe_execution::controller(
        &f,
        a.clone(),
        m.clone(),
        &body,
        true,
        Duration::from_secs(5),
    )
    .unwrap()
    .with_tokenizer(source(&f))
    .unwrap();
    c.run().await.unwrap();
    assert!(m.snapshot(&d(20)).unwrap().meets_native_floor(5));
    assert_eq!(a.status(&d(20)).unwrap().route_occupied, 0);
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    assert!(c.run().await.is_err());
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn native_measured_slow_streams_withdraw_then_fresh_budgeted_inference_recovers() {
    let b = paced_runs(vec![
        chat_pieces(Duration::from_millis(900)),
        chat_pieces(Duration::from_millis(900)),
        chat_pieces(Duration::from_millis(100)),
    ])
    .await;
    let mut f = Fixture::new(&b.base, ProxyEndpoint::Chat);
    let source = source(&f);
    let (a, m) = probe_execution::setup_measured(&f, source.digest().clone());
    f.executor = f
        .executor
        .with_observations(m.clone(), d(20))
        .unwrap()
        .with_tokenizer(source.clone())
        .unwrap();
    let body = probe_execution::bounded(
        serde_json::from_slice(&stream_request()).unwrap(),
        ProxyEndpoint::Chat,
    );
    let probe = probe_execution::controller(
        &f,
        a.clone(),
        m.clone(),
        &body,
        true,
        Duration::from_secs(5),
    )
    .unwrap()
    .with_tokenizer(source)
    .unwrap();
    for n in 1..=2 {
        let r = f.prepare_kind(n, &body, ProxyRail::Fiat, true);
        f.executor
            .execute_stream(&r.invocation, &body, &Cancellation::default(), |_| async {
                Ok(())
            })
            .await
            .unwrap();
        let last = m.snapshot(&d(20)).unwrap().last_measurement.unwrap();
        assert_eq!(last.native_interval_tokens, Some(3));
        assert!(last.native_tok_s.unwrap() < 5.0);
        assert_eq!(last.reported_output_tokens, Some(999999));
    }
    let view = m.snapshot(&d(20)).unwrap();
    assert_eq!(view.reason, health::Reason::SlowGeneration);
    assert_eq!(view.allowance, 0);
    assert_eq!(a.status(&d(20)).unwrap().available, 0);
    tokio::time::sleep(Duration::from_millis(view.recovery_after_ms + 1)).await;
    // Timer alone does not restore admission or create a new customer attempt.
    assert_eq!(a.status(&d(20)).unwrap().available, 0);
    probe.run().await.unwrap();
    assert!(m.snapshot(&d(20)).unwrap().meets_native_floor(5));
    assert_eq!(a.status(&d(20)).unwrap().available, 1);
    assert_eq!(b.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn native_required_admission_recovers_from_short_probe_without_mistaking_success_for_capacity(
) {
    let b = paced_runs(vec![
        vec![(Duration::ZERO, sse(&[delta("yes", json!("stop"))], true))],
        chat_pieces(Duration::from_millis(100)),
    ])
    .await;
    let f = Fixture::new(&b.base, ProxyEndpoint::Chat);
    let source = source(&f);
    let (a, m) = probe_execution::setup_measured(&f, source.digest().clone());
    let body = probe_execution::bounded(
        serde_json::from_slice(&stream_request()).unwrap(),
        ProxyEndpoint::Chat,
    );
    let c = probe_execution::controller(
        &f,
        a.clone(),
        m.clone(),
        &body,
        true,
        Duration::from_secs(5),
    )
    .unwrap()
    .with_tokenizer(source)
    .unwrap();
    c.run().await.unwrap();
    let view = m.snapshot(&d(20)).unwrap();
    assert_eq!(view.reason, health::Reason::UnverifiedThroughput);
    assert_eq!(a.status(&d(20)).unwrap().available, 0);
    assert!(view.recovery_after_ms > 0);
    assert!(c.run().await.is_err());
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    tokio::time::sleep(Duration::from_millis(view.recovery_after_ms + 1)).await;
    c.run().await.unwrap();
    assert!(m.snapshot(&d(20)).unwrap().meets_native_floor(5));
    assert_eq!(a.status(&d(20)).unwrap().available, 1);
    assert_eq!(b.calls.load(Ordering::SeqCst), 2);
    assert!(c.run().await.is_err());
    assert_eq!(b.calls.load(Ordering::SeqCst), 2);
}
