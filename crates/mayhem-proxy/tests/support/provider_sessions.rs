use super::*;
use mayhem_proxy::{exchange, serving};
#[path = "provider_opening.rs"]
mod opening;

#[tokio::test]
async fn provider_session_offers_and_recovers_signed_nonexecution_waiver_without_resending() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for streaming in [false, true] {
            let backend = backend(
                503,
                crate::probe_execution::vllm_refusal(false),
                Duration::ZERO,
            )
            .await;
            let bytes = if streaming { stream_request() } else { chat() };
            let f = Fixture::with_profile(
                &backend.base,
                ProxyEndpoint::Chat,
                128 * 1024 * 1024,
                "vllm_admission_v1",
            );
            let mut peer = Peer::start(rail, &f, &bytes, streaming, None).await;
            let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
            let (controller, monitor) = observed_server(&s, &f, &peer);
            let (_bridge, mut buyer, handle, saved) = connect(&s, &peer, &bytes, &controller).await;
            s.buyer
                .publish(saved.key().clone(), &recovery(&f, &peer), 1002)
                .await
                .unwrap();
            buyer
                .send(&exchange::Message::Execute {
                    request: serde_json::from_slice(&bytes).unwrap(),
                    streaming,
                })
                .await
                .unwrap();
            assert!(
                matches!(next(&mut buyer).await.message(), exchange::Message::Failure {failure} if failure.execution == Execution::Rejected)
            );
            let received = next(&mut buyer).await;
            let exchange::Message::Waiver { value } = received.message() else {
                panic!("expected zero-charge proposal")
            };
            let waiver = value.clone();
            assert!(waiver.draft.non_execution.is_some());
            occupied(&s, 0);
            assert_eq!(monitor.snapshot(&d(201)).unwrap().allowance, 0);
            assert_eq!(
                peer.command("status").await["publications"],
                1,
                "reservation only"
            );
            // A status retry replays this exact signed offer; it cannot infer again.
            buyer.send(&exchange::Message::Status).await.unwrap();
            assert!(matches!(
                next(&mut buyer).await.message(),
                exchange::Message::State {
                    state: exchange::PublicState::AwaitingReceipt
                }
            ));
            let again = next(&mut buyer).await;
            let exchange::Message::Waiver { value: repeated } = again.message() else {
                panic!("waiver recovery required")
            };
            assert_eq!(
                serde_json::to_value(&waiver).unwrap(),
                serde_json::to_value(repeated).unwrap()
            );
            let recovery = recovery(&f, &peer);
            recovery
                .refresh(buyer.session().authorization(), 1003)
                .await
                .unwrap();
            recovery
                .approve_waiver(waiver, bytes.clone(), None, false, 1004)
                .await
                .unwrap();
            let key = Digest::new(buyer.session().authorization().terms.digest().unwrap()).unwrap();
            let acknowledgment = recovery.sign_approved(&s.buyer_signer, key).await.unwrap();
            buyer
                .send(&exchange::Message::Acknowledge { acknowledgment })
                .await
                .unwrap();
            assert!(matches!(
                next(&mut buyer).await.message(),
                exchange::Message::State {
                    state: exchange::PublicState::Settled
                }
            ));
            assert_eq!(handle.wait().await.unwrap(), serving::End::Settled);
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                peer.command("status").await["publications"],
                2,
                "reservation and closure"
            );
            peer.stop().await;
        }
    }
}

fn bounds() -> serving::Limits {
    serving::Limits {
        sessions: 4,
        per_buyer: 2,
        outbound_messages: 16,
        outbound_bytes: 4 * 1024 * 1024,
        control_wait: Duration::from_secs(10),
        proposals: limits(),
    }
}
fn pool(f: &Fixture) -> Arc<Pool> {
    Arc::new(
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
    )
}
fn server(
    s: &Controlled,
    f: &Fixture,
    peer: &Peer,
    bounds: serving::Limits,
) -> serving::Controller {
    serving::Controller::new(
        s.runtime.clone(),
        s.journal.clone(),
        s.signing.clone(),
        peer.client.clone(),
        pool(f),
        bounds,
    )
    .unwrap()
}
fn observed_server(
    s: &Controlled,
    f: &Fixture,
    peer: &Peer,
) -> (serving::Controller, mayhem_proxy::health::Monitor) {
    use mayhem_proxy::{
        health::{Class, Monitor, Policy, Thinking},
        supervisor::RefreshPolicy,
    };
    // Canonical offer terms keep their credential group; the physical backend
    // allocation is an additional private constraint, not a different market.
    physical_constraint(s);
    let monitor = Monitor::new(
        Policy {
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
                page_pause_ms: 10,
                retry_initial_ms: 1000,
                retry_max_ms: 30_000,
                jitter_percent: 0,
            },
        },
        2,
        1,
    )
    .unwrap();
    monitor
        .register(d(201), 2, f.adapter.endpoint() != ProxyEndpoint::Decisions)
        .unwrap();
    s.runtime
        .capacity
        .bind_live(capacity::Scope::Group(d(200)), monitor.connection_source())
        .unwrap();
    s.runtime
        .capacity
        .bind_live(
            capacity::Scope::Route(d(201)),
            monitor.route_source(&d(201)).unwrap(),
        )
        .unwrap();
    assert_eq!(s.runtime.capacity.status(&d(201)).unwrap().available, 0);
    // Fixture bootstrap evidence only; production must earn it through separately
    // authorized, durably tracked probes. The actual paid requests below refresh it.
    for _ in 0..3 {
        monitor
            .observe_request(&d(201), Class::new(1024, Thinking::Unknown, false))
            .unwrap()
            .success(None);
    }
    let controller = if f.adapter.endpoint() != ProxyEndpoint::Decisions {
        serving::Controller::new_measured(
            s.runtime.clone(),
            s.journal.clone(),
            s.signing.clone(),
            peer.client.clone(),
            pool(f),
            bounds(),
            monitor.clone(),
            crate::native_execution::source(f),
        )
    } else {
        serving::Controller::new_observed(
            s.runtime.clone(),
            s.journal.clone(),
            s.signing.clone(),
            peer.client.clone(),
            pool(f),
            bounds(),
            monitor.clone(),
        )
    }
    .unwrap();
    (controller, monitor)
}
fn physical_constraint(s: &Controlled) {
    s.runtime
        .capacity
        .configure_allocation_group(d(202), 2)
        .unwrap();
    s.runtime
        .capacity
        .configure_route_with_constraints(
            capacity::Route {
                id: d(201),
                group: d(200),
                lane: capacity::Lane::Proxy,
                max_concurrency: 2,
            },
            vec![d(202)],
        )
        .unwrap();
}
fn occupied(s: &Controlled, expected: u32) {
    for group in [200, 202] {
        assert_eq!(
            s.runtime.capacity.group_status(&d(group)).unwrap().occupied,
            expected
        );
    }
    assert_eq!(
        s.runtime.capacity.status(&d(201)).unwrap().route_occupied,
        expected
    );
}
async fn connect(
    s: &Controlled,
    peer: &Peer,
    bytes: &[u8],
    controller: &serving::Controller,
) -> (Bridge, Channel, serving::Handle, SavedPurchase) {
    let context = context(peer);
    let bridge = Bridge::start(context.buyer.as_str(), &context.offer.provider_pubkey).await;
    let wire_limits = exchange::Limits {
        max_message_bytes: 1024 * 1024,
    };
    let mut listener =
        n::opening::Listener::connect(bridge.config(false), peer.identity.clone(), wire_limits)
            .await
            .unwrap();
    let buyer_identity = identity(peer);
    let dialing = n::Channel::dial(
        bridge.config(true),
        context.clone(),
        &buyer_identity,
        wire_limits,
    );
    let accepting = async {
        let incoming = listener.next(Duration::from_secs(5)).await.unwrap();
        controller.accept(incoming).unwrap()
    };
    let (buyer, handle) = tokio::join!(dialing, accepting);
    let buyer = buyer.unwrap();
    drop(listener);
    let (buyer, saved) = purchase(s, peer, bytes, buyer, &context).await;
    (bridge, buyer, handle, saved)
}
async fn purchase(
    s: &Controlled,
    peer: &Peer,
    bytes: &[u8],
    mut buyer: n::Channel,
    context: &n::Context,
) -> (Channel, SavedPurchase) {
    buyer
        .send(&n::Message::Request {
            request: serde_json::from_slice(bytes).unwrap(),
        })
        .await
        .unwrap();
    let response = buyer
        .receive(Duration::from_secs(5))
        .await
        .unwrap()
        .into_message(&context, Role::Buyer)
        .unwrap();
    let n::Message::Proposal { proposal } = response else {
        panic!("proposal required")
    };
    let output = (s.runtime.adapter.endpoint() != ProxyEndpoint::Decisions).then_some(37);
    let request = PurchaseRequest::new(
        proposal.adapter.clone(),
        bytes.to_vec(),
        prices(peer),
        output,
        lifetimes(),
    )
    .unwrap();
    let quote = peer.buyer_client.quote(&query(peer)).await.unwrap();
    let purchase = quote
        .prepare_purchase(&request, &proposal.session_binding(&context).unwrap())
        .unwrap();
    let saved = s
        .buyer
        .sign(purchase, quote, s.buyer_signer.clone(), 1000)
        .await
        .unwrap();
    buyer
        .send(&n::Message::Offer {
            offer: saved.offer(),
        })
        .await
        .unwrap();
    let accepted = buyer
        .receive(Duration::from_secs(5))
        .await
        .unwrap()
        .into_message(&context, Role::Buyer)
        .unwrap();
    let n::Message::Accepted { value } = accepted else {
        panic!("acceptance required")
    };
    s.buyer
        .retain_provider_acceptance(value.authorization)
        .await
        .unwrap();
    (buyer.into_paid(&identity(peer)).unwrap(), saved)
}
async fn next(buyer: &mut Channel) -> exchange::Received {
    buyer.receive(Some(Duration::from_secs(8))).await.unwrap()
}
async fn settle(
    s: &Controlled,
    f: &Fixture,
    peer: &Peer,
    buyer: &mut Channel,
    bytes: &[u8],
    received: exchange::Received,
) {
    let acknowledgment = approve(s, f, peer, buyer, bytes, received).await;
    buyer
        .send(&exchange::Message::Acknowledge { acknowledgment })
        .await
        .unwrap();
    assert!(matches!(
        next(buyer).await.message(),
        exchange::Message::State {
            state: exchange::PublicState::Settled
        }
    ));
}
async fn approve(
    s: &Controlled,
    f: &Fixture,
    peer: &Peer,
    buyer: &mut Channel,
    bytes: &[u8],
    received: exchange::Received,
) -> financial::recovery::SignedAcknowledgment {
    let snapshot = mayhem_proxy::buyer::PublicAcceptanceSnapshot {
        version: 1,
        adapter: f.adapter.public_snapshot(),
        offer: buyer.session().authorization().terms.offer.clone(),
    };
    let observed = buyer
        .session()
        .decode_result(received, &snapshot, bytes)
        .unwrap();
    let received = next(buyer).await;
    let exchange::Message::Receipt { value } = received.message() else {
        panic!("receipt required")
    };
    let recovery = recovery(f, peer);
    recovery
        .refresh(buyer.session().authorization(), 1003)
        .await
        .unwrap();
    let key = Digest::new(buyer.session().authorization().terms.digest().unwrap()).unwrap();
    recovery
        .approve_receipt(
            &pool(f),
            value.clone(),
            snapshot,
            bytes.to_vec(),
            observed,
            false,
            1004,
        )
        .await
        .unwrap();
    recovery.sign_approved(&s.buyer_signer, key).await.unwrap()
}

#[tokio::test]
async fn provider_session_negotiates_requires_funding_and_settles_all_json_endpoints_and_rails() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for (endpoint, bytes, response) in cases() {
            let backend = backend(200, response, Duration::ZERO).await;
            let f = Fixture::new(&backend.base, endpoint);
            let mut peer = Peer::start(rail, &f, &bytes, false, None).await;
            let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
            let (controller, monitor) = observed_server(&s, &f, &peer);
            let before = monitor.connection_source().revision().unwrap();
            let (_bridge, mut buyer, handle, saved) = connect(&s, &peer, &bytes, &controller).await;
            occupied(&s, 1);
            let command = exchange::Message::Execute {
                request: serde_json::from_slice(&bytes).unwrap(),
                streaming: false,
            };
            buyer.send(&command).await.unwrap();
            assert!(
                matches!(next(&mut buyer).await.message(),exchange::Message::Failure {failure}
                if failure.code==mayhem_proxy::connector::failure::Code::AdmissionUnavailable)
            );
            assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
            occupied(&s, 1);
            s.buyer
                .publish(saved.key().clone(), &recovery(&f, &peer), 1002)
                .await
                .unwrap();
            buyer.send(&command).await.unwrap();
            let received = next(&mut buyer).await;
            assert!(matches!(
                received.message(),
                exchange::Message::Result { .. }
            ));
            settle(&s, &f, &peer, &mut buyer, &bytes, received).await;
            assert_eq!(handle.wait().await.unwrap(), serving::End::Settled);
            assert!(monitor.connection_source().revision().unwrap() > before);
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            occupied(&s, 0);
            peer.stop().await;
        }
    }
}

#[tokio::test]
async fn provider_session_streams_and_settles_each_supported_stream_endpoint_on_each_rail() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for (endpoint, request, answer) in [
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
        ] {
            let backend = backend_raw(200, answer, "text/event-stream", Duration::ZERO).await;
            let f = Fixture::new(&backend.base, endpoint);
            let mut peer = Peer::start(rail, &f, &request, true, None).await;
            let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
            let (controller, monitor) = observed_server(&s, &f, &peer);
            let before = monitor.connection_source().revision().unwrap();
            let (_bridge, mut buyer, handle, saved) =
                connect(&s, &peer, &request, &controller).await;
            occupied(&s, 1);
            s.buyer
                .publish(saved.key().clone(), &recovery(&f, &peer), 1002)
                .await
                .unwrap();
            buyer
                .send(&exchange::Message::Execute {
                    request: serde_json::from_slice(&request).unwrap(),
                    streaming: true,
                })
                .await
                .unwrap();
            let mut events = 0;
            let received = loop {
                let received = next(&mut buyer).await;
                match received.message() {
                    exchange::Message::Stream { .. } => events += 1,
                    exchange::Message::Result { .. } => break received,
                    _ => panic!("expected stream or retained result"),
                }
            };
            assert!(events > 0);
            // Durable completed inference frees physical capacity before payment ACK.
            occupied(&s, 0);
            settle(&s, &f, &peer, &mut buyer, &request, received).await;
            assert_eq!(handle.wait().await.unwrap(), serving::End::Settled);
            assert!(monitor.connection_source().revision().unwrap() > before);
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            occupied(&s, 0);
            peer.stop().await;
        }
    }
}

async fn dispatched(backend: &Backend) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while backend.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn provider_session_disconnect_retains_json_result_and_recovers_without_a_second_post() {
    let bytes = chat();
    let backend = backend(200, answer(), Duration::from_millis(300)).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tap, &f, &bytes, false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let (controller, _monitor) = observed_server(&s, &f, &peer);
    let (bridge, mut buyer, handle, saved) = connect(&s, &peer, &bytes, &controller).await;
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
    dispatched(&backend).await;
    handle.disconnect();
    drop(buyer);
    drop(bridge);
    assert_eq!(handle.wait().await.unwrap(), serving::End::Disconnected);
    // The handle waits for the ongoing inference to finish and persist its result.
    // Lost delivery/ACK retains the result and financial obligation, not a GPU slot.
    occupied(&s, 0);
    let Pair {
        bridge: _bridge,
        mut buyer,
        provider,
        context,
    } = open(&peer, context(&peer), 1024 * 1024).await;
    let handle = controller.start(provider).unwrap();
    buyer.send(&n::Message::Recover).await.unwrap();
    let reply = buyer
        .receive(Duration::from_secs(5))
        .await
        .unwrap()
        .into_message(&context, Role::Buyer)
        .unwrap();
    assert!(matches!(reply, n::Message::Accepted { .. }));
    let mut buyer = buyer.into_paid(&identity(&peer)).unwrap();
    buyer.send(&exchange::Message::Status).await.unwrap();
    assert!(matches!(
        next(&mut buyer).await.message(),
        exchange::Message::State {
            state: exchange::PublicState::AwaitingReceipt
        }
    ));
    let result = next(&mut buyer).await;
    settle(&s, &f, &peer, &mut buyer, &bytes, result).await;
    assert_eq!(handle.wait().await.unwrap(), serving::End::Settled);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    occupied(&s, 0);
    peer.stop().await;
}

#[tokio::test]
async fn provider_session_long_generation_outlives_control_wait_and_explicit_cancel_stays_uncertain(
) {
    for cancel in [false, true] {
        let bytes = chat();
        let backend = backend(200, answer(), Duration::from_secs(2)).await;
        let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut peer = Peer::start(ProxyRail::Fiat, &f, &bytes, false, None).await;
        let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
        let mut limits = bounds();
        limits.control_wait = Duration::from_secs(1);
        physical_constraint(&s);
        capacity_ready(&s.runtime.capacity, capacity::Scope::Route(d(201)));
        let controller = server(&s, &f, &peer, limits);
        let (_bridge, mut buyer, handle, saved) = connect(&s, &peer, &bytes, &controller).await;
        s.buyer
            .publish(saved.key().clone(), &recovery(&f, &peer), 1002)
            .await
            .unwrap();
        let command = exchange::Message::Execute {
            request: serde_json::from_slice(&bytes).unwrap(),
            streaming: false,
        };
        buyer.send(&command).await.unwrap();
        dispatched(&backend).await;
        buyer.send(&command).await.unwrap();
        assert!(matches!(
            next(&mut buyer).await.message(),
            exchange::Message::State {
                state: exchange::PublicState::Running
            }
        ));
        if cancel {
            buyer.send(&exchange::Message::Cancel).await.unwrap();
            let mut cancelled = false;
            let mut failed = false;
            for _ in 0..2 {
                match next(&mut buyer).await.message() {
                    exchange::Message::State {
                        state: exchange::PublicState::CancelRequested,
                    } => cancelled = true,
                    exchange::Message::Failure { failure } => {
                        assert_eq!(
                            failure.code,
                            mayhem_proxy::connector::failure::Code::RequestCancelled
                        );
                        failed = true;
                    }
                    _ => panic!("unexpected cancellation output"),
                }
            }
            assert!(cancelled && failed);
            occupied(&s, 1);
            handle.disconnect();
            assert_eq!(handle.wait().await.unwrap(), serving::End::Disconnected);
        } else {
            let result = next(&mut buyer).await;
            settle(&s, &f, &peer, &mut buyer, &bytes, result).await;
            assert_eq!(handle.wait().await.unwrap(), serving::End::Settled);
            occupied(&s, 0);
        }
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        peer.stop().await;
    }
}

#[tokio::test]
async fn provider_session_reports_bad_arguments_without_claiming_backend_occupation() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &chat(), false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let controller = server(&s, &f, &peer, bounds());
    let mut request: Value = serde_json::from_slice(&chat()).unwrap();
    request["messages"] = json!("invalid");
    let mut context = context(&peer);
    context.request_hash =
        Digest::new(mayhem_proto::endpoint_request_fingerprint(&request)).unwrap();
    let Pair {
        bridge: _bridge,
        mut buyer,
        provider,
        ..
    } = open(&peer, context.clone(), 1024 * 1024).await;
    let handle = controller.start(provider).unwrap();
    buyer.send(&n::Message::Request { request }).await.unwrap();
    let reply = buyer
        .receive(Duration::from_secs(5))
        .await
        .unwrap()
        .into_message(&context, Role::Buyer)
        .unwrap();
    let n::Message::Refused { failure } = reply else {
        panic!("expected specific request rejection")
    };
    assert_eq!(
        failure.code,
        mayhem_proxy::connector::failure::Code::InvalidRequest
    );
    assert_eq!(failure.parameter.as_deref(), Some("messages"));
    assert_eq!(handle.wait().await.unwrap(), serving::End::Refused);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
        0
    );
    peer.stop().await;
}

fn maintenance_policy() -> serving::maintenance::Policy {
    serving::maintenance::Policy {
        page_size: 1,
        schedule: mayhem_proxy::supervisor::RefreshPolicy {
            interval_ms: 20,
            page_pause_ms: 1,
            retry_initial_ms: 10,
            retry_max_ms: 100,
            jitter_percent: 0,
        },
    }
}

#[tokio::test]
async fn provider_session_maintenance_finishes_retained_ack_without_buyer_or_new_inference() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        let bytes = chat();
        let backend = backend(200, answer(), Duration::ZERO).await;
        let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut peer = Peer::start(rail, &f, &bytes, false, None).await;
        let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
        let controller = server(&s, &f, &peer, bounds());
        let (_bridge, mut buyer, handle, saved) = connect(&s, &peer, &bytes, &controller).await;
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
        let acknowledgment = approve(&s, &f, &peer, &mut buyer, &bytes, result).await;
        peer.command("publish_pending").await;
        buyer
            .send(&exchange::Message::Acknowledge { acknowledgment })
            .await
            .unwrap();
        assert!(matches!(
            next(&mut buyer).await.message(),
            exchange::Message::State {
                state: exchange::PublicState::AwaitingReceipt
            }
        ));
        let invocation = buyer.session().invocation().clone();
        handle.disconnect();
        drop(buyer);
        assert_eq!(handle.wait().await.unwrap(), serving::End::Disconnected);
        assert_ne!(
            s.journal.get(&invocation).unwrap().unwrap().phase,
            attempts::Phase::Closed
        );
        let submissions_before_recovery = peer.command("status").await["submissions"].clone();
        peer.command("flush_publication").await;
        let mut runner = controller.maintenance(maintenance_policy(), 7).unwrap();
        assert!(
            controller.maintenance(maintenance_policy(), 8).is_err(),
            "one maintenance owner"
        );
        let mut recovered = 0;
        for _ in 0..4 {
            let page = runner.page().await.unwrap();
            assert!(page.examined <= 1);
            assert_eq!(page.failed, 0);
            recovered += page.recovered;
        }
        assert_eq!(recovered, 1);
        assert_eq!(
            s.journal.get(&invocation).unwrap().unwrap().phase,
            attempts::Phase::Closed
        );
        assert_eq!(
            peer.command("status").await["submissions"],
            submissions_before_recovery
        );
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        drop(runner);
        assert!(controller.maintenance(maintenance_policy(), 9).is_ok());
        peer.stop().await;
    }
}

#[tokio::test]
async fn provider_session_periodic_recovery_retires_only_canonical_expired_unsigned_admission() {
    let bytes = chat();
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &bytes, false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let controller = server(&s, &f, &peer, bounds());
    let (_bridge, buyer, handle, saved) = connect(&s, &peer, &bytes, &controller).await;
    handle.disconnect();
    drop(buyer);
    handle.wait().await.unwrap();
    let mut runner = controller.maintenance(maintenance_policy(), 7).unwrap();
    for _ in 0..4 {
        assert_eq!(runner.page().await.unwrap().recovered, 0);
    }
    assert_eq!(
        s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
        1
    );
    peer.command(&json!({"epoch":saved.offer().terms.billing_epoch}).to_string())
        .await;
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let (updates, mut health) =
        tokio::sync::watch::channel(serving::maintenance::Health::default());
    let work = tokio::spawn(runner.run(stopped, updates));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            health.changed().await.unwrap();
            if health
                .borrow()
                .last
                .as_ref()
                .is_some_and(|p| p.recovered > 0)
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    stop.send_replace(true);
    work.await.unwrap().unwrap();
    assert!(!health.borrow().running);
    assert_eq!(
        s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
        0
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(peer.command("status").await["publications"], 0);
    peer.stop().await;
}

#[tokio::test]
async fn provider_session_control_capacity_releases_after_handle_drop_and_idle_wait() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &chat(), false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let mut limits = bounds();
    limits.sessions = 1;
    limits.per_buyer = 1;
    limits.control_wait = Duration::from_millis(100);
    let controller = server(&s, &f, &peer, limits);
    let first = open(&peer, context(&peer), 1024 * 1024).await;
    let handle = controller.start(first.provider).unwrap();
    let second = open(&peer, context(&peer), 1024 * 1024).await;
    assert!(matches!(
        controller.start(second.provider),
        Err(serving::Error::Busy)
    ));
    // No request starts and no capacity is allocated while waiting for negotiation.
    let result = handle.wait().await;
    assert!(result.is_err());
    let third = open(&peer, context(&peer), 1024 * 1024).await;
    let handle = controller.start(third.provider).unwrap();
    handle.disconnect();
    assert_eq!(handle.wait().await.unwrap(), serving::End::Disconnected);
    assert_eq!(
        s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
        0
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    peer.stop().await;
}
