use super::*;
use mayhem_proxy::connector::failure::{Code, Failure, Scope, Stage};

async fn refuse_once(
    bridge: &Bridge,
    peer: &Peer,
    listener: &mut n::opening::Listener,
    bytes: &[u8],
) {
    let ctx = context(peer);
    let incoming = listener.next(Duration::from_secs(5)).await.unwrap();
    assert_eq!(incoming.context().digest().unwrap(), ctx.digest().unwrap());
    let mut channel = n::Channel::connect(
        bridge.config(false),
        ctx.clone(),
        &peer.identity,
        Role::Provider,
        exchange::Limits {
            max_message_bytes: 2 * 1024 * 1024,
        },
    )
    .await
    .unwrap();
    let mut opening = mayhem_bridge::ScBridgeClient::connect(bridge.config(false))
        .await
        .unwrap();
    opening
        .session_send(
            ctx.buyer.as_str(),
            ctx.session_id.as_str(),
            json!({
                "t":"p.negotiate.ready", "schema_version":1,
                "session_id":ctx.session_id, "context_digest":ctx.digest().unwrap(),
            }),
        )
        .await
        .unwrap();
    let received = channel.receive(Duration::from_secs(5)).await.unwrap();
    let n::Message::Request { request } = received.message() else {
        panic!("request required")
    };
    assert_eq!(*request, serde_json::from_slice::<Value>(bytes).unwrap());
    let failure = Failure::new(
        Code::LocalCapacity,
        Scope::Request,
        Stage::BeforeDispatch,
        Execution::Unknown,
    );
    channel
        .send(&n::Message::Refused {
            failure: (&failure).into(),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn buyer_waits_for_unsigned_capacity_without_changing_identity_or_reexecuting() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for streaming in [false, true] {
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
            let (provider, _) = observed_server(&s, &f, &peer);
            let ctx = context(&peer);
            let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
            let (buyer, _, _) = buyer_controller(&s, &f, &peer, &bridge, 1);
            let mut listener = listener(&bridge, &peer).await;
            let gate = Gate::allow();
            let request = make_request(&s, &f, &peer, &bytes, gate.clone());
            let identity = request.identity();
            let (_stop, rx) = watch::channel(false);
            let (send, mut receive) = buyer::stream_channel(buyer::StreamLimits {
                queued_events: 8,
                queued_bytes: 128 * 1024,
                event_bytes: 16 * 1024,
                total_events: 1024,
                total_bytes: 512 * 1024,
            })
            .unwrap();
            let (result, end, ()) = tokio::join!(
                async {
                    if streaming {
                        buyer.execute_stream(request, send, rx).await
                    } else {
                        drop(send);
                        buyer.execute(request, rx).await
                    }
                },
                async {
                    for _ in 0..2 {
                        refuse_once(&bridge, &peer, &mut listener, &bytes).await;
                        assert!(gate.terms.lock().unwrap().is_empty());
                        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
                    }
                    provider
                        .accept(listener.next(Duration::from_secs(5)).await.unwrap())
                        .unwrap()
                        .wait()
                        .await
                },
                async { while receive.recv().await.is_some() {} }
            );
            assert!(matches!(result.unwrap(), buyer::Outcome::Completed {
                identity: actual, settlement: FinancialOutcome::Paid { .. }, ..
            } if actual == identity));
            assert_eq!(end.unwrap(), serving::End::Settled);
            assert_eq!(gate.terms.lock().unwrap().len(), 1);
            assert_eq!(gate.outputs.lock().unwrap().len(), 1);
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            assert_eq!(peer.command("status").await["publications"], 2);
            buyer.shutdown().await.unwrap();
            peer.stop().await;
        }
    }
}

#[tokio::test]
async fn buyer_capacity_wait_cancels_without_signing_publishing_or_dispatching() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let bytes = chat();
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &bytes, false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let ctx = context(&peer);
    let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
    let (buyer, negotiation, _) = buyer_controller(&s, &f, &peer, &bridge, 1);
    let mut listener = listener(&bridge, &peer).await;
    let gate = Gate::allow();
    let request = make_request(&s, &f, &peer, &bytes, gate.clone());
    let identity = request.identity();
    let (stop, rx) = watch::channel(false);
    let (result, ()) = tokio::join!(buyer.execute(request, rx), async {
        refuse_once(&bridge, &peer, &mut listener, &bytes).await;
        stop.send_replace(true);
    });
    let failure = error(result);
    assert_eq!(failure.code, buyer::Code::Stopped);
    assert!(!failure.recovery_required);
    assert!(gate.terms.lock().unwrap().is_empty());
    assert!(negotiation
        .lookup(identity.billing_id, identity.billing_attempt)
        .await
        .unwrap()
        .is_none());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(peer.command("status").await["publications"], 0);
    buyer.shutdown().await.unwrap();
    peer.stop().await;
}
