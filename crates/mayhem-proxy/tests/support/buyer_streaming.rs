use super::*;

fn channel() -> (buyer::StreamSender, buyer::StreamReceiver) {
    buyer::stream_channel(buyer::StreamLimits {
        queued_events: 1,
        queued_bytes: 16 * 1024,
        event_bytes: 16 * 1024,
        total_events: 1024,
        total_bytes: 512 * 1024,
    })
    .unwrap()
}
fn endpoints() -> Vec<(ProxyEndpoint, Vec<u8>, Vec<u8>)> {
    vec![
        (
            ProxyEndpoint::Chat,
            stream_request(),
            sse(
                &[delta("hello", json!(null)), delta(" world", json!("stop"))],
                true,
            ),
        ),
        (
            ProxyEndpoint::Completions,
            serde_json::to_vec(&json!({"model":"public-model","prompt":"hi","stream":true}))
                .unwrap(),
            sse(
                &[
                    json!({"id":"c","choices":[{"index":0,"text":"hello","finish_reason":null}]}),
                    json!({"id":"c","choices":[{"index":0,"text":" world","finish_reason":"stop"}]}),
                ],
                true,
            ),
        ),
        (
            ProxyEndpoint::Responses,
            response_fixture::body(),
            sse(&response_fixture::flow(r#"{"city":"Paris"}"#), false),
        ),
    ]
}

#[tokio::test]
async fn buyer_streaming_all_three_endpoints_and_rails_verify_original_output_before_payment() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for (endpoint, bytes, response) in endpoints() {
            let backend = backend_raw(200, response, "text/event-stream", Duration::ZERO).await;
            let f = Fixture::new(&backend.base, endpoint);
            let mut peer = Peer::start(rail, &f, &bytes, true, None).await;
            let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
            let (provider, _) = observed_server(&s, &f, &peer);
            let ctx = context(&peer);
            let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
            let (buyer, _, _) = buyer_controller(&s, &f, &peer, &bridge, 1);
            let mut listener = listener(&bridge, &peer).await;
            let gate = Gate::allow();
            let request = make_request(&s, &f, &peer, &bytes, gate.clone());
            let (send, mut receive) = channel();
            let (_stop, rx) = watch::channel(false);
            let (result, provider_result, events) = tokio::join!(
                buyer.execute_stream(request, send, rx),
                async {
                    provider
                        .accept(listener.next(Duration::from_secs(5)).await.unwrap())
                        .unwrap()
                        .wait()
                        .await
                },
                async {
                    let mut count = 0;
                    while let Some(event) = receive.recv().await {
                        let event: Value = serde_json::from_slice(event.json_bytes()).unwrap();
                        assert!(event.get("proxy").is_none());
                        assert!(event["type"]
                            .as_str()
                            .is_none_or(|s| !s.ends_with(".done") && s != "response.completed"));
                        if let Some(choices) = event["choices"].as_array() {
                            assert!(choices.iter().all(|v| v["finish_reason"].is_null()));
                        }
                        count += 1;
                    }
                    count
                }
            );
            let buyer::Outcome::Completed {
                response,
                settlement,
                ..
            } = result.unwrap()
            else {
                panic!("paid streaming completion required")
            };
            assert!(matches!(settlement, FinancialOutcome::Paid { .. }));
            assert_eq!(gate.outputs.lock().unwrap().as_slice(), [response]);
            assert!(events > 0);
            assert_eq!(provider_result.unwrap(), serving::End::Settled);
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            assert_eq!(peer.command("status").await["publications"], 2);
            buyer.shutdown().await.unwrap();
            peer.stop().await;
        }
    }
}

// Deliberately withhold the upstream terminal frames until the test observes
// provisional delivery. A buffered JSON path cannot pass this fixture.
async fn gated_backend() -> (Backend, Arc<Notify>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1/", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let bodies = Arc::new(Mutex::new(vec![]));
    let recorded = bodies.clone();
    let release = Arc::new(Notify::new());
    let wait = release.clone();
    let handle = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = vec![];
            let mut buf = [0; 1024];
            let boundary = loop {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    return;
                };
                bytes.extend_from_slice(&buf[..n]);
                assert!(bytes.len() < 1024 * 1024);
                if let Some(n) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                    break n + 4;
                }
            };
            let length = String::from_utf8_lossy(&bytes[..boundary])
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|n| n.trim().parse::<usize>().ok())
                })
                .unwrap();
            while bytes.len() < boundary + length {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    return;
                };
                bytes.extend_from_slice(&buf[..n]);
            }
            recorded
                .lock()
                .unwrap()
                .push(serde_json::from_slice(&bytes[boundary..boundary + length]).unwrap());
            count.fetch_add(1, Ordering::SeqCst);
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.unwrap();
            let prefix = sse(
                &[
                    delta("he", json!(null)),
                    delta("ll", json!(null)),
                    delta("o", json!(null)),
                    delta(" ", json!(null)),
                ],
                false,
            );
            socket
                .write_all(format!("{:x}\r\n", prefix.len()).as_bytes())
                .await
                .unwrap();
            socket.write_all(&prefix).await.unwrap();
            socket.write_all(b"\r\n").await.unwrap();
            socket.flush().await.unwrap();
            wait.notified().await;
            let tail = sse(&[delta("world", json!("stop"))], true);
            if socket
                .write_all(format!("{:x}\r\n", tail.len()).as_bytes())
                .await
                .is_err()
            {
                continue;
            }
            if socket.write_all(&tail).await.is_err() {
                continue;
            }
            let _ = socket.write_all(b"\r\n0\r\n\r\n").await;
        }
    });
    (
        Backend {
            base,
            calls,
            bodies,
            handle,
        },
        release,
    )
}

#[tokio::test]
async fn buyer_streaming_delivers_before_upstream_terminal_and_retains_before_approval() {
    let (backend, release) = gated_backend().await;
    let bytes = stream_request();
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &bytes, true, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let (provider, _) = observed_server(&s, &f, &peer);
    let ctx = context(&peer);
    let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
    let (buyer, _, recovery) = buyer_controller(&s, &f, &peer, &bridge, 1);
    let mut listener = listener(&bridge, &peer).await;
    let gate = Gate::allow();
    let request = make_request(&s, &f, &peer, &bytes, gate.clone());
    let identity = request.identity();
    let (send, mut receive) = channel();
    let (_stop, rx) = watch::channel(false);
    let active = buyer.clone();
    let task = tokio::spawn(async move { active.execute_stream(request, send, rx).await });
    let handle = provider
        .accept(listener.next(Duration::from_secs(5)).await.unwrap())
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), receive.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(!first.json_bytes().is_empty());
    drop(first);
    assert!(!task.is_finished());
    assert!(gate.outputs.lock().unwrap().is_empty());
    assert_eq!(peer.command("status").await["publications"], 1);
    let saved = buyer.retained_purchase(&identity).await.unwrap().unwrap();
    let key = Digest::new(saved.offer().terms.digest().unwrap()).unwrap();
    let before = recovery.recover(key).await.unwrap();
    assert!(!before.outcome_approved && !before.outcome_signed);
    gate.reject_output.store(true, Ordering::SeqCst);
    release.notify_one();
    while receive.recv().await.is_some() {}
    let failure = error(task.await.unwrap());
    assert!(failure.recovery_required);
    let _ = handle.wait().await;
    assert!(gate.outputs.lock().unwrap().is_empty());
    gate.reject_output.store(false, Ordering::SeqCst);
    let (_stop, rx) = watch::channel(false);
    let (result, handle) = tokio::join!(buyer.recover(identity, gate.clone(), rx), async {
        provider
            .accept(listener.next(Duration::from_secs(5)).await.unwrap())
            .unwrap()
    });
    assert!(matches!(result.unwrap(), buyer::Outcome::Completed { .. }));
    assert_eq!(handle.wait().await.unwrap(), serving::End::Settled);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert_eq!(gate.outputs.lock().unwrap().len(), 1);
    buyer.shutdown().await.unwrap();
    peer.stop().await;
}

#[tokio::test]
async fn buyer_streaming_dropped_or_stalled_consumer_cancels_and_joins_original_purchase() {
    for mode in ["cancel", "drop", "shutdown"] {
        let (backend, release) = gated_backend().await;
        let bytes = stream_request();
        let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut peer = Peer::start(ProxyRail::Tap, &f, &bytes, true, None).await;
        let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
        let (provider, _) = observed_server(&s, &f, &peer);
        let ctx = context(&peer);
        let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
        let (buyer, _, _) = buyer_controller(&s, &f, &peer, &bridge, 1);
        let mut listener = listener(&bridge, &peer).await;
        let gate = Gate::allow();
        let request = make_request(&s, &f, &peer, &bytes, gate.clone());
        let identity = request.identity();
        let (send, mut receive) = channel();
        let (stop, rx) = watch::channel(false);
        let active = buyer.clone();
        let task = tokio::spawn(async move { active.execute_stream(request, send, rx).await });
        let handle = provider
            .accept(listener.next(Duration::from_secs(5)).await.unwrap())
            .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(5), receive.recv())
            .await
            .unwrap()
            .unwrap();
        drop(first);
        if mode == "drop" {
            drop(receive);
        } else if mode == "cancel" {
            stop.send_replace(true);
        } else {
            tokio::time::timeout(Duration::from_secs(5), buyer.shutdown())
                .await
                .unwrap()
                .unwrap();
        }
        let failure = error(
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap(),
        );
        assert_eq!(failure.code, buyer::Code::Stopped);
        assert!(failure.recovery_required);
        assert!(buyer.retained_purchase(&identity).await.unwrap().is_some());
        assert!(gate.outputs.lock().unwrap().is_empty());
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        assert_eq!(peer.command("status").await["publications"], 1);
        release.notify_one();
        let _ = handle.wait().await;
        buyer.shutdown().await.unwrap();
        assert_eq!(buyer.active_sessions(), 0);
        peer.stop().await;
    }
}

#[tokio::test]
async fn buyer_streaming_authenticated_event_tampering_never_reaches_output_or_approval() {
    for attack in ["stream_content", "stream_identity"] {
        let response = sse(
            &[delta("hello", json!(null)), delta(" world", json!("stop"))],
            true,
        );
        let backend = backend_raw(200, response, "text/event-stream", Duration::ZERO).await;
        let bytes = stream_request();
        let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut peer = Peer::start(ProxyRail::Fiat, &f, &bytes, true, None).await;
        let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
        let (provider, _) = observed_server(&s, &f, &peer);
        let ctx = context(&peer);
        let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
        *bridge.attack.lock().await = Some(attack);
        let (buyer, _, recovery) = buyer_controller(&s, &f, &peer, &bridge, 1);
        let mut listener = listener(&bridge, &peer).await;
        let gate = Gate::allow();
        let request = make_request(&s, &f, &peer, &bytes, gate.clone());
        let identity = request.identity();
        let (send, mut receive) = channel();
        let (_stop, rx) = watch::channel(false);
        let (result, handle, events) = tokio::join!(
            buyer.execute_stream(request, send, rx),
            async {
                provider
                    .accept(listener.next(Duration::from_secs(5)).await.unwrap())
                    .unwrap()
            },
            async {
                let mut count = 0;
                while receive.recv().await.is_some() {
                    count += 1;
                }
                count
            }
        );
        let failure = error(result);
        assert_eq!(failure.code, buyer::Code::Verification);
        assert!(failure.recovery_required);
        if attack == "stream_content" {
            assert!(events > 0);
        } else {
            assert_eq!(events, 0);
        }
        let _ = handle.wait().await;
        let saved = buyer.retained_purchase(&identity).await.unwrap().unwrap();
        let key = Digest::new(saved.offer().terms.digest().unwrap()).unwrap();
        let status = recovery.recover(key).await.unwrap();
        assert!(!status.outcome_approved && !status.outcome_signed);
        assert!(gate.outputs.lock().unwrap().is_empty());
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        assert_eq!(peer.command("status").await["publications"], 1);
        buyer.shutdown().await.unwrap();
        peer.stop().await;
    }
}
