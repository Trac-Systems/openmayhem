use super::*;
use mayhem_proxy::buyer_controller as buyer;
use mayhem_proxy::financial::recovery::FinancialOutcome;
use std::{future::Future, pin::Pin};
use tokio::sync::{watch, Notify};

struct Gate {
    terms: Mutex<Vec<mayhem_proto::proxy::finance::ProxySpendTerms>>,
    reject: bool,
    reject_output: std::sync::atomic::AtomicBool,
    outputs: Mutex<Vec<Value>>,
    non_admissions: Mutex<Vec<Digest>>,
    reject_non_admission: std::sync::atomic::AtomicBool,
    entered: Notify,
    release: Option<Arc<Notify>>,
}
impl Gate {
    fn allow() -> Arc<Self> {
        Arc::new(Self {
            terms: Mutex::new(vec![]),
            reject: false,
            reject_output: std::sync::atomic::AtomicBool::new(false),
            outputs: Mutex::new(vec![]),
            non_admissions: Mutex::new(vec![]),
            reject_non_admission: std::sync::atomic::AtomicBool::new(false),
            entered: Notify::new(),
            release: None,
        })
    }
}
impl buyer::AuthorizationGate for Gate {
    fn retain_non_admission<'a>(
        &'a self,
        proof: &'a buyer::NonAdmission,
    ) -> Pin<Box<dyn Future<Output = Result<(), buyer::GateError>> + Send + 'a>> {
        Box::pin(async move {
            assert_eq!(
                proof.identity().billing_id.as_str(),
                proof.terms().billing_id
            );
            assert_eq!(
                proof.identity().session_id.as_str(),
                proof.terms().session_id
            );
            if self.reject_non_admission.load(Ordering::SeqCst) {
                return Err(buyer::GateError::Unavailable);
            }
            self.non_admissions
                .lock()
                .unwrap()
                .push(proof.commitment().clone());
            Ok(())
        })
    }

    fn retain_verified_output<'a>(
        &'a self,
        output: buyer::VerifiedOutput<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<(), buyer::GateError>> + Send + 'a>> {
        Box::pin(async move {
            assert_eq!(
                output.receipt().draft.body.accepted_terms,
                output.authorization().terms.digest().unwrap()
            );
            assert_eq!(
                output.identity().billing_id.as_str(),
                output.authorization().terms.billing_id
            );
            assert!(output.response().is_object());
            if self.reject_output.load(Ordering::SeqCst) {
                return Err(buyer::GateError::Unavailable);
            }
            self.outputs.lock().unwrap().push(output.response().clone());
            Ok(())
        })
    }

    fn authorize<'a>(
        &'a self,
        purchase: &'a financial::quote::PreparedPurchase,
    ) -> Pin<Box<dyn Future<Output = Result<(), buyer::GateError>> + Send + 'a>> {
        Box::pin(async move {
            self.terms.lock().unwrap().push(purchase.terms().clone());
            self.entered.notify_one();
            if let Some(release) = &self.release {
                release.notified().await;
            }
            if self.reject {
                Err(buyer::GateError::Rejected)
            } else {
                Ok(())
            }
        })
    }
}
fn make_request(
    s: &Controlled,
    f: &Fixture,
    peer: &Peer,
    bytes: &[u8],
    gate: Arc<Gate>,
) -> buyer::Request {
    buyer::Request {
        context: context(peer),
        body: bytes.to_vec(),
        gate,
        authorization: buyer::Authorization {
            prices: prices(peer),
            output_units: (f.adapter.endpoint() == ProxyEndpoint::Chat).then_some(37),
            lifetimes: lifetimes(),
            settlement_policy: s.runtime.approved_policy.clone(),
            endpoint_contract: f.adapter.contract_hash().clone(),
            recipe_hash: f.adapter.recipe_hash().clone(),
        },
    }
}
fn buyer_controller(
    s: &Controlled,
    f: &Fixture,
    peer: &Peer,
    bridge: &Bridge,
    sessions: usize,
) -> (
    Arc<buyer::Controller>,
    Arc<BuyerNegotiation>,
    Arc<BuyerRecovery>,
) {
    let negotiation = Arc::new(
        BuyerNegotiation::new(
            Arc::new(
                financial::negotiation::Store::open(
                    f._store.path().join("buyer-controller"),
                    identity(peer),
                    financial::negotiation::Limits {
                        max_records: 8,
                        max_payload_bytes: 1024 * 1024,
                        max_record_bytes: 128 * 1024,
                        closed_retention_ms: 1000,
                    },
                )
                .unwrap(),
            ),
            peer.buyer_client.clone(),
            4,
        )
        .unwrap(),
    );
    let recovery = Arc::new(recovery(f, peer));
    let controller = buyer::Controller::new(
        negotiation.clone(),
        recovery.clone(),
        peer.buyer_client.clone(),
        s.buyer_signer.clone(),
        pool(f),
        bridge.config(true),
        buyer::Limits {
            sessions,
            buffer_bytes: 128 * 1024 * 1024,
            protocol: f.adapter.limits(),
            message_bytes: 2 * 1024 * 1024,
            control_wait: Duration::from_secs(5),
        },
    )
    .unwrap();
    (Arc::new(controller), negotiation, recovery)
}
async fn listener(bridge: &Bridge, peer: &Peer) -> n::opening::Listener {
    n::opening::Listener::connect(
        bridge.config(false),
        peer.identity.clone(),
        exchange::Limits {
            max_message_bytes: 2 * 1024 * 1024,
        },
    )
    .await
    .unwrap()
}
fn error(result: buyer::Result<buyer::Outcome>) -> buyer::Error {
    match result {
        Err(error) => error,
        Ok(_) => panic!("expected buyer failure"),
    }
}

#[tokio::test]
async fn buyer_controller_non_admission_hook_failure_reopens_original_fence_without_signing() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let bytes = chat();
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Fiat, &f, &bytes, false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let (provider, _) = observed_server(&s, &f, &peer);
    let ctx = context(&peer);
    let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
    let (buyer, negotiation, recovery) = buyer_controller(&s, &f, &peer, &bridge, 1);
    let mut listener = listener(&bridge, &peer).await;
    let gate = Arc::new(Gate {
        terms: Mutex::new(vec![]),
        reject: true,
        reject_output: std::sync::atomic::AtomicBool::new(false),
        outputs: Mutex::new(vec![]),
        non_admissions: Mutex::new(vec![]),
        reject_non_admission: std::sync::atomic::AtomicBool::new(true),
        entered: Notify::new(),
        release: None,
    });
    let request = make_request(&s, &f, &peer, &bytes, gate.clone());
    let identity = request.identity();
    let (stop, stopped) = watch::channel(false);
    let (result, handle) = tokio::join!(buyer.execute(request, stopped), async {
        provider
            .accept(listener.next(Duration::from_secs(5)).await.unwrap())
            .unwrap()
    });
    assert!(error(result).recovery_required);
    let _ = handle.wait().await;
    assert!(negotiation
        .lookup(identity.billing_id.clone(), identity.billing_attempt)
        .await
        .unwrap()
        .is_none());
    assert_eq!(gate.terms.lock().unwrap().len(), 1);
    assert!(gate.non_admissions.lock().unwrap().is_empty());
    buyer.shutdown().await.unwrap();
    drop((buyer, negotiation, recovery));
    let (buyer, _, _) = buyer_controller(&s, &f, &peer, &bridge, 1);
    let recovered_gate = Gate::allow();
    let mut commitment = None;
    for _ in 0..2 {
        let buyer::Outcome::NotAdmitted {
            identity: original,
            proof,
        } = buyer
            .recover(identity.clone(), recovered_gate.clone(), stop.subscribe())
            .await
            .unwrap()
        else {
            panic!("original unsigned fence must be recoverable")
        };
        assert_eq!(original, identity);
        assert_eq!(proof.terms(), &gate.terms.lock().unwrap()[0]);
        if let Some(previous) = &commitment {
            assert_eq!(proof.commitment(), previous);
        }
        commitment = Some(proof.commitment().clone());
    }
    let failure = error(
        buyer
            .execute(
                make_request(&s, &f, &peer, &bytes, Gate::allow()),
                stop.subscribe(),
            )
            .await,
    );
    assert_eq!(failure.code, buyer::Code::RecoveryRequired);
    let mut wrong = identity.clone();
    wrong.request_hash = d(9009);
    assert!(buyer
        .recover(wrong, recovered_gate.clone(), stop.subscribe())
        .await
        .is_err());
    assert_eq!(recovered_gate.non_admissions.lock().unwrap().len(), 2);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(peer.command("status").await["publications"], 0);
    buyer.shutdown().await.unwrap();
    peer.stop().await;
}

#[tokio::test]
async fn buyer_controller_json_chat_and_decisions_settle_exact_original_terms_on_all_rails() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for (endpoint, bytes, response) in cases()
            .into_iter()
            .filter(|(e, _, _)| matches!(e, ProxyEndpoint::Chat | ProxyEndpoint::Decisions))
        {
            let backend = backend(200, response, Duration::ZERO).await;
            let f = Fixture::new(&backend.base, endpoint);
            let mut peer = Peer::start(rail, &f, &bytes, false, None).await;
            let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
            let (provider, _) = observed_server(&s, &f, &peer);
            let ctx = context(&peer);
            let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
            let (buyer, negotiation, _) = buyer_controller(&s, &f, &peer, &bridge, 2);
            let mut listener = listener(&bridge, &peer).await;
            let gate = Gate::allow();
            let request = make_request(&s, &f, &peer, &bytes, gate.clone());
            let id = request.identity();
            let (stop, rx) = watch::channel(false);
            let (result, provider_result) = tokio::join!(buyer.execute(request, rx), async {
                provider
                    .accept(listener.next(Duration::from_secs(5)).await.unwrap())
                    .unwrap()
                    .wait()
                    .await
            });
            assert_eq!(provider_result.unwrap(), serving::End::Settled);
            let buyer::Outcome::Completed {
                identity,
                authorization,
                settlement,
                response,
            } = result.unwrap()
            else {
                panic!("verified completed result required")
            };
            assert_eq!(identity, id);
            assert_eq!(authorization.terms.offer, ctx.offer);
            assert_eq!(authorization.terms.rail, rail);
            assert_eq!(
                response["id"],
                format!("proxy_{}", ctx.invocation().unwrap().as_str())
            );
            let FinancialOutcome::Paid { receipt } = settlement else {
                panic!("canonical paid receipt required")
            };
            assert_eq!(
                receipt.body.accepted_terms,
                authorization.terms.digest().unwrap()
            );
            let gated = gate.terms.lock().unwrap();
            assert_eq!(gated.len(), 1);
            assert_eq!(gated[0], authorization.terms);
            assert_eq!(
                gated[0].max_spend_au,
                ctx.offer.cost(&gated[0].max_usage).unwrap()
            );
            drop(gated);
            assert_eq!(gate.outputs.lock().unwrap().as_slice(), &[response]);
            let again = error(
                buyer
                    .execute(
                        make_request(&s, &f, &peer, &bytes, Gate::allow()),
                        stop.subscribe(),
                    )
                    .await,
            );
            assert_eq!(again.code, buyer::Code::RecoveryRequired);
            assert!(again.recovery_required);
            assert!(matches!(
                buyer
                    .recover(id.clone(), gate.clone(), stop.subscribe())
                    .await
                    .unwrap(),
                buyer::Outcome::Closed {
                    settlement: FinancialOutcome::Paid { .. },
                    ..
                }
            ));
            assert!(negotiation
                .lookup(id.billing_id, id.billing_attempt)
                .await
                .unwrap()
                .unwrap()
                .confirmed());
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            assert_eq!(peer.command("status").await["publications"], 2);
            assert_eq!(
                peer.command("state").await["summary"]["reserved_au"],
                (50 + receipt.body.au_owed_cum).to_string(),
                "native hold is unchanged"
            );
            buyer.shutdown().await.unwrap();
            assert_eq!(buyer.active_sessions(), 0);
            peer.stop().await;
        }
    }
}

#[tokio::test]
async fn buyer_controller_signed_nonexecution_waiver_closes_without_retry_on_all_rails() {
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
        let (provider, _) = observed_server(&s, &f, &peer);
        let ctx = context(&peer);
        let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
        let (buyer, _, _) = buyer_controller(&s, &f, &peer, &bridge, 2);
        let mut listener = listener(&bridge, &peer).await;
        let (stop, rx) = watch::channel(false);
        let (result, provider_result) = tokio::join!(
            buyer.execute(make_request(&s, &f, &peer, &bytes, Gate::allow()), rx),
            async {
                provider
                    .accept(listener.next(Duration::from_secs(5)).await.unwrap())
                    .unwrap()
                    .wait()
                    .await
            }
        );
        assert_eq!(provider_result.unwrap(), serving::End::Settled);
        assert!(matches!(
            result.unwrap(),
            buyer::Outcome::Closed {
                settlement: FinancialOutcome::Waived { .. },
                ..
            }
        ));
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        assert_eq!(peer.command("status").await["publications"], 2);
        drop(stop);
        buyer.shutdown().await.unwrap();
        peer.stop().await;
    }
}

#[tokio::test]
async fn buyer_controller_rejects_unapproved_prices_hashes_streaming_and_gate_denial_before_signing(
) {
    for reject in ["price", "recipe", "contract", "stream", "gate"] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let bytes = chat();
        let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut peer = Peer::start(ProxyRail::Fiat, &f, &bytes, false, None).await;
        let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
        let (provider, _) = observed_server(&s, &f, &peer);
        let ctx = context(&peer);
        let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
        let (buyer, negotiation, _) = buyer_controller(&s, &f, &peer, &bridge, 1);
        let mut listener = listener(&bridge, &peer).await;
        let gate = Arc::new(Gate {
            reject: reject == "gate",
            reject_output: std::sync::atomic::AtomicBool::new(false),
            outputs: Mutex::new(vec![]),
            non_admissions: Mutex::new(vec![]),
            reject_non_admission: std::sync::atomic::AtomicBool::new(false),
            terms: Mutex::new(vec![]),
            entered: Notify::new(),
            release: None,
        });
        let mut request = make_request(&s, &f, &peer, &bytes, gate.clone());
        let id = request.identity();
        match reject {
            "price" => request.authorization.prices.rates[0].per_unit_au = 0,
            "recipe" => request.authorization.recipe_hash = d(909),
            "contract" => request.authorization.endpoint_contract = d(909),
            "stream" => {
                let mut value: Value = serde_json::from_slice(&bytes).unwrap();
                value["stream"] = json!(true);
                request.context.request_hash =
                    Digest::new(mayhem_proto::endpoint_request_fingerprint(&value)).unwrap();
                request.body = serde_json::to_vec(&value).unwrap();
            }
            _ => {}
        }
        let (_stop, rx) = watch::channel(false);
        let result = if matches!(reject, "price" | "stream") {
            buyer.execute(request, rx).await
        } else {
            let (result, handle) = tokio::join!(buyer.execute(request, rx), async {
                provider
                    .accept(listener.next(Duration::from_secs(5)).await.unwrap())
                    .unwrap()
            });
            let _ = handle.wait().await;
            result
        };
        if reject == "gate" {
            let buyer::Outcome::NotAdmitted { identity, proof } = result.unwrap() else {
                panic!("gate denial requires non-admission proof")
            };
            assert_eq!(identity, id);
            assert_eq!(proof.identity(), &id);
            assert_eq!(gate.non_admissions.lock().unwrap().len(), 1);
        } else {
            let failure = error(result);
            assert!(!failure.recovery_required, "{reject}");
        }
        assert!(negotiation
            .lookup(id.billing_id, id.billing_attempt)
            .await
            .unwrap()
            .is_none());
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        assert_eq!(peer.command("status").await["publications"], 0);
        assert_eq!(
            gate.terms.lock().unwrap().len(),
            usize::from(reject == "gate")
        );
        buyer.shutdown().await.unwrap();
        peer.stop().await;
    }
}

#[tokio::test]
async fn buyer_controller_abandoned_gate_keeps_session_permit_until_joined_and_never_signs() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let bytes = chat();
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Fiat, &f, &bytes, false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let (provider, _) = observed_server(&s, &f, &peer);
    let ctx = context(&peer);
    let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
    let (buyer, negotiation, _) = buyer_controller(&s, &f, &peer, &bridge, 1);
    let mut listener = listener(&bridge, &peer).await;
    let release = Arc::new(Notify::new());
    let gate = Arc::new(Gate {
        terms: Mutex::new(vec![]),
        reject: false,
        reject_output: std::sync::atomic::AtomicBool::new(false),
        outputs: Mutex::new(vec![]),
        non_admissions: Mutex::new(vec![]),
        reject_non_admission: std::sync::atomic::AtomicBool::new(false),
        entered: Notify::new(),
        release: Some(release.clone()),
    });
    let request = make_request(&s, &f, &peer, &bytes, gate.clone());
    let id = request.identity();
    let (stop, rx) = watch::channel(false);
    let cloned = buyer.clone();
    let task = tokio::spawn(async move { cloned.execute(request, rx).await });
    let handle = provider
        .accept(listener.next(Duration::from_secs(5)).await.unwrap())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(buyer.active_sessions(), 1);
    let busy = error(
        buyer
            .execute(
                make_request(&s, &f, &peer, &bytes, Gate::allow()),
                stop.subscribe(),
            )
            .await,
    );
    assert_eq!(busy.code, buyer::Code::Busy);
    {
        let abandoned_shutdown = buyer.shutdown();
        tokio::pin!(abandoned_shutdown);
        tokio::select! {
            _ = &mut abandoned_shutdown => panic!("shutdown must join the authorization commit"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }
    assert_eq!(
        buyer.active_sessions(),
        1,
        "dropping shutdown waiter keeps the owned commit alive"
    );
    let shutting = buyer.shutdown();
    tokio::pin!(shutting);
    tokio::select! { _ = &mut shutting => panic!("shutdown must join the authorization commit"), _ = tokio::time::sleep(Duration::from_millis(20)) => {} }
    release.notify_one();
    shutting.await.unwrap();
    let _ = handle.wait().await;
    assert_eq!(buyer.active_sessions(), 0);
    assert_eq!(
        gate.non_admissions.lock().unwrap().len(),
        1,
        "joined abandoned gate receives durable non-admission proof"
    );
    assert!(negotiation
        .lookup(id.billing_id, id.billing_attempt)
        .await
        .unwrap()
        .is_none());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(peer.command("status").await["publications"], 0);
    peer.stop().await;
}

#[tokio::test]
async fn buyer_controller_cancelled_execution_retains_original_purchase_and_refuses_duplicate_execute(
) {
    let backend = backend(200, answer(), Duration::from_millis(300)).await;
    let bytes = chat();
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &bytes, false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let (provider, _) = observed_server(&s, &f, &peer);
    let ctx = context(&peer);
    let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
    let (buyer, negotiation, _) = buyer_controller(&s, &f, &peer, &bridge, 1);
    let mut listener = listener(&bridge, &peer).await;
    let request = make_request(&s, &f, &peer, &bytes, Gate::allow());
    let id = request.identity();
    let (stop, rx) = watch::channel(false);
    let cloned = buyer.clone();
    let task = tokio::spawn(async move { cloned.execute(request, rx).await });
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
    let saved = negotiation
        .lookup(id.billing_id.clone(), id.billing_attempt)
        .await
        .unwrap()
        .unwrap();
    assert!(saved.authorization().is_some());
    assert!(saved.confirmed());
    assert_eq!(saved.offer().terms.session_id, id.session_id.as_str());
    let (_retry, retry) = watch::channel(false);
    let duplicate = error(
        buyer
            .execute(make_request(&s, &f, &peer, &bytes, Gate::allow()), retry)
            .await,
    );
    assert_eq!(duplicate.code, buyer::Code::RecoveryRequired);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    buyer.shutdown().await.unwrap();
    let _ = handle.wait().await;
    assert_eq!(
        peer.command("status").await["publications"],
        1,
        "cancellation did not fabricate settlement"
    );
    peer.stop().await;
}

#[tokio::test]
async fn buyer_controller_retains_verified_output_before_payment_and_recovers_without_execute() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let bytes = chat();
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tap, &f, &bytes, false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let (provider, _) = observed_server(&s, &f, &peer);
    let ctx = context(&peer);
    let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
    let (buyer, negotiation, recovery) = buyer_controller(&s, &f, &peer, &bridge, 1);
    let mut listener = listener(&bridge, &peer).await;
    let gate = Gate::allow();
    gate.reject_output.store(true, Ordering::SeqCst);
    let request = make_request(&s, &f, &peer, &bytes, gate.clone());
    let id = request.identity();
    let (stop, rx) = watch::channel(false);
    let (result, handle) = tokio::join!(buyer.execute(request, rx), async {
        provider
            .accept(listener.next(Duration::from_secs(5)).await.unwrap())
            .unwrap()
    });
    let failure = error(result);
    assert_eq!(failure.code, buyer::Code::Storage);
    assert_eq!(failure.stage, buyer::Stage::Verifying);
    assert!(failure.recovery_required);
    let _ = handle.wait().await;
    assert_eq!(
        peer.command("status").await["publications"],
        1,
        "no payment before durable output"
    );
    assert!(gate.outputs.lock().unwrap().is_empty());
    assert!(negotiation
        .lookup(id.billing_id.clone(), id.billing_attempt)
        .await
        .unwrap()
        .unwrap()
        .confirmed());
    let saved = negotiation
        .lookup(id.billing_id.clone(), id.billing_attempt)
        .await
        .unwrap()
        .unwrap();
    let key = Digest::new(saved.authorization().unwrap().terms.digest().unwrap()).unwrap();
    let before = recovery.recover(key.clone()).await.unwrap();
    assert!(!before.outcome_approved && !before.outcome_signed);
    let (wallet, _, _) = signer(&mut peer).await;
    let mut runner = financial::recovery::runner::Runner::new(
        recovery.clone(),
        Some(Arc::try_unwrap(wallet).ok().unwrap()),
        financial::recovery::runner::Policy::default(),
        1,
    )
    .unwrap();
    let page = runner.page().await.unwrap();
    assert_eq!(page.checked, 1);
    assert!(page.error_code.is_none());
    let after = recovery.recover(key.clone()).await.unwrap();
    assert!(
        !after.outcome_approved && !after.outcome_signed,
        "recovery cannot sign before output retention"
    );
    assert!(recovery.sign_approved(&s.buyer_signer, key).await.is_err());
    gate.reject_output.store(false, Ordering::SeqCst);
    let (result, provider_result) = tokio::join!(
        buyer.recover(id.clone(), gate.clone(), stop.subscribe()),
        async {
            provider
                .accept(listener.next(Duration::from_secs(5)).await.unwrap())
                .unwrap()
                .wait()
                .await
        }
    );
    assert_eq!(provider_result.unwrap(), serving::End::Settled);
    let buyer::Outcome::Completed {
        identity,
        response,
        settlement: FinancialOutcome::Paid { .. },
        ..
    } = result.unwrap()
    else {
        panic!("recovered verified output required")
    };
    assert_eq!(identity, id);
    assert_eq!(gate.outputs.lock().unwrap().as_slice(), &[response]);
    assert_eq!(
        gate.terms.lock().unwrap().len(),
        1,
        "recovery uses original authorization"
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert_eq!(peer.command("status").await["publications"], 2);
    buyer.shutdown().await.unwrap();
    peer.stop().await;
}
