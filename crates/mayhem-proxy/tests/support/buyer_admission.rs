use super::*;

async fn published(peer: &mut Peer) {
    for _ in 0..100 {
        if peer.command("status").await["submissions"]
            .as_u64()
            .unwrap()
            > 0
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("buyer did not publish its retained purchase");
}

async fn delayed(
    rail: ProxyRail,
    streaming: bool,
    delay: Duration,
    pending: bool,
    delayed_status: bool,
) {
    let bytes = if streaming { stream_request() } else { chat() };
    let response = if streaming {
        sse(
            &[delta("hello", json!(null)), delta(" world", json!("stop"))],
            true,
        )
    } else {
        serde_json::to_vec(&answer()).unwrap()
    };
    let backend = backend_raw(
        200,
        response,
        if streaming {
            "text/event-stream"
        } else {
            "application/json"
        },
        Duration::ZERO,
    )
    .await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(rail, &f, &bytes, streaming, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    // An accepted idle provider closes in two seconds without Status. Both
    // short all-rail cases and the real ten-second canonical ACK window exceed it.
    let (provider, _) = observed_server_with_wait(
        &s,
        &f,
        &peer,
        Duration::from_secs(if delayed_status { 4 } else { 2 }),
    );
    let ctx = context(&peer);
    let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
    if delayed_status {
        *bridge.attack.lock().await = Some("prepared_delay");
    }
    let (buyer, negotiation, _) =
        buyer_controller_with_wait(&s, &f, &peer, &bridge, 1, Duration::from_secs(20));
    let mut listener = listener(&bridge, &peer).await;
    let gate = Gate::allow();
    let request = make_request(&s, &f, &peer, &bytes, gate.clone());
    let id = request.identity();
    peer.command(if pending {
        "publish_pending"
    } else {
        "financial_unavailable"
    })
    .await;
    let (send, receive) = if streaming {
        let (send, receive) = buyer::stream_channel(buyer::StreamLimits {
            queued_events: 1,
            queued_bytes: 16 * 1024,
            event_bytes: 16 * 1024,
            total_events: 1024,
            total_bytes: 512 * 1024,
        })
        .unwrap();
        (Some(send), Some(receive))
    } else {
        (None, None)
    };
    let (_stop, rx) = watch::channel(false);
    let (result, provider_result, (), events) = tokio::join!(
        async {
            match send {
                Some(send) => buyer.execute_stream(request, send, rx).await,
                None => buyer.execute(request, rx).await,
            }
        },
        async {
            provider
                .accept(listener.next(Duration::from_secs(5)).await.unwrap())
                .unwrap()
                .wait()
                .await
        },
        async {
            published(&mut peer).await;
            tokio::time::sleep(delay).await;
            if delayed_status {
                for attempt in 0..100 {
                    if bridge.attack.lock().await.is_none() {
                        break;
                    }
                    assert!(attempt < 99, "Status must reach the provider");
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
            assert_eq!(
                backend.calls.load(Ordering::SeqCst),
                0,
                "Status and admission reads must never execute"
            );
            assert_eq!(gate.terms.lock().unwrap().len(), 1);
            let saved = negotiation
                .lookup(id.billing_id.clone(), id.billing_attempt)
                .await
                .unwrap()
                .unwrap();
            assert!(saved.authorization().is_some());
            assert!(!saved.confirmed());
            assert_eq!(
                peer.command("status").await["publications"],
                if pending { 0 } else { 1 }
            );
            peer.command(if pending {
                "flush_publication"
            } else {
                "financial_available"
            })
            .await;
        },
        async {
            let mut count = 0;
            if let Some(mut receive) = receive {
                while receive.recv().await.is_some() {
                    count += 1;
                }
            }
            count
        },
    );
    assert_eq!(provider_result.unwrap(), serving::End::Settled);
    let buyer::Outcome::Completed {
        identity,
        authorization,
        settlement,
        ..
    } = result.unwrap()
    else {
        panic!("same purchase must complete")
    };
    assert_eq!(identity, id);
    assert!(matches!(settlement, FinancialOutcome::Paid { .. }));
    assert_eq!(
        gate.terms.lock().unwrap().as_slice(),
        [authorization.terms.clone()]
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert_eq!(peer.command("status").await["publications"], 2);
    assert_eq!(
        peer.command("status").await["submissions"],
        2,
        "one reservation and one final receipt"
    );
    assert_eq!(events > 0, streaming);
    assert!(negotiation
        .lookup(id.billing_id, id.billing_attempt)
        .await
        .unwrap()
        .unwrap()
        .confirmed());
    buyer.shutdown().await.unwrap();
    peer.stop().await;
}

#[tokio::test]
async fn buyer_admission_keeps_original_channel_and_terms_on_every_rail_and_stream_mode() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for streaming in [false, true] {
            delayed(rail, streaming, Duration::from_secs(3), true, false).await;
        }
    }
}

#[tokio::test]
async fn buyer_admission_waits_past_periodic_canonical_ack_and_old_read_batch() {
    // trac-peer's normal updater ACK interval is ten seconds. The old twelve
    // immediate-error reads exhausted at 9.75s and discarded this same purchase.
    delayed(ProxyRail::Tnk, false, Duration::from_secs(12), true, false).await;
}

#[tokio::test]
async fn buyer_admission_rechecks_full_financial_state_after_admitted_intent() {
    delayed(ProxyRail::Tap, true, Duration::from_secs(3), false, false).await;
}

#[tokio::test]
async fn buyer_admission_finishes_pending_status_read_before_execute() {
    delayed(ProxyRail::Fiat, false, Duration::ZERO, true, true).await;
}

#[tokio::test]
async fn buyer_admission_lost_prepared_reply_retires_channel_without_execute() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let bytes = chat();
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &bytes, false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let (provider, _) = observed_server_with_wait(&s, &f, &peer, Duration::from_secs(2));
    let ctx = context(&peer);
    let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
    *bridge.attack.lock().await = Some("prepared_drop");
    let (buyer, negotiation, _) =
        buyer_controller_with_wait(&s, &f, &peer, &bridge, 1, Duration::from_secs(4));
    let mut listener = listener(&bridge, &peer).await;
    let gate = Gate::allow();
    let request = make_request(&s, &f, &peer, &bytes, gate.clone());
    let id = request.identity();
    peer.command("publish_pending").await;
    let (_stop, rx) = watch::channel(false);
    let (result, provider_result, published_at) = tokio::join!(
        buyer.execute(request, rx),
        async {
            provider
                .accept(listener.next(Duration::from_secs(5)).await.unwrap())
                .unwrap()
                .wait()
                .await
        },
        async {
            published(&mut peer).await;
            std::time::Instant::now()
        },
    );
    assert!(published_at.elapsed() < Duration::from_secs(5));
    let failure = error(result);
    assert_eq!(failure.stage, buyer::Stage::Accepted);
    assert!(matches!(
        failure.code,
        buyer::Code::Transport | buyer::Code::Unconfirmed
    ));
    assert!(failure.recovery_required);
    assert_eq!(provider_result.unwrap(), serving::End::ControlIdle);
    assert!(
        bridge.attack.lock().await.is_none(),
        "fixture must drop Prepared"
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(gate.terms.lock().unwrap().len(), 1);
    let saved = negotiation
        .lookup(id.billing_id, id.billing_attempt)
        .await
        .unwrap()
        .unwrap();
    assert!(saved.authorization().is_some());
    assert!(!saved.confirmed());
    assert_eq!(peer.command("status").await["submissions"], 1);
    buyer.shutdown().await.unwrap();
    peer.stop().await;
}

#[tokio::test]
async fn buyer_admission_cancellation_and_invalid_proof_never_send_execute() {
    for permanent in [false, true] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let bytes = chat();
        let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut peer = Peer::start(ProxyRail::Fiat, &f, &bytes, false, None).await;
        let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
        let (provider, _) = observed_server_with_wait(&s, &f, &peer, Duration::from_secs(2));
        let ctx = context(&peer);
        let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
        let (buyer, negotiation, _) =
            buyer_controller_with_wait(&s, &f, &peer, &bridge, 1, Duration::from_secs(8));
        let mut listener = listener(&bridge, &peer).await;
        let gate = Gate::allow();
        let request = make_request(&s, &f, &peer, &bytes, gate.clone());
        let id = request.identity();
        peer.command("financial_unavailable").await;
        let (stop, rx) = watch::channel(false);
        let (result, handle, changed) = tokio::join!(
            buyer.execute(request, rx),
            async {
                provider
                    .accept(listener.next(Duration::from_secs(5)).await.unwrap())
                    .unwrap()
            },
            async {
                published(&mut peer).await;
                if permanent {
                    peer.command("signature").await;
                    peer.command("financial_available").await;
                } else {
                    stop.send_replace(true);
                }
                std::time::Instant::now()
            },
        );
        assert!(changed.elapsed() < Duration::from_secs(3));
        let failure = error(result);
        assert_eq!(failure.stage, buyer::Stage::Accepted);
        assert!(failure.recovery_required);
        if permanent {
            assert_eq!(failure.code, buyer::Code::Unconfirmed);
            assert_eq!(failure.financial_code, Some("proxy_financial_invalid"));
        } else {
            assert_eq!(failure.code, buyer::Code::Stopped);
        }
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        assert_eq!(gate.terms.lock().unwrap().len(), 1);
        assert!(negotiation
            .lookup(id.billing_id, id.billing_attempt)
            .await
            .unwrap()
            .unwrap()
            .authorization()
            .is_some());
        tokio::time::timeout(Duration::from_secs(4), handle.wait())
            .await
            .unwrap()
            .unwrap();
        buyer.shutdown().await.unwrap();
        peer.stop().await;
    }
}

#[tokio::test]
async fn buyer_admission_deadline_preserves_unknown_diagnostic_without_execute() {
    for (command, diagnostic) in [
        ("financial_unknown", "proxy_financial_unknown"),
        ("financial_unavailable", "proxy_financial_unavailable"),
    ] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let bytes = chat();
        let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut peer = Peer::start(ProxyRail::Fiat, &f, &bytes, false, None).await;
        let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
        let (provider, _) = observed_server_with_wait(&s, &f, &peer, Duration::from_secs(2));
        let ctx = context(&peer);
        let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
        let (buyer, negotiation, _) =
            buyer_controller_with_wait(&s, &f, &peer, &bridge, 1, Duration::from_secs(2));
        let mut listener = listener(&bridge, &peer).await;
        let gate = Gate::allow();
        let request = make_request(&s, &f, &peer, &bytes, gate.clone());
        let id = request.identity();
        peer.command(command).await;
        let (_stop, rx) = watch::channel(false);
        let (result, handle) = tokio::join!(buyer.execute(request, rx), async {
            provider
                .accept(listener.next(Duration::from_secs(5)).await.unwrap())
                .unwrap()
        },);
        let failure = error(result);
        assert_eq!(failure.code, buyer::Code::Unconfirmed);
        assert_eq!(failure.stage, buyer::Stage::Accepted);
        assert_eq!(failure.financial_code, Some(diagnostic));
        assert!(failure.recovery_required);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        assert_eq!(gate.terms.lock().unwrap().len(), 1);
        let saved = negotiation
            .lookup(id.billing_id, id.billing_attempt)
            .await
            .unwrap()
            .unwrap();
        assert!(saved.authorization().is_some());
        assert!(!saved.confirmed());
        assert_eq!(peer.command("status").await["submissions"], 1);
        handle.wait().await.unwrap();
        buyer.shutdown().await.unwrap();
        peer.stop().await;
    }
}
