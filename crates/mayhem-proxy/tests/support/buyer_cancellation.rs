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

// Fail only the provider's financial observations. Buyer canonical reads remain
// available, so a recovery can reach Status without racing the injected fault.
struct ProviderFinancialFault {
    blocked: Arc<std::sync::atomic::AtomicBool>,
    failures: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}
impl Drop for ProviderFinancialFault {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl ProviderFinancialFault {
    async fn install(peer: &mut Peer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let upstream = peer.rpc_url.trim_end_matches('/').to_owned();
        let blocked = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fault = blocked.clone();
        let failures = Arc::new(AtomicUsize::new(0));
        let count = failures.clone();
        let task = tokio::spawn(async move {
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                let boundary = loop {
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert!(n > 0 && bytes.len() < 2 * 1024 * 1024);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(n) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        break n + 4;
                    }
                };
                let header = String::from_utf8(bytes[..boundary].to_vec()).unwrap();
                let path = header
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|n| n.trim().parse::<usize>().ok())
                    })
                    .unwrap();
                assert!(length < 2 * 1024 * 1024);
                while bytes.len() < boundary + length {
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                }
                let (status, body) =
                    if path == "/proxy/financial-state" && fault.load(Ordering::SeqCst) {
                        count.fetch_add(1, Ordering::SeqCst);
                        (
                            503,
                            br#"{"ok":false,"code":"proxy_financial_unavailable"}"#.to_vec(),
                        )
                    } else {
                        let response = client
                            .post(format!("{upstream}{path}"))
                            .header("content-type", "application/json")
                            .body(bytes[boundary..boundary + length].to_vec())
                            .send()
                            .await
                            .unwrap();
                        let status = response.status().as_u16();
                        let body = response.bytes().await.unwrap().to_vec();
                        assert!(body.len() < 2 * 1024 * 1024);
                        (status, body)
                    };
                let header = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                socket.write_all(header.as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
            }
        });
        peer.client = Arc::new(
            financial::Client::new(
                &base,
                mayhem_proxy::discovery::Identity {
                    network_id: peer.identity.network_id.clone(),
                    msb_bootstrap: peer.identity.msb_bootstrap.as_str().into(),
                    subnet_bootstrap: peer.identity.subnet_bootstrap.as_str().into(),
                    contract_version: peer.template.terms.contract_version,
                },
                peer.identity.controller_pubkey.as_str().into(),
                4,
            )
            .unwrap(),
        );
        Self {
            blocked,
            failures,
            task,
        }
    }
}

#[tokio::test]
async fn buyer_recovery_pending_receipt_releases_control_and_later_settles_retained_result() {
    for cancelled in [false, true] {
        pending_proof_recovery(cancelled, false).await;
    }
}

#[tokio::test]
async fn buyer_recovery_pending_waiver_releases_control_and_later_closes_without_execution() {
    pending_proof_recovery(true, true).await;
}

async fn pending_proof_recovery(cancelled: bool, waiver: bool) {
    let (backend, release) = gated_backend(false, true).await;
    let bytes = chat();
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let policy = json!({"schema_version":1,"lane":"proxy","payable_outcomes":["cancelled","complete"],"allow_checkpoints":false});
    let mut peer = Peer::start(ProxyRail::Fiat, &f, &bytes, false, Some(policy)).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let fault = ProviderFinancialFault::install(&mut peer).await;
    let (provider, _) = observed_server(&s, &f, &peer);
    let ctx = context(&peer);
    let invocation = ctx.invocation().unwrap();
    let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
    let (buyer, negotiation, recovery) = buyer_controller(&s, &f, &peer, &bridge, 1);
    let mut listener = listener(&bridge, &peer).await;
    let gate = Gate::allow();
    let request = make_request(&s, &f, &peer, &bytes, gate.clone());
    let id = request.identity();
    let (stop, rx) = watch::channel(false);
    if waiver {
        // Retain an accepted purchase before Execute, so recovery must send the
        // real Prepared -> Cancel exchange to establish nonexecution evidence.
        peer.command("financial_unavailable").await;
    }
    let active = buyer.clone();
    let task = tokio::spawn(async move { active.execute(request, rx).await });
    let handle = provider
        .accept(listener.next(Duration::from_secs(5)).await.unwrap())
        .unwrap();
    if waiver {
        let failure = error(task.await.unwrap());
        assert_eq!(failure.code, buyer::Code::Unconfirmed);
        assert_eq!(failure.stage, buyer::Stage::Accepted);
        handle.wait().await.unwrap();
        peer.command("financial_available").await;
        fault.blocked.store(true, Ordering::SeqCst);
    } else {
        tokio::time::timeout(Duration::from_secs(5), async {
            while backend.calls.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        fault.blocked.store(true, Ordering::SeqCst);
        if cancelled {
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
        } else {
            release.notify_one();
            assert!(error(task.await.unwrap()).recovery_required);
        }
        handle.disconnect();
        let _ = tokio::time::timeout(Duration::from_secs(5), handle.wait())
            .await
            .unwrap();
    }
    let mut original = s.journal.get(&invocation).unwrap().unwrap();
    let retained = s.journal.recover(&invocation, original.attempt).unwrap();
    assert_eq!(retained.result.is_some(), !waiver);
    assert!(original.closure.is_none());
    assert_eq!(backend.calls.load(Ordering::SeqCst), usize::from(!waiver));
    let finance_before = peer.command("state").await;
    let saved = negotiation
        .lookup(id.billing_id.clone(), id.billing_attempt)
        .await
        .unwrap()
        .unwrap();
    let authorization = saved.authorization().unwrap();
    let key = Digest::new(authorization.terms.digest().unwrap()).unwrap();
    // Repeat the failed proof recovery: both Prepared -> Cancel and subsequent
    // Status-only waiver reads must release their connection without settlement.
    for round in 0..2 {
        let (_resume, rx) = watch::channel(false);
        let (pending, handle) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(buyer.recover(id.clone(), gate.clone(), rx), async {
                provider
                    .accept(listener.next(Duration::from_secs(2)).await.unwrap())
                    .unwrap()
            })
        })
        .await
        .unwrap();
        assert!(matches!(pending.unwrap(), buyer::Outcome::Pending {
        identity, state: exchange::PublicState::AwaitingReceipt,
    } if identity == id));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), handle.wait())
                .await
                .unwrap()
                .unwrap(),
            serving::End::ControlIdle
        );
        if waiver && round == 0 {
            original = s.journal.get(&invocation).unwrap().unwrap();
            assert!(original.cancellation_requested);
            assert!(matches!(
                original.resolution,
                Some(attempts::Resolution::NotExecuted { .. })
            ));
            assert!(original.closure.is_none());
        }
        assert_eq!(
            s.journal.get(&invocation).unwrap().as_ref(),
            Some(&original)
        );
    }
    assert!(fault.failures.load(Ordering::SeqCst) > 0);
    assert_eq!(
        s.journal.get(&invocation).unwrap().as_ref(),
        Some(&original)
    );
    let again = s.journal.recover(&invocation, original.attempt).unwrap();
    assert_eq!(
        again.result.as_ref().map(|r| (&r.digest, &r.reply.body)),
        retained.result.as_ref().map(|r| (&r.digest, &r.reply.body))
    );
    let financial = recovery.recover(key).await.unwrap();
    assert!(
        financial.draft.is_none() && financial.signed.is_none() && financial.confirmed.is_none()
    );
    assert_eq!(peer.command("state").await, finance_before);
    assert_eq!(peer.command("status").await["publications"], 1);
    assert!(gate.outputs.lock().unwrap().is_empty());
    fault.blocked.store(false, Ordering::SeqCst);
    let (_resume, rx) = watch::channel(false);
    let (completed, end) = tokio::join!(buyer.recover(id.clone(), gate.clone(), rx), async {
        provider
            .accept(listener.next(Duration::from_secs(5)).await.unwrap())
            .unwrap()
            .wait()
            .await
    });
    assert_eq!(end.unwrap(), serving::End::Settled);
    match completed.unwrap() {
        buyer::Outcome::Completed {
            settlement: FinancialOutcome::Paid { receipt },
            ..
        } if !waiver => {
            assert_eq!(
                serde_json::to_value(receipt.body.outcome).unwrap(),
                json!(if cancelled { "cancelled" } else { "complete" })
            );
        }
        buyer::Outcome::Closed {
            settlement: FinancialOutcome::Waived { .. },
            ..
        } if waiver => {}
        _ => panic!("the original signed proof must settle"),
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), usize::from(!waiver));
    assert_eq!(peer.command("status").await["publications"], 2);
    assert_eq!(gate.terms.lock().unwrap().len(), 1);
    buyer.shutdown().await.unwrap();
    peer.stop().await;
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
