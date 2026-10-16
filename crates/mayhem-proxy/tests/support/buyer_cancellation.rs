use super::*;

// Hold the terminal response until the authenticated cancellation is durable.
// This makes cancellation independent of scheduler speed and financial I/O.
async fn gated_backend(streaming: bool, terminal: bool) -> (Backend, Arc<Notify>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1/", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let bodies = Arc::new(Mutex::new(vec![]));
    let recorded = bodies.clone();
    let release = Arc::new(Notify::new());
    let wait = release.clone();
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = vec![];
        let mut buf = [0; 1024];
        let boundary = loop {
            let n = socket.read(&mut buf).await.unwrap();
            assert!(n > 0);
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
            assert!(n > 0);
            bytes.extend_from_slice(&buf[..n]);
        }
        recorded
            .lock()
            .unwrap()
            .push(serde_json::from_slice(&bytes[boundary..boundary + length]).unwrap());
        count.fetch_add(1, Ordering::SeqCst);
        wait.notified().await;
        let (body, content_type) = if streaming {
            (
                sse(
                    &[delta(
                        "hello",
                        if terminal { json!("stop") } else { Value::Null },
                    )],
                    terminal,
                ),
                "text/event-stream",
            )
        } else {
            (
                serde_json::to_vec(&if terminal { answer() } else { json!({}) }).unwrap(),
                "application/json",
            )
        };
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        socket.write_all(headers.as_bytes()).await.unwrap();
        socket.write_all(&body).await.unwrap();
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
async fn buyer_controller_cancelled_json_and_stream_recover_original_receipt_on_all_rails() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for streaming in [false, true] {
            let (backend, release) = gated_backend(streaming, true).await;
            let bytes = if streaming { stream_request() } else { chat() };
            let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
            let policy = json!({"schema_version":1,"lane":"proxy","payable_outcomes":["cancelled","complete"],"allow_checkpoints":false});
            let mut peer = Peer::start(rail, &f, &bytes, streaming, Some(policy)).await;
            let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
            let (provider, _) = observed_server(&s, &f, &peer);
            let ctx = context(&peer);
            let invocation = ctx.invocation().unwrap();
            let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
            let (buyer, negotiation, _) = buyer_controller(&s, &f, &peer, &bridge, 1);
            let mut listener = listener(&bridge, &peer).await;
            let gate = Gate::allow();
            let request = make_request(&s, &f, &peer, &bytes, gate.clone());
            let id = request.identity();
            let (stop, rx) = watch::channel(false);
            let active = buyer.clone();
            let task = tokio::spawn(async move {
                if streaming {
                    let (send, _receive) = buyer::stream_channel(buyer::StreamLimits {
                        queued_events: 1,
                        queued_bytes: 16 * 1024,
                        event_bytes: 16 * 1024,
                        total_events: 1024,
                        total_bytes: 512 * 1024,
                    })
                    .unwrap();
                    active.execute_stream(request, send, rx).await
                } else {
                    active.execute(request, rx).await
                }
            });
            let handle = provider
                .accept(listener.next(Duration::from_secs(5)).await.unwrap())
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                while backend.calls.load(Ordering::SeqCst) == 0 {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .unwrap();
            stop.send_replace(true);
            let failure = error(task.await.unwrap());
            assert_eq!(failure.code, buyer::Code::Stopped);
            assert!(failure.recovery_required);
            tokio::time::timeout(Duration::from_secs(5), async {
                while !s
                    .journal
                    .get(&invocation)
                    .unwrap()
                    .unwrap()
                    .cancellation_requested
                {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .unwrap();
            occupied(&s, 1);
            // A status-only recovery reports live cancelled work promptly. It
            // must not monopolize a control slot or interrupt the original drain.
            let (_resume, rx) = watch::channel(false);
            let (pending, recovery_handle) =
                tokio::join!(buyer.recover(id.clone(), gate.clone(), rx), async {
                    provider
                        .accept(listener.next(Duration::from_secs(5)).await.unwrap())
                        .unwrap()
                });
            assert!(matches!(pending.unwrap(), buyer::Outcome::Pending {
                identity, state: exchange::PublicState::Running,
            } if identity == id));
            assert_eq!(
                recovery_handle.wait().await.unwrap(),
                serving::End::ControlIdle
            );
            occupied(&s, 1);
            release.notify_one();
            let _ = tokio::time::timeout(Duration::from_secs(5), handle.wait())
                .await
                .unwrap();
            occupied(&s, 0);
            assert!(gate.outputs.lock().unwrap().is_empty());
            assert_eq!(peer.command("status").await["publications"], 1);
            let saved = negotiation
                .lookup(id.billing_id.clone(), id.billing_attempt)
                .await
                .unwrap()
                .unwrap();
            let original = saved.authorization().unwrap().clone();
            let (_resume, rx) = watch::channel(false);
            let (result, provider_result) =
                tokio::join!(buyer.recover(id.clone(), gate.clone(), rx), async {
                    provider
                        .accept(listener.next(Duration::from_secs(5)).await.unwrap())
                        .unwrap()
                        .wait()
                        .await
                });
            assert_eq!(provider_result.unwrap(), serving::End::Settled);
            let buyer::Outcome::Completed {
                identity,
                response,
                authorization,
                settlement: FinancialOutcome::Paid { receipt },
            } = result.unwrap()
            else {
                panic!("original cancelled outcome must reconcile")
            };
            assert_eq!(identity, id);
            assert_eq!(authorization, original);
            assert_eq!(
                serde_json::to_value(receipt.body.outcome).unwrap(),
                json!("cancelled")
            );
            assert_eq!(gate.outputs.lock().unwrap().as_slice(), &[response]);
            assert_eq!(gate.terms.lock().unwrap().len(), 1);
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            assert_eq!(peer.command("status").await["publications"], 2);
            buyer.shutdown().await.unwrap();
            peer.stop().await;
        }
    }
}

#[tokio::test]
async fn buyer_controller_orphan_expiry_uses_original_policy_and_canonical_1125_1126_boundary() {
    for (rail, streaming, opt_in) in [
        (ProxyRail::Fiat, false, true),
        (ProxyRail::Tnk, true, true),
        (ProxyRail::Tap, false, true),
        (ProxyRail::Fiat, false, false),
    ] {
        let (backend, release) = gated_backend(streaming, false).await;
        let bytes = if streaming { stream_request() } else { chat() };
        let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut policy = json!({"schema_version":1,"lane":"proxy","payable_outcomes":["cancelled","complete","partial","refused"],"allow_checkpoints":false});
        if opt_in {
            policy["hold_expiry"] = json!("release_unfinalized_and_block_retry");
        }
        let mut peer = Peer::start(rail, &f, &bytes, streaming, Some(policy)).await;
        peer.command(r#"{"epoch":1094}"#).await;
        let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
        let (provider, _) = observed_server(&s, &f, &peer);
        let ctx = context(&peer);
        let invocation = ctx.invocation().unwrap();
        let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
        let (buyer, negotiation, recovery) = buyer_controller(&s, &f, &peer, &bridge, 1);
        let mut listener = listener(&bridge, &peer).await;
        let gate = Gate::allow();
        let mut request = make_request(&s, &f, &peer, &bytes, gate.clone());
        request.authorization.lifetimes.reservation_epochs = 24;
        request.authorization.lifetimes.receipt_grace_epochs = 6;
        let id = request.identity();
        let (stop, rx) = watch::channel(false);
        let active = buyer.clone();
        let task = tokio::spawn(async move {
            if streaming {
                let (send, _receive) = buyer::stream_channel(buyer::StreamLimits {
                    queued_events: 1,
                    queued_bytes: 16 * 1024,
                    event_bytes: 16 * 1024,
                    total_events: 1024,
                    total_bytes: 512 * 1024,
                })
                .unwrap();
                active.execute_stream(request, send, rx).await
            } else {
                active.execute(request, rx).await
            }
        });
        let handle = provider
            .accept(listener.next(Duration::from_secs(5)).await.unwrap())
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while backend.calls.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        stop.send_replace(true);
        assert_eq!(error(task.await.unwrap()).code, buyer::Code::Stopped);
        tokio::time::timeout(Duration::from_secs(5), async {
            while !s
                .journal
                .get(&invocation)
                .unwrap()
                .unwrap()
                .cancellation_requested
            {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        release.notify_one();
        let _ = handle.wait().await;
        let original = negotiation
            .lookup(id.billing_id.clone(), id.billing_attempt)
            .await
            .unwrap()
            .unwrap()
            .authorization()
            .unwrap()
            .clone();
        assert_eq!(original.terms.billing_epoch, 1095);
        assert_eq!(original.terms.reservation_expires_after_epoch, 1119);
        assert_eq!(original.terms.reservation_receipt_grace_epochs, 6);
        let key = Digest::new(original.terms.digest().unwrap()).unwrap();
        let before = peer.command("state").await;
        let (_resume, resume) = watch::channel(false);
        for epoch in [1125, 1126] {
            peer.command(&json!({"epoch":epoch}).to_string()).await;
            if epoch == 1126 && opt_in {
                // No provider accepts this recovery connection: eligible
                // financial expiry must work without a terminal response.
                let result = tokio::time::timeout(
                    Duration::from_secs(5),
                    buyer.recover(id.clone(), gate.clone(), resume.clone()),
                )
                .await
                .unwrap()
                .unwrap();
                let buyer::Outcome::Closed {
                    authorization,
                    settlement: FinancialOutcome::ExpiredUnknown { expiry },
                    ..
                } = result
                else {
                    panic!("canonical unresolved expiry required")
                };
                assert_eq!(authorization, original);
                assert_eq!(expiry.body.observed_epoch, 1126);
                let retained = recovery.recover(key.clone()).await.unwrap();
                assert_eq!(retained.signed, Some(expiry.clone()));
                assert!(matches!(
                    retained.confirmed,
                    Some(FinancialOutcome::ExpiredUnknown { .. })
                ));
                let after = peer.command("state").await;
                assert_eq!(after["summary"]["reserved_au"], "50");
                assert_eq!(before["balance"], after["balance"]);
                assert_eq!(after["billing"]["reserved_au"], "0");
                assert_eq!(after["billing"]["retry_blocked"], true);
                let again = buyer
                    .recover(id.clone(), gate.clone(), resume.clone())
                    .await
                    .unwrap();
                assert!(matches!(again, buyer::Outcome::Closed {
                    settlement: FinancialOutcome::ExpiredUnknown { expiry: repeated }, ..
                } if repeated == expiry));
                assert_eq!(peer.command("status").await["publications"], 2);
            } else {
                let (result, handle) = tokio::join!(
                    buyer.recover(id.clone(), gate.clone(), resume.clone()),
                    async {
                        provider
                            .accept(listener.next(Duration::from_secs(5)).await.unwrap())
                            .unwrap()
                    }
                );
                assert!(matches!(result.unwrap(), buyer::Outcome::Pending {
                    identity, state: exchange::PublicState::OutcomeUnknown,
                } if identity == id));
                let _ = handle.wait().await;
                let retained = recovery.recover(key.clone()).await.unwrap();
                assert!(
                    retained.draft.is_none()
                        && retained.signed.is_none()
                        && retained.confirmed.is_none()
                );
                assert_eq!(peer.command("status").await["publications"], 1);
                assert_eq!(peer.command("state").await, before);
            }
        }
        let retained = s.journal.get(&invocation).unwrap().unwrap();
        assert_eq!(retained.phase, Phase::Dispatched);
        assert!(retained.cancellation_requested && retained.closure.is_none());
        assert!(s
            .journal
            .recover(&invocation, retained.attempt)
            .unwrap()
            .result
            .is_none());
        occupied(&s, 1);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        assert_eq!(gate.terms.lock().unwrap().len(), 1);
        assert!(gate.outputs.lock().unwrap().is_empty());
        buyer.shutdown().await.unwrap();
        peer.stop().await;
    }
}
