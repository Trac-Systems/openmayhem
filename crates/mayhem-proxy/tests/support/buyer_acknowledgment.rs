use super::*;

struct ReceiptGate {
    gate: Arc<Gate>,
    reached: Notify,
    release: Notify,
}
impl ReceiptGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            gate: Gate::allow(),
            reached: Notify::new(),
            release: Notify::new(),
        })
    }
}
impl buyer::AuthorizationGate for ReceiptGate {
    fn retain_non_admission<'a>(
        &'a self,
        proof: &'a buyer::NonAdmission,
    ) -> Pin<Box<dyn Future<Output = Result<(), buyer::GateError>> + Send + 'a>> {
        self.gate.retain_non_admission(proof)
    }
    fn authorize<'a>(
        &'a self,
        purchase: &'a financial::quote::PreparedPurchase,
    ) -> Pin<Box<dyn Future<Output = Result<(), buyer::GateError>> + Send + 'a>> {
        self.gate.authorize(purchase)
    }
    fn retain_verified_output<'a>(
        &'a self,
        output: buyer::VerifiedOutput<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<(), buyer::GateError>> + Send + 'a>> {
        Box::pin(async move {
            self.gate.retain_verified_output(output).await?;
            self.reached.notify_one();
            self.release.notified().await;
            Ok(())
        })
    }
}

async fn submitted(peer: &mut Peer, expected: u64) {
    for _ in 0..100 {
        if peer.command("status").await["submissions"]
            .as_u64()
            .unwrap()
            >= expected
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("expected the original signed acknowledgment to reach publication");
}

async fn paid_case(rail: ProxyRail, streaming: bool, fault: &'static str) {
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
    let (provider, _) = observed_server_with_wait(&s, &f, &peer, Duration::from_secs(2));
    let ctx = context(&peer);
    let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
    if matches!(
        fault,
        "ack_lost" | "ack_drop" | "settled_drop" | "settled_delay"
    ) {
        *bridge.attack.lock().await = Some(fault);
    } else if matches!(fault, "mismatch" | "different") {
        *bridge.attack.lock().await = Some("ack_drop");
    }
    let wait = if matches!(fault, "stall" | "ack_drop") {
        3
    } else {
        7
    };
    let (buyer, _, recovery) =
        buyer_controller_with_wait(&s, &f, &peer, &bridge, 1, Duration::from_secs(wait));
    let mut listener = listener(&bridge, &peer).await;
    let gate = ReceiptGate::new();
    let mut request = make_request(&s, &f, &peer, &bytes, gate.gate.clone());
    request.gate = gate.clone();
    let id = request.identity();
    let (send, receive) = if streaming {
        let (s, r) =
            buyer::stream_channel(buyer::StreamLimits::for_response_bytes(512 * 1024)).unwrap();
        (Some(s), Some(r))
    } else {
        (None, None)
    };
    let (stop, rx) = watch::channel(false);
    let (result, handle, released, events) = tokio::join!(
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
        },
        async {
            gate.reached.notified().await;
            match fault {
                "late" | "stall" | "cancel" => {
                    peer.command("publish_pending").await;
                }
                _ => (),
            }
            let released = std::time::Instant::now();
            gate.release.notify_one();
            if matches!(fault, "mismatch" | "different") {
                for attempt in 0..100 {
                    if bridge.attack.lock().await.is_none() {
                        break;
                    }
                    assert!(attempt < 99, "signed acknowledgment must be attempted");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                if fault == "mismatch" {
                    peer.command("signature").await;
                } else {
                    // A valid but different dual-signed terminal receipt is
                    // financial evidence, not proof of this exact approval.
                    let key =
                        Digest::new(gate.gate.terms.lock().unwrap()[0].digest().unwrap()).unwrap();
                    let financial::recovery::SignedAcknowledgment::Receipt { mut receipt } =
                        recovery.acknowledgment(key.clone()).await.unwrap().unwrap()
                    else {
                        panic!("receipt required")
                    };
                    receipt.body.at_ms += 1;
                    let signatures = peer
                        .command(&json!({"sign_receipt": receipt.body}).to_string())
                        .await;
                    receipt.buyer_sig = signatures["buyer_sig"].as_str().unwrap().to_owned();
                    receipt.provider_sig = signatures["provider_sig"].as_str().unwrap().to_owned();
                    let saved = recovery.recover(key).await.unwrap();
                    peer.client
                        .submit_receipt(&saved.authorization, &saved.policy, &receipt)
                        .await
                        .unwrap();
                }
            }
            if matches!(fault, "late" | "cancel") {
                submitted(&mut peer, 2).await;
                if fault == "cancel" {
                    stop.send_replace(true);
                } else {
                    tokio::time::sleep(Duration::from_millis(1300)).await;
                    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
                    peer.command("flush_publication").await;
                }
            }
            released
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
    let elapsed = released.elapsed();
    let expected_failure = matches!(
        fault,
        "stall" | "ack_drop" | "cancel" | "mismatch" | "different"
    );
    if expected_failure {
        let failure = error(result);
        assert_eq!(failure.stage, buyer::Stage::Acknowledging);
        assert!(failure.recovery_required);
        if fault == "cancel" {
            assert_eq!(failure.code, buyer::Code::Stopped);
        } else if fault == "different" {
            assert_eq!(failure.code, buyer::Code::Verification);
            assert!(elapsed < Duration::from_secs(2));
        } else if fault == "mismatch" {
            assert_eq!(failure.financial_code, Some("proxy_financial_invalid"));
            assert!(
                elapsed < Duration::from_secs(2),
                "invalid proof must not use the grace period"
            );
        } else {
            assert_eq!(failure.code, buyer::Code::Unconfirmed);
            assert!(elapsed < Duration::from_secs(4));
        }
    } else {
        let buyer::Outcome::Completed {
            identity,
            response,
            authorization,
            settlement,
            observation,
        } = result.unwrap()
        else {
            panic!("verified same purchase must complete")
        };
        assert_eq!(identity, id);
        assert_eq!(observation.accepted().authorization, authorization);
        assert_eq!(
            observation.financial_outcome().unwrap(),
            Some(settlement.clone())
        );
        let FinancialOutcome::Paid { receipt } = settlement else {
            panic!("exact final receipt required")
        };
        assert!(receipt.body.final_receipt);
        assert_eq!(
            receipt.body.accepted_terms,
            authorization.terms.digest().unwrap()
        );
        let financial::recovery::SignedAcknowledgment::Receipt { receipt: approved } = recovery
            .acknowledgment(Digest::new(&receipt.body.accepted_terms).unwrap())
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("retained receipt required")
        };
        assert_eq!(receipt, approved);
        assert_eq!(gate.gate.outputs.lock().unwrap().as_slice(), [response]);
        assert_eq!(peer.command("status").await["publications"], 2);
        if fault == "healthy" {
            assert!(
                elapsed < Duration::from_millis(900),
                "Settled must wake the paced proof check: {elapsed:?}"
            );
        }
        if fault == "settled_delay" {
            assert!(
                elapsed < Duration::from_secs(2),
                "canonical proof must finish without the delayed wire response"
            );
        }
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert_eq!(gate.gate.terms.lock().unwrap().len(), 1);
    assert_eq!(gate.gate.outputs.lock().unwrap().len(), 1);
    assert_eq!(events > 0, streaming);
    if matches!(
        fault,
        "ack_lost" | "ack_drop" | "settled_drop" | "settled_delay"
    ) {
        assert!(
            bridge.attack.lock().await.is_none(),
            "targeted wire fault must run"
        );
    }
    if fault != "mismatch" {
        assert_eq!(
            peer.command("status").await["submissions"],
            if fault == "ack_drop" { 1 } else { 2 }
        );
    }
    // Approval survives any transport/timeout failure; no new authorization or
    // automatic Execute is created while its canonical outcome is unknown.
    let key = Digest::new(gate.gate.terms.lock().unwrap()[0].digest().unwrap()).unwrap();
    assert!(recovery.recover(key).await.unwrap().outcome_signed);
    let _ = tokio::time::timeout(Duration::from_secs(4), handle.wait())
        .await
        .unwrap();
    buyer.shutdown().await.unwrap();
    peer.stop().await;
}

#[tokio::test]
async fn buyer_acknowledgment_lost_transport_completes_exact_purchase_on_every_rail_and_stream_mode(
) {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for streaming in [false, true] {
            for fault in ["ack_lost", "settled_drop"] {
                paid_case(rail, streaming, fault).await;
            }
        }
    }
}

#[tokio::test]
async fn buyer_acknowledgment_awaiting_receipt_and_late_canonical_proof_complete_without_redispatch(
) {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for streaming in [false, true] {
            paid_case(rail, streaming, "late").await;
        }
    }
}

#[tokio::test]
async fn buyer_acknowledgment_stalled_missing_or_invalid_proof_never_completes() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for fault in ["ack_drop", "stall", "mismatch", "different", "cancel"] {
            paid_case(rail, true, fault).await;
        }
    }
}

#[tokio::test]
async fn buyer_acknowledgment_exact_waiver_survives_lost_settled_reply_on_all_rails() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        let backend = backend(
            503,
            crate::probe_execution::vllm_refusal(false),
            Duration::ZERO,
        )
        .await;
        let bytes = chat();
        let f = Fixture::with_profile(
            &backend.base,
            ProxyEndpoint::Chat,
            128 * 1024 * 1024,
            "vllm_admission_v1",
        );
        let mut peer = Peer::start(rail, &f, &bytes, false, None).await;
        let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
        let (provider, _) = observed_server_with_wait(&s, &f, &peer, Duration::from_secs(2));
        let ctx = context(&peer);
        let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
        *bridge.attack.lock().await = Some("settled_drop");
        let (buyer, _, recovery) = buyer_controller(&s, &f, &peer, &bridge, 1);
        let mut listener = listener(&bridge, &peer).await;
        let gate = Gate::allow();
        let request = make_request(&s, &f, &peer, &bytes, gate.clone());
        let id = request.identity();
        let (_stop, rx) = watch::channel(false);
        let (result, handle) = tokio::join!(buyer.execute(request, rx), async {
            provider
                .accept(listener.next(Duration::from_secs(5)).await.unwrap())
                .unwrap()
        });
        let buyer::Outcome::Closed {
            identity,
            authorization,
            settlement,
            observation,
        } = result.unwrap()
        else {
            panic!("signed waiver required")
        };
        assert_eq!(identity, id);
        assert_eq!(observation.accepted().authorization, authorization);
        assert_eq!(
            observation.financial_outcome().unwrap(),
            Some(settlement.clone())
        );
        let FinancialOutcome::Waived { closure } = settlement else {
            panic!("waiver required")
        };
        let key = Digest::new(authorization.terms.digest().unwrap()).unwrap();
        let financial::recovery::SignedAcknowledgment::Waiver { closure: approved } =
            recovery.acknowledgment(key).await.unwrap().unwrap()
        else {
            panic!("retained waiver required")
        };
        assert_eq!(closure, approved);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        assert_eq!(gate.terms.lock().unwrap().len(), 1);
        assert!(gate.outputs.lock().unwrap().is_empty());
        assert!(bridge.attack.lock().await.is_none());
        assert_eq!(peer.command("status").await["submissions"], 2);
        let _ = tokio::time::timeout(Duration::from_secs(4), handle.wait())
            .await
            .unwrap();
        buyer.shutdown().await.unwrap();
        peer.stop().await;
    }
}

#[tokio::test]
async fn buyer_acknowledgment_canonical_completion_does_not_wait_for_lost_control_reply_or_poll_interval(
) {
    paid_case(ProxyRail::Tnk, false, "healthy").await;
    paid_case(ProxyRail::Tap, true, "settled_delay").await;
}
