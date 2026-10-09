use super::*;
use mayhem_bridge::ScBridgeClient;

fn opener(context: &n::Context) -> Value {
    json!({"t":"p.negotiate.open","schema_version":1,"context":context})
}
#[tokio::test]
async fn provider_opening_dispatcher_keeps_existing_sessions_responsive_at_global_quota() {
    use serving::dispatch::{Dispatcher, Health, Limits, Registration};
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Fiat, &f, &chat(), false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let controller = server(&s, &f, &peer, bounds());
    let original = context(&peer);
    let bridge = Bridge::start(original.buyer.as_str(), &original.offer.provider_pubkey).await;
    let listener =
        n::opening::Listener::connect(bridge.config(false), peer.identity.clone(), wire_limits())
            .await
            .unwrap();
    let limits = Limits {
        sessions: 2,
        registrations: 2,
        observation_wait: Duration::from_millis(50),
    };
    let registration = || Registration::new(&original.offer, controller.clone()).unwrap();
    assert!(Dispatcher::new(
        listener,
        vec![registration(), registration()],
        limits,
        maintenance_policy(),
        8
    )
    .is_err());
    assert!(controller.maintenance(maintenance_policy(), 8).is_ok());
    let mut wrong = original.offer.clone();
    wrong.provider_pubkey = d(801).as_str().into();
    assert!(Registration::new(&wrong, controller.clone()).is_err());
    let mut wrong = original.offer.clone();
    wrong.endpoint = ProxyEndpoint::Responses;
    assert!(Registration::new(&wrong, controller.clone()).is_err());
    let listener =
        n::opening::Listener::connect(bridge.config(false), peer.identity.clone(), wire_limits())
            .await
            .unwrap();
    let dispatcher = Dispatcher::new(
        listener,
        vec![registration()],
        limits,
        maintenance_policy(),
        8,
    )
    .unwrap();
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let (updates, health) = tokio::sync::watch::channel(Health::default());
    let task = tokio::spawn(dispatcher.run(stopped, updates));
    let mut first = n::Channel::dial(
        bridge.config(true),
        original.clone(),
        &identity(&peer),
        wire_limits(),
    )
    .await
    .unwrap();
    let mut second = original.clone();
    second.session_id = d(802);
    second.billing_id = d(803);
    let second = n::Channel::dial(bridge.config(true), second, &identity(&peer), wire_limits())
        .await
        .unwrap();
    let mut third = original.clone();
    third.session_id = d(804);
    third.billing_id = d(805);
    assert!(n::Channel::dial(
        bridge
            .config(true)
            .with_operation_deadline(Some(Duration::from_millis(200))),
        third,
        &identity(&peer),
        wire_limits()
    )
    .await
    .is_err());
    assert_eq!(health.borrow().accepted, 2);
    assert_eq!(health.borrow().rejected, 1);
    // Public description has separate control headroom even when every paid
    // serving connection is occupied. It neither allocates a lease nor opens
    // another paid session, and existing control streams remain responsive.
    let description = mayhem_proxy::descriptor::Context::new(
        &identity(&peer),
        original.offer.clone(),
        original.rail,
        original.settlement_policy_hash.clone(),
    )
    .unwrap();
    let described =
        mayhem_proxy::descriptor::fetch(bridge.config(true), &identity(&peer), &description)
            .await
            .unwrap();
    assert!(described
        .adapter(
            &description,
            f.adapter.contract_hash(),
            f.adapter.recipe_hash(),
            f.adapter.limits()
        )
        .is_ok());
    assert_eq!(health.borrow().accepted, 2);
    assert_eq!(
        s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
        0
    );
    assert_eq!(peer.command("status").await["submissions"], 0);
    // Existing control streams remain usable when a third opening is refused.
    first.send(&n::Message::Recover).await.unwrap();
    assert!(matches!(
        first
            .receive(Duration::from_secs(3))
            .await
            .unwrap()
            .message(),
        n::Message::Refused { .. }
    ));
    drop(second);
    stop.send_replace(true);
    task.await.unwrap().unwrap();
    assert!(!health.borrow().running);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
        0
    );
    assert_eq!(peer.command("status").await["submissions"], 0);
    peer.stop().await;
}
fn wire_limits() -> exchange::Limits {
    exchange::Limits {
        max_message_bytes: 1024 * 1024,
    }
}

#[tokio::test]
async fn provider_opening_dispatches_registered_submarkets_and_runs_one_recovery_owner_per_controller(
) {
    use serving::dispatch::{Dispatcher, Health, Limits, Registration};
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        let bytes = chat();
        let backend = backend(200, answer(), Duration::ZERO).await;
        let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut peer = Peer::start(rail, &f, &bytes, false, None).await;
        let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
        let controller = server(&s, &f, &peer, bounds());
        let context = context(&peer);
        let bridge = Bridge::start(context.buyer.as_str(), &context.offer.provider_pubkey).await;
        let listener = n::opening::Listener::connect(
            bridge.config(false),
            peer.identity.clone(),
            wire_limits(),
        )
        .await
        .unwrap();
        let mut alias = context.offer.clone();
        alias.market_id = d(701).as_str().into();
        let registrations = vec![
            Registration::new(&context.offer, controller.clone()).unwrap(),
            Registration::new(&alias, controller.clone()).unwrap(),
        ];
        let dispatcher = Dispatcher::new(
            listener,
            registrations,
            Limits {
                sessions: 2,
                registrations: 2,
                observation_wait: Duration::from_millis(50),
            },
            maintenance_policy(),
            7,
        )
        .unwrap();
        assert!(matches!(
            controller.maintenance(maintenance_policy(), 7),
            Err(serving::Error::Busy)
        ));
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let (updates, mut health) = tokio::sync::watch::channel(Health::default());
        let task = tokio::spawn(dispatcher.run(stopped, updates));
        let mut unknown = context.clone();
        unknown.offer.market_id = d(702).as_str().into();
        let short = bridge
            .config(true)
            .with_operation_deadline(Some(Duration::from_millis(200)));
        assert!(
            n::Channel::dial(short, unknown, &identity(&peer), wire_limits())
                .await
                .is_err()
        );
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        let buyer = n::Channel::dial(
            bridge.config(true),
            context.clone(),
            &identity(&peer),
            wire_limits(),
        )
        .await
        .unwrap();
        let (mut buyer, saved) = purchase(&s, &peer, &bytes, buyer, &context).await;
        s.buyer
            .publish(saved.key().clone(), &recovery(&f, &peer), 1002)
            .await
            .unwrap();
        buyer
            .send(&exchange::Message::Execute {
                request: serde_json::from_slice(&bytes).unwrap(),
                streaming: false,
            })
            .await
            .unwrap();
        let result = next(&mut buyer).await;
        settle(&s, &f, &peer, &mut buyer, &bytes, result).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                health.changed().await.unwrap();
                if health.borrow().completed == 1 && health.borrow().maintenance_steps > 0 {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(health.borrow().accepted, 1);
        assert_eq!(health.borrow().rejected, 1);
        assert_eq!(health.borrow().failed, 0);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        stop.send_replace(true);
        task.await.unwrap().unwrap();
        assert!(!health.borrow().running);
        assert_eq!(health.borrow().active, 0);
        assert!(controller.maintenance(maintenance_policy(), 7).is_ok());
        peer.stop().await;
    }
}

#[tokio::test]
async fn provider_opening_authenticates_context_ignores_foreign_frames_and_bounds_duplicate_sessions(
) {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Fiat, &f, &chat(), false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let controller = server(&s, &f, &peer, bounds());
    let context = context(&peer);
    let bridge = Bridge::start(context.buyer.as_str(), &context.offer.provider_pubkey).await;
    let mut listener =
        n::opening::Listener::connect(bridge.config(false), peer.identity.clone(), wire_limits())
            .await
            .unwrap();
    let mut buyer = ScBridgeClient::connect(bridge.config(true)).await.unwrap();
    buyer
        .session_subscribe([context.session_id.as_str()])
        .await
        .unwrap();
    buyer
        .session_open(&context.offer.provider_pubkey, context.session_id.as_str())
        .await
        .unwrap();
    let good = opener(&context);
    let mut bad = Vec::new();
    for (field, value) in [
        ("buyer", json!("f".repeat(64))),
        ("session_id", json!("f".repeat(64))),
        ("network_id", json!("different")),
    ] {
        let mut frame = good.clone();
        frame["context"][field] = value;
        bad.push(frame);
    }
    let mut frame = good.clone();
    frame["context"]["offer"]["provider_pubkey"] = json!("f".repeat(64));
    bad.push(frame);
    let mut frame = good.clone();
    frame["schema_version"] = json!(2);
    bad.push(frame);
    let mut frame = good.clone();
    frame["extra"] = json!(true);
    bad.push(frame);
    let mut frame = good.clone();
    frame["extra"] = json!("x".repeat(41 * 1024));
    bad.push(frame);
    let send = async {
        buyer
            .session_send(
                &context.offer.provider_pubkey,
                context.session_id.as_str(),
                json!({"t":"s.open"}),
            )
            .await
            .unwrap();
        for frame in bad {
            buyer
                .session_send(
                    &context.offer.provider_pubkey,
                    context.session_id.as_str(),
                    frame,
                )
                .await
                .unwrap();
        }
        *bridge.attack.lock().await = Some("lineage");
        buyer
            .session_send(
                &context.offer.provider_pubkey,
                context.session_id.as_str(),
                good.clone(),
            )
            .await
            .unwrap();
        buyer
            .session_send(
                &context.offer.provider_pubkey,
                context.session_id.as_str(),
                good.clone(),
            )
            .await
            .unwrap();
    };
    let (_, incoming) = tokio::join!(send, listener.next(Duration::from_secs(3)));
    let incoming = incoming.unwrap();
    assert_eq!(listener.counts().accepted, 1);
    assert_eq!(listener.counts().rejected, 8);
    assert_eq!(listener.counts().unrelated, 1);
    let handle = controller.accept(incoming).unwrap();
    let ready = buyer
        .next_session_frame(Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(ready["frame"]["t"], "p.negotiate.ready");
    assert_eq!(
        ready["frame"]["context_digest"],
        context.digest().unwrap().as_str()
    );
    buyer
        .session_send(
            &context.offer.provider_pubkey,
            context.session_id.as_str(),
            good,
        )
        .await
        .unwrap();
    let duplicate = listener.next(Duration::from_secs(3)).await.unwrap();
    assert!(matches!(
        controller.accept(duplicate),
        Err(serving::Error::Busy)
    ));
    handle.disconnect();
    // This buyer also sent a second raw opening into its already-established
    // negotiation stream. Strict framing may close that same stream first;
    // neither path permits a duplicate controller or an inference dispatch.
    assert!(matches!(
        handle.wait().await,
        Ok(serving::End::Disconnected) | Err(serving::Error::Transport(exchange::Error::Protocol))
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
        0
    );
    assert_eq!(peer.command("status").await["submissions"], 0);
    peer.stop().await;
}

#[tokio::test]
async fn provider_opening_rejects_substituted_readiness_before_any_request_or_hold() {
    for attack in ["ready_context", "remote", "lineage", "extra"] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut peer = Peer::start(ProxyRail::Tap, &f, &chat(), false, None).await;
        let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
        let controller = server(&s, &f, &peer, bounds());
        let context = context(&peer);
        let bridge = Bridge::start(context.buyer.as_str(), &context.offer.provider_pubkey).await;
        let mut listener = n::opening::Listener::connect(
            bridge.config(false),
            peer.identity.clone(),
            wire_limits(),
        )
        .await
        .unwrap();
        let local = identity(&peer);
        let dial = n::Channel::dial(bridge.config(true), context, &local, wire_limits());
        let accept = async {
            let incoming = listener.next(Duration::from_secs(3)).await.unwrap();
            *bridge.attack.lock().await = Some(attack);
            controller.accept(incoming).unwrap()
        };
        let (buyer, handle) = tokio::join!(dial, accept);
        assert!(buyer.is_err());
        handle.disconnect();
        let _ = handle.wait().await;
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
            0
        );
        assert_eq!(peer.command("status").await["submissions"], 0);
        peer.stop().await;
    }
}

#[tokio::test]
async fn provider_opening_has_a_finite_wait_and_expires_undispatched_context() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &chat(), false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let controller = server(&s, &f, &peer, bounds());
    let context = context(&peer);
    let bridge = Bridge::start(context.buyer.as_str(), &context.offer.provider_pubkey).await;
    let short = Duration::from_millis(100);
    let mut listener = n::opening::Listener::connect(
        bridge.config(false).with_operation_deadline(Some(short)),
        peer.identity.clone(),
        wire_limits(),
    )
    .await
    .unwrap();
    let local = identity(&peer);
    let dial = n::Channel::dial(
        bridge.config(true).with_operation_deadline(Some(short)),
        context,
        &local,
        wire_limits(),
    );
    let receive = listener.next(Duration::from_secs(3));
    let (buyer, incoming) = tokio::join!(dial, receive);
    assert!(buyer.is_err());
    tokio::time::sleep(short).await;
    assert!(matches!(
        controller.accept(incoming.unwrap()),
        Err(serving::Error::Transport(exchange::Error::Identity))
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
        0
    );
    assert_eq!(peer.command("status").await["submissions"], 0);
    peer.stop().await;
}
