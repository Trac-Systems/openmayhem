use super::*;
use mayhem_proxy::exchange::{self, Channel, Limits, Message, Role, Session};
#[path = "exchange_bridge.rs"]
mod exchange_bridge;
use super::protected_signing::{authorities, buyer as buyer_recovery, key, snapshot};
use exchange_bridge::Bridge;

async fn channels(p: &Paid, bridge: &Bridge, bound: usize) -> (Channel, Channel) {
    let mut buyer = p.peer.identity.clone();
    buyer.controller_pubkey = Digest::new(&p.authorization.terms.buyer_pubkey).unwrap();
    let provider = Channel::connect(
        bridge.config(false),
        Session::new(p.authorization.clone(), &p.peer.identity, Role::Provider).unwrap(),
        Limits {
            max_message_bytes: bound,
        },
    )
    .await
    .unwrap();
    let buyer = Channel::connect(
        bridge.config(true),
        Session::new(p.authorization.clone(), &buyer, Role::Buyer).unwrap(),
        Limits {
            max_message_bytes: bound,
        },
    )
    .await
    .unwrap();
    (buyer, provider)
}
async fn receive(
    sender: &mut Channel,
    receiver: &mut Channel,
    message: Message,
) -> exchange::Received {
    let (sent, received) = tokio::join!(
        sender.send(&message),
        receiver.receive(Some(Duration::from_secs(10)))
    );
    sent.unwrap();
    received.unwrap()
}
async fn paid(
    endpoint: ProxyEndpoint,
    rail: ProxyRail,
    base: &str,
    request: &[u8],
    streaming: bool,
) -> Paid {
    Paid::start_scoped(base, endpoint, rail, request, streaming, None, true, true).await
}

async fn settle(
    p: &mut Paid,
    buyer: &mut Channel,
    provider: &mut Channel,
    request: &[u8],
    body: &Value,
) {
    let b = buyer_recovery(p);
    b.refresh(&p.authorization, 1).await.unwrap();
    let (provider_signer, buyer_signer) = authorities(p).await;
    let delivered = receive(
        provider,
        buyer,
        Message::Result {
            response: body.clone(),
        },
    )
    .await;
    let evidence = buyer
        .session()
        .decode_result(delivered, &snapshot(p), request)
        .unwrap();
    assert!(evidence.upstream_id.is_none() && evidence.reported_usage.is_none());
    let receipt = p
        .executor
        .sign_terminal_receipt(&provider_signer, &p.record.invocation, p.record.attempt)
        .await
        .unwrap();
    let delivered = receive(provider, buyer, Message::Receipt { value: receipt }).await;
    let Message::Receipt { value: receipt } = delivered.message() else {
        panic!("receipt expected")
    };
    let mut altered = receipt.clone();
    altered.draft.result_commitment = attempts::ResultCommitment::OwnedV1;
    assert!(
        b.approve_receipt(
            &p.verifier(),
            altered,
            snapshot(p),
            request.to_vec(),
            serde_json::from_value(serde_json::to_value(&evidence).unwrap()).unwrap(),
            false,
            2
        )
        .await
        .is_err(),
        "changing the commitment selector cannot reinterpret a provider signature"
    );
    b.approve_receipt(
        &p.verifier(),
        receipt.clone(),
        snapshot(p),
        request.to_vec(),
        evidence,
        false,
        2,
    )
    .await
    .unwrap();
    let acknowledgment = b.sign_approved(&buyer_signer, key(p)).await.unwrap();
    let received = receive(buyer, provider, Message::Acknowledge { acknowledgment }).await;
    assert!(provider
        .session()
        .acknowledge(received, &p.executor, p.record.attempt)
        .await
        .unwrap());
}

#[tokio::test]
async fn authenticated_exchange_delivers_all_json_endpoints_and_rails_then_replays_without_inference(
) {
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
            let (_, request, mut response) =
                cases().into_iter().find(|case| case.0 == endpoint).unwrap();
            response["id"] = json!("private-upstream-identifier");
            let backend = backend(200, response, Duration::ZERO).await;
            let mut p = paid(endpoint, rail, &backend.base, &request, false).await;
            let bridge = Bridge::start(
                &p.authorization.terms.buyer_pubkey,
                &p.authorization.terms.offer.provider_pubkey,
            )
            .await;
            let (mut buyer, mut provider) = channels(&p, &bridge, 8 * 1024 * 1024).await;
            let session = provider.session().clone();
            let message = receive(
                &mut buyer,
                &mut provider,
                Message::Execute {
                    request: serde_json::from_slice(&request).unwrap(),
                    streaming: false,
                },
            )
            .await;
            let result = session
                .execute_json(message, &p.executor, &Cancellation::default())
                .await
                .unwrap();
            assert_eq!(
                result.reply.upstream_id.as_ref().unwrap().as_str(),
                "private-upstream-identifier"
            );
            let body = result.reply.body;
            let request_status = receive(&mut buyer, &mut provider, Message::Status).await;
            assert_eq!(
                session.status(request_status, &p.executor).await.unwrap().0,
                exchange::PublicState::AwaitingReceipt
            );
            settle(&mut p, &mut buyer, &mut provider, &request, &body).await;
            let received = receive(
                &mut buyer,
                &mut provider,
                Message::Execute {
                    request: serde_json::from_slice(&request).unwrap(),
                    streaming: false,
                },
            )
            .await;
            assert_eq!(
                session
                    .execute_json(received, &p.executor, &Cancellation::default())
                    .await
                    .unwrap()
                    .reply
                    .body,
                body
            );
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            let received = receive(&mut buyer, &mut provider, Message::Status).await;
            let (state, replay) = session.status(received, &p.executor).await.unwrap();
            assert_eq!(state, exchange::PublicState::Settled);
            assert_eq!(replay, Some(body));
            assert!(p.authority.lease(&p.lease).unwrap().is_none());
            use base64::Engine;
            let frames = bridge.frames.lock().await;
            let wire = frames
                .iter()
                .flat_map(|frame| {
                    base64::engine::general_purpose::STANDARD
                        .decode(frame["frame"]["data"].as_str().unwrap())
                        .unwrap()
                })
                .collect::<Vec<_>>();
            assert!(!String::from_utf8(wire)
                .unwrap()
                .contains("private-upstream-identifier"));
            drop(frames);
            p.peer.stop().await;
        }
    }
}

#[tokio::test]
async fn authenticated_exchange_rejects_foreign_identity_terms_fragments_and_replayed_frames() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let p = paid(
        ProxyEndpoint::Chat,
        ProxyRail::Tnk,
        &backend.base,
        &chat(),
        false,
    )
    .await;
    let mut wrong = p.peer.identity.clone();
    wrong.controller_pubkey = d(919);
    assert!(Session::new(p.authorization.clone(), &wrong, Role::Provider).is_err());
    let mut auth = p.authorization.clone();
    auth.terms.max_total_spend_au += 1;
    assert!(Session::new(auth, &p.peer.identity, Role::Provider).is_err());
    for attack in [
        "remote",
        "terms",
        "offset",
        "sequence",
        "size",
        "digest",
        "extra",
        "lineage",
        "duplicate",
    ] {
        let bridge = Bridge::start(
            &p.authorization.terms.buyer_pubkey,
            &p.authorization.terms.offer.provider_pubkey,
        )
        .await;
        let (mut buyer, mut provider) = channels(&p, &bridge, 1024 * 1024).await;
        *bridge.attack.lock().await = Some(attack);
        let (send, got) = tokio::join!(
            buyer.send(&Message::Status),
            provider.receive(Some(Duration::from_secs(2)))
        );
        send.unwrap();
        if attack == "duplicate" {
            got.unwrap();
            assert!(provider
                .receive(Some(Duration::from_secs(2)))
                .await
                .is_err());
        } else {
            assert!(got.is_err(), "{attack}");
        }
        assert!(matches!(
            provider.receive(Some(Duration::from_millis(1))).await,
            Err(exchange::Error::Interrupted)
        ));
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    p.peer.stop().await;
}

#[tokio::test]
async fn authenticated_exchange_chunks_large_payloads_and_recovers_after_interrupted_read() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let p = paid(
        ProxyEndpoint::Chat,
        ProxyRail::Tap,
        &backend.base,
        &chat(),
        false,
    )
    .await;
    let bridge = Bridge::start(
        &p.authorization.terms.buyer_pubkey,
        &p.authorization.terms.offer.provider_pubkey,
    )
    .await;
    let (mut buyer, mut provider) = channels(&p, &bridge, 4 * 1024 * 1024).await;
    let payload = json!({"text":"a".repeat(3*1024*1024)});
    let received = receive(
        &mut provider,
        &mut buyer,
        Message::Result {
            response: payload.clone(),
        },
    )
    .await;
    assert!(matches!(received.message(),Message::Result{response} if response==&payload));
    assert!(bridge.frames.lock().await.len() > 64);
    assert!(bridge
        .frames
        .lock()
        .await
        .iter()
        .all(|v| v.to_string().len() < 64 * 1024));
    assert!(
        provider.send(&Message::Status).await.is_err(),
        "wrong direction"
    );
    let read = buyer.receive(None);
    assert!(tokio::time::timeout(Duration::from_millis(20), read)
        .await
        .is_err());
    assert!(matches!(
        buyer.receive(None).await,
        Err(exchange::Error::Interrupted)
    ));
    buyer.close().await.unwrap();
    provider.close().await.unwrap();
    let (mut buyer, mut provider) = channels(&p, &bridge, 1024 * 1024).await;
    let received = receive(&mut buyer, &mut provider, Message::Cancel).await;
    let cancel = Cancellation::default();
    provider
        .session()
        .cancel(received, &p.executor, &cancel)
        .await
        .unwrap();
    assert!(cancel.is_cancelled());
    assert!(p.authority.lease(&p.lease).unwrap().is_none());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert!(
        p.journal
            .get(&p.record.invocation)
            .unwrap()
            .unwrap()
            .cancellation_requested
    );
    p.peer.stop().await;
}

#[tokio::test]
async fn authenticated_exchange_streams_all_supported_endpoints_then_settles_received_results() {
    let fixtures = [
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
    ];
    for (endpoint, request, answer) in fixtures {
        let backend = backend_raw(200, answer, "text/event-stream", Duration::ZERO).await;
        let mut p = paid(endpoint, ProxyRail::Tap, &backend.base, &request, true).await;
        let bridge = Bridge::start(
            &p.authorization.terms.buyer_pubkey,
            &p.authorization.terms.offer.provider_pubkey,
        )
        .await;
        let (mut buyer, mut provider) = channels(&p, &bridge, 1024 * 1024).await;
        let command = receive(
            &mut buyer,
            &mut provider,
            Message::Execute {
                request: serde_json::from_slice(&request).unwrap(),
                streaming: true,
            },
        )
        .await;
        let session = provider.session().clone();
        let provider = Arc::new(tokio::sync::Mutex::new(provider));
        let cancellation = Cancellation::default();
        let send = async {
            let output = session
                .execute_stream(command, &p.executor, &cancellation, |event| {
                    let provider = provider.clone();
                    async move {
                        provider
                            .lock()
                            .await
                            .send(&Message::Stream { event })
                            .await
                            .map_err(|_| ())
                    }
                })
                .await
                .unwrap();
            provider
                .lock()
                .await
                .send(&Message::State {
                    state: exchange::PublicState::AwaitingReceipt,
                })
                .await
                .unwrap();
            output
        };
        let receive_stream = async {
            let mut count = 0;
            loop {
                let got = buyer.receive(Some(Duration::from_secs(10))).await.unwrap();
                match got.message() {
                    Message::Stream { event } => {
                        assert!(event.is_object());
                        count += 1;
                    }
                    Message::State {
                        state: exchange::PublicState::AwaitingReceipt,
                    } => break,
                    _ => panic!("unexpected stream message"),
                }
            }
            assert!(count > 0);
        };
        let (output, ()) = tokio::join!(send, receive_stream);
        let mut provider = Arc::try_unwrap(provider).ok().unwrap().into_inner();
        settle(
            &mut p,
            &mut buyer,
            &mut provider,
            &request,
            &output.reply.body,
        )
        .await;
        let command = receive(
            &mut buyer,
            &mut provider,
            Message::Execute {
                request: serde_json::from_slice(&request).unwrap(),
                streaming: true,
            },
        )
        .await;
        assert!(matches!(
            session
                .execute_stream(command, &p.executor, &cancellation, |_| async { Ok(()) })
                .await,
            Err(exchange::Error::Execution(Error::ExistingResult))
        ));
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        assert!(p.authority.lease(&p.lease).unwrap().is_none());
        p.peer.stop().await;
    }
}

#[tokio::test]
async fn authenticated_exchange_waives_from_public_evidence_without_private_metadata() {
    let backend = backend(200, terminal_response("refused"), Duration::ZERO).await;
    let mut p = paid(
        ProxyEndpoint::Chat,
        ProxyRail::Fiat,
        &backend.base,
        &chat(),
        false,
    )
    .await;
    let bridge = Bridge::start(
        &p.authorization.terms.buyer_pubkey,
        &p.authorization.terms.offer.provider_pubkey,
    )
    .await;
    let (mut buyer, mut provider) = channels(&p, &bridge, 1024 * 1024).await;
    let command = receive(
        &mut buyer,
        &mut provider,
        Message::Execute {
            request: serde_json::from_slice(&chat()).unwrap(),
            streaming: false,
        },
    )
    .await;
    let output = provider
        .session()
        .execute_json(command, &p.executor, &Cancellation::default())
        .await
        .unwrap();
    let delivered = receive(
        &mut provider,
        &mut buyer,
        Message::Result {
            response: output.reply.body,
        },
    )
    .await;
    let evidence = buyer
        .session()
        .decode_result(delivered, &snapshot(&p), &chat())
        .unwrap();
    let b = buyer_recovery(&p);
    b.refresh(&p.authorization, 1).await.unwrap();
    let (provider_signer, buyer_signer) = authorities(&mut p).await;
    let value = p
        .executor
        .sign_waiver(&provider_signer, &p.record.invocation, p.record.attempt)
        .await
        .unwrap();
    let delivered = receive(&mut provider, &mut buyer, Message::Waiver { value }).await;
    let Message::Waiver { value } = delivered.message() else {
        panic!("waiver expected")
    };
    b.approve_waiver(value.clone(), chat(), Some(evidence), false, 2)
        .await
        .unwrap();
    let acknowledgment = b.sign_approved(&buyer_signer, key(&p)).await.unwrap();
    let delivered = receive(
        &mut buyer,
        &mut provider,
        Message::Acknowledge { acknowledgment },
    )
    .await;
    assert!(provider
        .session()
        .acknowledge(delivered, &p.executor, p.record.attempt)
        .await
        .unwrap());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    p.peer.stop().await;
}

#[tokio::test]
async fn authenticated_exchange_cancel_after_dispatch_keeps_unknown_capacity_and_cannot_retry() {
    let backend = backend(200, answer(), Duration::from_secs(2)).await;
    let p = paid(
        ProxyEndpoint::Chat,
        ProxyRail::Tnk,
        &backend.base,
        &chat(),
        false,
    )
    .await;
    let bridge = Bridge::start(
        &p.authorization.terms.buyer_pubkey,
        &p.authorization.terms.offer.provider_pubkey,
    )
    .await;
    let (mut buyer, mut provider) = channels(&p, &bridge, 1024 * 1024).await;
    let session = provider.session().clone();
    let received = receive(
        &mut buyer,
        &mut provider,
        Message::Execute {
            request: serde_json::from_slice(&chat()).unwrap(),
            streaming: false,
        },
    )
    .await;
    let cancel = Cancellation::default();
    let execute = session.execute_json(received, &p.executor, &cancel);
    let stop = async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while backend.calls.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let received = receive(&mut buyer, &mut provider, Message::Cancel).await;
        session
            .cancel(received, &p.executor, &cancel)
            .await
            .unwrap();
    };
    let (result, ()) = tokio::join!(execute, stop);
    assert!(result.is_err());
    let received = receive(&mut buyer, &mut provider, Message::Status).await;
    let (state, output) = session.status(received, &p.executor).await.unwrap();
    assert_eq!(state, exchange::PublicState::CancelRequested);
    assert!(output.is_none());
    assert!(p.authority.lease(&p.lease).unwrap().is_some());
    let received = receive(
        &mut buyer,
        &mut provider,
        Message::Execute {
            request: serde_json::from_slice(&chat()).unwrap(),
            streaming: false,
        },
    )
    .await;
    assert!(session
        .execute_json(received, &p.executor, &Cancellation::default())
        .await
        .is_err());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert!(p
        .executor
        .prepare_waiver(&p.record.invocation, p.record.attempt)
        .await
        .is_err());
    p.peer.stop().await;
}
