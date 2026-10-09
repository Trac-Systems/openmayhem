use super::*;

async fn transfer(
    sender: &mut exchange::Sender,
    receiver: &mut exchange::Receiver,
    message: Message,
) -> exchange::Received {
    let (sent, got) = tokio::join!(
        sender.send(&message),
        receiver.receive(Some(Duration::from_secs(5)))
    );
    sent.unwrap();
    got.unwrap()
}

#[tokio::test]
async fn duplex_all_json_endpoints_and_rails_preserve_sequences_and_replay_one_inference() {
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
            let (_, request, response) =
                cases().into_iter().find(|case| case.0 == endpoint).unwrap();
            let backend = backend(200, response, Duration::ZERO).await;
            let p = paid(endpoint, rail, &backend.base, &request, false).await;
            let bridge = Bridge::start(
                &p.authorization.terms.buyer_pubkey,
                &p.authorization.terms.offer.provider_pubkey,
            )
            .await;
            let (mut buyer, mut provider) = channels(&p, &bridge, 1024 * 1024).await;
            // A completed pre-handoff exchange cannot reset the sequence counters.
            let command = receive(&mut buyer, &mut provider, Message::Status).await;
            let (state, _) = provider
                .session()
                .status(command, &p.executor)
                .await
                .unwrap();
            receive(&mut provider, &mut buyer, Message::State { state }).await;
            let session = provider.session().clone();
            let (mut buyer_tx, mut buyer_rx) = buyer.into_duplex().unwrap();
            let (mut provider_tx, mut provider_rx) = provider.into_duplex().unwrap();
            let command = transfer(
                &mut buyer_tx,
                &mut provider_rx,
                Message::Execute {
                    request: serde_json::from_slice(&request).unwrap(),
                    streaming: false,
                },
            )
            .await;
            let output = session
                .execute_json(command, &p.executor, &Cancellation::default())
                .await
                .unwrap();
            let body = output.reply.body;
            // Opposite directions progress concurrently on each peer's same socket.
            let (received, status) = tokio::join!(
                transfer(
                    &mut provider_tx,
                    &mut buyer_rx,
                    Message::Result {
                        response: body.clone()
                    }
                ),
                transfer(&mut buyer_tx, &mut provider_rx, Message::Status)
            );
            buyer_rx
                .session()
                .decode_result(received, &snapshot(&p), &request)
                .unwrap();
            assert_eq!(
                session.status(status, &p.executor).await.unwrap().1,
                Some(body.clone())
            );
            let command = transfer(
                &mut buyer_tx,
                &mut provider_rx,
                Message::Execute {
                    request: serde_json::from_slice(&request).unwrap(),
                    streaming: false,
                },
            )
            .await;
            assert_eq!(
                session
                    .execute_json(command, &p.executor, &Cancellation::default())
                    .await
                    .unwrap()
                    .reply
                    .body,
                body
            );
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            p.peer.stop().await;
        }
    }
}

#[tokio::test]
async fn duplex_rejects_foreign_replayed_and_malformed_fragments_without_backend_dispatch() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let p = paid(
        ProxyEndpoint::Chat,
        ProxyRail::Tnk,
        &backend.base,
        &chat(),
        false,
    )
    .await;
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
        let (buyer, provider) = channels(&p, &bridge, 1024 * 1024).await;
        let (mut tx, _buyer_rx) = buyer.into_duplex().unwrap();
        let (_provider_tx, mut rx) = provider.into_duplex().unwrap();
        *bridge.attack.lock().await = Some(attack);
        let (sent, received) = tokio::join!(
            tx.send(&Message::Status),
            rx.receive(Some(Duration::from_secs(2)))
        );
        sent.unwrap();
        if attack == "duplicate" {
            received.unwrap();
            assert!(rx.receive(Some(Duration::from_secs(2))).await.is_err());
        } else {
            assert!(received.is_err(), "{attack}");
        }
        assert!(matches!(
            rx.receive(None).await,
            Err(exchange::Error::Interrupted)
        ));
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    p.peer.stop().await;
}

#[tokio::test]
async fn duplex_stream_cancel_reaches_provider_while_stream_delivery_is_pending() {
    let request = stream_request();
    let backend = backend_raw(
        200,
        sse(
            &[delta("hello", json!(null)), delta(" world", json!("stop"))],
            true,
        ),
        "text/event-stream",
        Duration::ZERO,
    )
    .await;
    let p = paid(
        ProxyEndpoint::Chat,
        ProxyRail::Tap,
        &backend.base,
        &request,
        true,
    )
    .await;
    let bridge = Bridge::start(
        &p.authorization.terms.buyer_pubkey,
        &p.authorization.terms.offer.provider_pubkey,
    )
    .await;
    let (buyer, provider) = channels(&p, &bridge, 1024 * 1024).await;
    let session = provider.session().clone();
    let (mut buyer_tx, mut buyer_rx) = buyer.into_duplex().unwrap();
    let (provider_tx, mut provider_rx) = provider.into_duplex().unwrap();
    let command = transfer(
        &mut buyer_tx,
        &mut provider_rx,
        Message::Execute {
            request: serde_json::from_slice(&request).unwrap(),
            streaming: true,
        },
    )
    .await;
    let cancel = Cancellation::default();
    let provider_tx = Arc::new(tokio::sync::Mutex::new(provider_tx));
    let resumed = Arc::new(tokio::sync::Notify::new());
    let executor = async {
        session
            .execute_stream(command, &p.executor, &cancel, |event| {
                let tx = provider_tx.clone();
                let resumed = resumed.clone();
                async move {
                    // Retain the send owner while waiting: incoming control must remain independent.
                    let mut tx = tx.lock().await;
                    tx.send(&Message::Stream { event }).await.map_err(|_| ())?;
                    resumed.notified().await;
                    Err(())
                }
            })
            .await
    };
    let control = async {
        let received = provider_rx
            .receive(Some(Duration::from_secs(10)))
            .await
            .unwrap();
        assert!(matches!(received.message(), Message::Cancel));
        session
            .cancel(received, &p.executor, &cancel)
            .await
            .unwrap();
        assert!(cancel.is_cancelled());
        resumed.notify_one();
    };
    let client = async {
        let got = buyer_rx
            .receive(Some(Duration::from_secs(10)))
            .await
            .unwrap();
        assert!(matches!(got.message(), Message::Stream { .. }));
        buyer_tx.send(&Message::Cancel).await.unwrap();
    };
    let (result, (), ()) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(executor, control, client)
    })
    .await
    .unwrap();
    assert!(result.is_err());
    assert!(
        p.journal
            .get(&p.record.invocation)
            .unwrap()
            .unwrap()
            .cancellation_requested
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    let command = transfer(
        &mut buyer_tx,
        &mut provider_rx,
        Message::Execute {
            request: serde_json::from_slice(&request).unwrap(),
            streaming: true,
        },
    )
    .await;
    assert!(session
        .execute_stream(command, &p.executor, &Cancellation::default(), |_| async {
            Ok(())
        })
        .await
        .is_err());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    p.peer.stop().await;
}
