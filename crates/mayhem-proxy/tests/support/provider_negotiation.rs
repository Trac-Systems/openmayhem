use super::super::super::authenticated_exchange::exchange_bridge::Bridge;
use super::*;
use financial::negotiation::{BuyerOffer, SavedPurchase};
use financial::provider::{ProviderNegotiation, Runtime};
use mayhem_proxy::exchange::{Channel, Message, Role, Session};

async fn deliver(
    buyer: &mut Channel,
    provider: &mut Channel,
    request: &[u8],
) -> mayhem_proxy::exchange::Received {
    let message = Message::Execute {
        request: serde_json::from_slice(request).unwrap(),
        streaming: false,
    };
    let (sent, received) = tokio::join!(
        buyer.send(&message),
        provider.receive(Some(Duration::from_secs(5)))
    );
    sent.unwrap();
    received.unwrap()
}

async fn channels(peer: &Peer, auth: &ProxySpendAuthorization) -> (Bridge, Channel, Channel) {
    let bridge = Bridge::start(&auth.terms.buyer_pubkey, &auth.terms.offer.provider_pubkey).await;
    let provider = Channel::connect(
        bridge.config(false),
        Session::new(auth.clone(), &peer.identity, Role::Provider).unwrap(),
        mayhem_proxy::exchange::Limits {
            max_message_bytes: 1024 * 1024,
        },
    )
    .await
    .unwrap();
    let buyer = Channel::connect(
        bridge.config(true),
        Session::new(auth.clone(), &identity(peer), Role::Buyer).unwrap(),
        mayhem_proxy::exchange::Limits {
            max_message_bytes: 1024 * 1024,
        },
    )
    .await
    .unwrap();
    (bridge, buyer, provider)
}

struct Setup {
    runtime: Arc<Runtime>,
    journal: Arc<Journal>,
    provider: ProviderNegotiation,
    provider_signer: Arc<Authority>,
    lease: capacity::Reservation,
    buyer: BuyerNegotiation,
    saved: SavedPurchase,
    buyer_key: [u8; 32],
}
fn journal_limits(bytes: u64) -> attempts::Limits {
    attempts::Limits {
        max_records: 8,
        max_unfinished: 8,
        closed_retention_ms: 1000,
        max_payload_bytes: bytes,
    }
}
impl Setup {
    async fn new(f: &Fixture, peer: &mut Peer, bytes: &[u8]) -> Self {
        let (buyer_signer, buyer_key, provider_key) = signer(peer).await;
        let provider_signer = Arc::new(
            Authority::from_unlocked_wallet(
                SigningKey::from_bytes(&provider_key),
                peer.identity.clone(),
            )
            .unwrap(),
        );
        let capacity = Arc::new(
            capacity::Authority::open(
                f._store.path().join("provider-capacity"),
                peer.identity.clone(),
                capacity::Limits {
                    max_groups: 4,
                    max_routes: 8,
                    max_leases: 8,
                    max_evidence_age: Duration::from_secs(60),
                },
            )
            .unwrap(),
        );
        capacity.configure_group(d(200), 2).unwrap();
        capacity
            .configure_route(capacity::Route {
                id: d(201),
                group: d(200),
                lane: capacity::Lane::Proxy,
                max_concurrency: 2,
            })
            .unwrap();
        capacity_ready(&capacity, capacity::Scope::Group(d(200)));
        capacity_ready(&capacity, capacity::Scope::Route(d(201)));
        let invocation =
            mayhem_proxy::exchange::invocation_for_terms(&peer.template.terms).unwrap();
        let lease = capacity
            .reserve(
                &d(201),
                capacity::Work {
                    invocation,
                    request_hash: Digest::new(&peer.template.terms.request_hash).unwrap(),
                },
            )
            .unwrap();
        let mut binding = session(peer);
        binding.capacity_lease = lease.lease().id.clone();
        let (q, purchase) = prepare(peer, f, bytes, &binding).await;
        let policy = purchase.policy().clone();
        let buyer = controller(f, peer, 8);
        let saved = buyer.sign(purchase, q, buyer_signer, 1000).await.unwrap();
        let runtime = Arc::new(Runtime {
            adapter: f.adapter.clone(),
            connection: f.connection.clone(),
            capacity,
            route: d(201),
            approved_policy: policy,
        });
        let journal = Arc::new(
            Journal::open(
                f._store.path().join("provider-journal"),
                peer.identity.clone(),
                journal_limits(128 * 1024 * 1024),
            )
            .unwrap(),
        );
        let provider =
            ProviderNegotiation::new(journal.clone(), provider_signer.clone(), 4).unwrap();
        Self {
            runtime,
            journal,
            provider,
            provider_signer,
            lease,
            buyer,
            saved,
            buyer_key,
        }
    }
    async fn approve(
        &self,
        peer: &Peer,
        offer: BuyerOffer,
        request: Vec<u8>,
    ) -> mayhem_proxy::Result<financial::provider::Approval> {
        let state = peer
            .client
            .offer_state(&financial::offer::Query {
                offer: self.saved.offer().terms.offer,
                rail: self.saved.offer().terms.rail,
                settlement_policy_hash: self.saved.offer().terms.settlement_policy_hash,
            })
            .await?;
        self.runtime.approve(offer, state, &self.lease, request)
    }
    fn executor(&self, f: &Fixture, peer: &Peer) -> PaidExecutor {
        let pool = Arc::new(
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
        );
        let executor = Executor::new(
            f.connection.clone(),
            f.adapter.clone(),
            pool,
            Arc::new(Storage::new(self.journal.clone(), 4).unwrap()),
        )
        .unwrap();
        PaidExecutor::new(
            executor,
            peer.client.clone(),
            self.runtime.capacity.clone(),
            d(201),
        )
        .unwrap()
    }
}

#[tokio::test]
async fn negotiated_session_rejects_unsigned_prepared_record_and_different_signed_acceptance() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Fiat, &f, &chat(), false, None).await;
    let mut s = Setup::new(&f, &mut peer, &chat()).await;
    let approved = s.approve(&peer, s.saved.offer(), chat()).await.unwrap();
    let invocation = approved.invocation().clone();
    let signed = s.provider.accept(approved, 1001).await.unwrap();
    let record = s.journal.get(&invocation).unwrap().unwrap();
    let unsigned = Arc::new(
        Journal::open(
            f._store.path().join("unsigned-draft"),
            peer.identity.clone(),
            journal_limits(128 * 1024 * 1024),
        )
        .unwrap(),
    );
    let draft = unsigned
        .prepare(invocation.clone(), record.binding.clone(), 1001)
        .unwrap();
    unsigned
        .retain_acceptance(
            &invocation,
            draft.attempt,
            &attempts::AcceptanceSnapshot {
                adapter: f.adapter.snapshot(),
                offer: signed.authorization.terms.offer.clone(),
            },
        )
        .unwrap();
    unsigned
        .retain_request(
            &invocation,
            draft.attempt,
            &chat(),
            f.adapter.limits().response_bytes,
        )
        .unwrap();
    let original = std::mem::replace(&mut s.journal, unsigned);
    let (_bridge, mut buyer, mut provider) = channels(&peer, &signed.authorization).await;
    let received = deliver(&mut buyer, &mut provider, &chat()).await;
    assert!(matches!(
        provider
            .session()
            .execute_json(received, &s.executor(&f, &peer), &Cancellation::default())
            .await,
        Err(mayhem_proxy::exchange::Error::Identity)
    ));
    s.journal = original;

    let mut changed = signed.authorization;
    changed.terms.capacity_lease = d(919).as_str().into();
    let keys = peer.command("ephemeral_test_wallet_seeds").await;
    let provider_key: [u8; 32] = serde_json::from_value(keys["provider"].clone()).unwrap();
    changed.buyer_sig = signature(&s.buyer_key, &changed.terms.buyer_signing_bytes().unwrap());
    changed.provider_sig = signature(
        &provider_key,
        &changed.terms.provider_signing_bytes().unwrap(),
    );
    changed
        .verify(mayhem_proxy::receipts::verify_signature)
        .unwrap();
    let (_bridge, mut buyer, mut provider) = channels(&peer, &changed).await;
    let received = deliver(&mut buyer, &mut provider, &chat()).await;
    assert!(matches!(
        provider
            .session()
            .execute_json(received, &s.executor(&f, &peer), &Cancellation::default())
            .await,
        Err(mayhem_proxy::exchange::Error::Identity)
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(peer.command("status").await["publications"], 0);
    peer.stop().await;
}

#[tokio::test]
async fn provider_countersignature_survives_reopen_and_hands_off_to_one_paid_post_for_every_endpoint_rail(
) {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for (endpoint, bytes, response) in cases() {
            let backend = backend(200, response, Duration::ZERO).await;
            let f = Fixture::new(&backend.base, endpoint);
            let mut peer = Peer::start(rail, &f, &bytes, false, None).await;
            let mut s = Setup::new(&f, &mut peer, &bytes).await;
            let approval = s
                .approve(&peer, s.saved.offer(), bytes.clone())
                .await
                .unwrap();
            let invocation = approval.invocation().clone();
            let signed = s.provider.accept(approval, 1001).await.unwrap();
            signed
                .authorization
                .verify(mayhem_proxy::receipts::verify_signature)
                .unwrap();
            let record = s.journal.get(&invocation).unwrap().unwrap();
            assert_eq!(record.phase, Phase::Prepared);
            let retained = s.journal.recover(&invocation, record.attempt).unwrap();
            assert_eq!(retained.request.unwrap().body, bytes);
            assert!(retained.acceptance.is_some());
            assert!(
                retained.financial.is_none(),
                "signature does not invent a canonical hold"
            );
            assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
            assert_eq!(peer.command("status").await["publications"], 0);
            let allocated = s.journal.allocated_payload_bytes().unwrap();
            drop(s.provider);
            drop(s.journal);
            s.journal = Arc::new(
                Journal::open(
                    f._store.path().join("provider-journal"),
                    peer.identity.clone(),
                    journal_limits(128 * 1024 * 1024),
                )
                .unwrap(),
            );
            s.provider =
                ProviderNegotiation::new(s.journal.clone(), s.provider_signer.clone(), 4).unwrap();
            let recovered = s
                .provider
                .recover(invocation.clone())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(recovered.authorization, signed.authorization);
            assert_eq!(s.journal.allocated_payload_bytes().unwrap(), allocated);
            let again = s
                .approve(&peer, s.saved.offer(), bytes.clone())
                .await
                .unwrap();
            assert_eq!(
                s.provider.accept(again, 1002).await.unwrap().authorization,
                signed.authorization
            );
            assert_eq!(
                s.journal.allocated_payload_bytes().unwrap(),
                allocated,
                "duplicate signing does not allocate again"
            );
            s.buyer
                .retain_provider_acceptance(signed.authorization.clone())
                .await
                .unwrap();
            let (_bridge, mut buyer_channel, mut provider_channel) =
                channels(&peer, &signed.authorization).await;
            let executor = s.executor(&f, &peer);
            let unsent = deliver(&mut buyer_channel, &mut provider_channel, &bytes).await;
            assert!(
                provider_channel
                    .session()
                    .execute_json(unsent, &executor, &Cancellation::default())
                    .await
                    .is_err(),
                "dual signatures do not replace canonical funding"
            );
            assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
            let finance = recovery(&f, &peer);
            s.buyer
                .publish(s.saved.key().clone(), &finance, 1003)
                .await
                .unwrap();
            assert_eq!(peer.command("status").await["publications"], 1);
            let request = deliver(&mut buyer_channel, &mut provider_channel, &bytes).await;
            let delivered = provider_channel
                .session()
                .execute_json(request, &executor, &Cancellation::default())
                .await
                .unwrap();
            assert_eq!(delivered.attempt.attempt, record.attempt);
            assert!(
                matches!(
                    executor
                        .execute_json(&invocation, &bytes, &Cancellation::default())
                        .await,
                    Err(mayhem_proxy::execution::Error::RecoveryRequired)
                ),
                "retry must recover the owned result, not dispatch again"
            );
            assert!(executor
                .recover(&invocation, record.attempt)
                .await
                .unwrap()
                .result
                .is_some());
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            assert!(executor
                .reconcile_capacity(&invocation, record.attempt)
                .await
                .unwrap());
            assert_eq!(s.runtime.capacity.status(&d(201)).unwrap().available, 2);
            let provider_receipt = executor
                .sign_terminal_receipt(&s.provider_signer, &invocation, record.attempt)
                .await
                .unwrap();
            // Independently verify delivered output using only buyer-owned public
            // evidence before the buyer signs the provider's receipt.
            use mayhem_proxy::buyer::Evidence;
            let prepared = s
                .saved
                .snapshot()
                .verify_request(&record.binding, &bytes)
                .unwrap();
            let created = delivered.reply.body[if endpoint == ProxyEndpoint::Responses {
                "created_at"
            } else {
                "created"
            }]
            .as_u64()
            .unwrap();
            let received = prepared
                .decode_json(
                    delivered.reply.body.clone(),
                    delivered.reply.body["id"].as_str().unwrap(),
                    created,
                )
                .unwrap();
            let verifier = Pool::new(
                env!("CARGO_BIN_EXE_mayhem-proxy-worker"),
                f._work.path(),
                PoolLimits {
                    max_children: 2,
                    max_buffer_bytes: 64 * 1024 * 1024,
                    startup_timeout: Duration::from_secs(5),
                    processing_timeout: Duration::from_secs(3),
                },
            )
            .unwrap();
            let approval = mayhem_proxy::receipts::approve_terminal(
                &verifier,
                &provider_receipt.draft,
                &provider_receipt.provider_sig,
                &signed.authorization,
                s.saved.policy(),
                s.saved.snapshot(),
                &bytes,
                &received,
                false,
            )
            .await
            .unwrap();
            let body = provider_receipt.draft.body;
            let receipt = mayhem_proto::proxy::finance::ProxyUsageReceipt {
                buyer_sig: signature(&s.buyer_key, approval.signing_bytes()),
                provider_sig: provider_receipt.provider_sig,
                body,
            };
            executor
                .retain_terminal_receipt(&invocation, record.attempt, &receipt)
                .await
                .unwrap();
            assert!(executor
                .publish_terminal_receipt(&invocation, record.attempt)
                .await
                .unwrap());
            assert_eq!(peer.command("status").await["publications"], 2);
            assert_eq!(
                peer.command("state").await["summary"]["reserved_au"],
                (50 + receipt.body.au_owed_cum).to_string()
            );
            let closed = s.journal.get(&invocation).unwrap().unwrap();
            assert_eq!(closed.phase, Phase::Closed);
            assert_eq!(
                s.journal
                    .prune_closed(closed.expires_at_ms.unwrap(), 64)
                    .unwrap(),
                1
            );
            assert_eq!(
                s.journal.allocated_payload_bytes().unwrap(),
                0,
                "all owned payloads and signatures are included in pruning"
            );
            assert!(s.provider.recover(invocation).await.unwrap().is_none());
            peer.stop().await;
        }
    }
}

#[tokio::test]
async fn provider_approval_checks_streaming_requests_on_all_llm_endpoints_and_rails() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for (endpoint, mut body) in [
            (
                ProxyEndpoint::Chat,
                serde_json::from_slice::<Value>(&chat()).unwrap(),
            ),
            (
                ProxyEndpoint::Completions,
                json!({"model":"public-model","prompt":"hi"}),
            ),
            (
                ProxyEndpoint::Responses,
                json!({"model":"public-model","input":"hi"}),
            ),
        ] {
            body["stream"] = json!(true);
            let bytes = serde_json::to_vec(&body).unwrap();
            let backend = backend(200, answer(), Duration::ZERO).await;
            let f = Fixture::new(&backend.base, endpoint);
            let mut peer = Peer::start(rail, &f, &bytes, true, None).await;
            let s = Setup::new(&f, &mut peer, &bytes).await;
            let a = s.approve(&peer, s.saved.offer(), bytes).await.unwrap();
            let signed = s.provider.accept(a, 1001).await.unwrap();
            signed
                .authorization
                .verify(mayhem_proxy::receipts::verify_signature)
                .unwrap();
            assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
            peer.stop().await;
        }
    }
}

#[tokio::test]
async fn provider_rejects_invalid_signature_request_metering_connection_and_capacity_without_signing(
) {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let bytes = chat();
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &bytes, false, None).await;
    let s = Setup::new(&f, &mut peer, &bytes).await;
    let invocation = mayhem_proxy::exchange::invocation_for_terms(&s.saved.offer().terms).unwrap();
    for fault in ["signature", "request", "metering", "connection", "lease"] {
        let mut offer = s.saved.offer();
        let mut body = bytes.clone();
        match fault {
            "signature" => {}
            "request" => body =
                br#"{"model":"public-model","messages":[{"role":"user","content":"different"}]}"#
                    .to_vec(),
            "metering" => {
                *offer.terms.max_usage.get_mut("input_token").unwrap() += 1;
                offer.terms.max_spend_au = offer.terms.offer.cost(&offer.terms.max_usage).unwrap();
            }
            "connection" => offer.terms.connection_digest = "0".repeat(64),
            "lease" => offer.terms.capacity_lease = "0".repeat(64),
            _ => unreachable!(),
        }
        offer.buyer_sig = if fault == "signature" {
            "0".repeat(128)
        } else {
            signature(&s.buyer_key, &offer.terms.buyer_signing_bytes().unwrap())
        };
        assert!(s.approve(&peer, offer, body).await.is_err(), "{fault}");
    }
    let a = s
        .approve(&peer, s.saved.offer(), bytes.clone())
        .await
        .unwrap();
    let ticket = s
        .runtime
        .capacity
        .begin_observation(capacity::Scope::Group(d(200)))
        .unwrap();
    s.runtime
        .capacity
        .observe(
            ticket,
            capacity::Evidence {
                state: capacity::Readiness::Busy,
                allowance: 0,
                age: Duration::ZERO,
                valid_for: Duration::from_secs(60),
            },
        )
        .unwrap();
    assert!(
        s.provider.accept(a, 1001).await.is_err(),
        "recheck after asynchronous preparation"
    );
    assert!(s
        .provider
        .recover(invocation.clone())
        .await
        .unwrap()
        .is_none());
    capacity_ready(&s.runtime.capacity, capacity::Scope::Group(d(200)));
    let a = s
        .approve(&peer, s.saved.offer(), bytes.clone())
        .await
        .unwrap();
    s.runtime.capacity.dispatch(&s.lease).unwrap();
    assert!(
        s.provider.accept(a, 1002).await.is_err(),
        "already dispatched is not a new signing reservation"
    );
    assert!(s.provider.recover(invocation).await.unwrap().is_none());
    assert_eq!(peer.command("status").await["publications"], 0);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    peer.stop().await;
}

#[tokio::test]
async fn provider_signing_rejects_wrong_wallet_and_full_storage_without_exposing_a_signature() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let bytes = chat();
    let mut peer = Peer::start(ProxyRail::Fiat, &f, &bytes, false, None).await;
    let s = Setup::new(&f, &mut peer, &bytes).await;
    let (buyer_signer, _, _) = signer(&mut peer).await;
    assert!(ProviderNegotiation::new(s.journal.clone(), buyer_signer, 4).is_err());
    let small = Arc::new(
        Journal::open(
            f._store.path().join("small-provider-journal"),
            peer.identity.clone(),
            journal_limits(1),
        )
        .unwrap(),
    );
    let provider = ProviderNegotiation::new(small.clone(), s.provider_signer.clone(), 4).unwrap();
    let a = s.approve(&peer, s.saved.offer(), bytes).await.unwrap();
    let invocation = a.invocation().clone();
    assert!(provider.accept(a, 1001).await.is_err());
    assert!(provider.recover(invocation).await.unwrap().is_none());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(peer.command("status").await["publications"], 0);
    peer.stop().await;
}

#[tokio::test]
async fn competing_provider_acceptances_can_recover_only_one_exact_signed_purchase() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let bytes = chat();
    let mut peer = Peer::start(ProxyRail::Tap, &f, &bytes, false, None).await;
    let s = Setup::new(&f, &mut peer, &bytes).await;
    let original = s.saved.offer();
    let mut other = original.clone();
    *other.terms.max_usage.get_mut("output_token").unwrap() += 1;
    other.terms.max_spend_au = other.terms.offer.cost(&other.terms.max_usage).unwrap();
    other.buyer_sig = signature(&s.buyer_key, &other.terms.buyer_signing_bytes().unwrap());
    let a = s
        .approve(&peer, original.clone(), bytes.clone())
        .await
        .unwrap();
    let invocation = a.invocation().clone();
    let b = s
        .approve(&peer, original.clone(), bytes.clone())
        .await
        .unwrap();
    let c = s.approve(&peer, other.clone(), bytes).await.unwrap();
    let (a, b, c) = tokio::join!(
        s.provider.accept(a, 1001),
        s.provider.accept(b, 1001),
        s.provider.accept(c, 1001)
    );
    let saved = s
        .provider
        .recover(invocation.clone())
        .await
        .unwrap()
        .unwrap();
    let successes: Vec<_> = [a, b, c].into_iter().filter_map(Result::ok).collect();
    assert!(!successes.is_empty());
    for value in successes {
        assert_eq!(value.authorization, saved.authorization);
    }
    assert!(
        saved.authorization.terms == original.terms || saved.authorization.terms == other.terms
    );
    assert_eq!(
        s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
        1
    );
    assert_eq!(s.journal.get(&invocation).unwrap().unwrap().attempt, 1);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(peer.command("status").await["publications"], 0);
    peer.stop().await;
}

#[tokio::test]
#[ignore = "subprocess used by provider_signature_commit_survives_abrupt_process_exit"]
async fn provider_acceptance_abrupt_exit_child() {
    let output = match std::env::var_os("MAYHEM_TEST_PROVIDER_ACCEPTANCE_CRASH") {
        Some(v) => std::path::PathBuf::from(v),
        None => return,
    };
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let bytes = chat();
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &bytes, false, None).await;
    let s = Setup::new(&f, &mut peer, &bytes).await;
    let a = s.approve(&peer, s.saved.offer(), bytes).await.unwrap();
    let invocation = a.invocation().clone();
    let signed = s.provider.accept(a, 1001).await.unwrap();
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(peer.command("status").await["publications"], 0);
    // Only public ephemeral test identity/signatures and generated temp paths.
    std::fs::write(
        output,
        serde_json::to_vec(&json!({"store":f._store.path(),"work":f._work.path(),
        "identity":peer.identity,"invocation":invocation,"signed":signed}))
        .unwrap(),
    )
    .unwrap();
    std::process::exit(74); // Deliberately no Rust journal/capacity destructors.
}

#[tokio::test]
async fn provider_signature_commit_survives_abrupt_process_exit() {
    let temp = dir();
    let output = temp.path().join("public-fixture.json");
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "paid_acceptance::purchase::negotiation::provider::provider_acceptance_abrupt_exit_child", "--nocapture"])
        .env("MAYHEM_TEST_PROVIDER_ACCEPTANCE_CRASH", &output)
        .kill_on_drop(true).spawn().unwrap();
    let status = tokio::time::timeout(Duration::from_secs(30), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.code(), Some(74));
    let value: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let store = std::path::PathBuf::from(value["store"].as_str().unwrap());
    let work = std::path::PathBuf::from(value["work"].as_str().unwrap());
    let identity: Identity = serde_json::from_value(value["identity"].clone()).unwrap();
    let invocation: Digest = serde_json::from_value(value["invocation"].clone()).unwrap();
    let expected: attempts::SignedProviderAcceptance =
        serde_json::from_value(value["signed"].clone()).unwrap();
    let journal = Journal::open(
        store.join("provider-journal"),
        identity,
        journal_limits(128 * 1024 * 1024),
    )
    .unwrap();
    let record = journal.get(&invocation).unwrap().unwrap();
    assert_eq!(record.phase, Phase::Prepared);
    let signed = journal
        .provider_acceptance(&invocation, record.attempt)
        .unwrap()
        .unwrap();
    assert_eq!(signed.authorization, expected.authorization);
    let saved = journal.recover(&invocation, record.attempt).unwrap();
    assert_eq!(saved.request.unwrap().body, chat());
    assert!(saved.acceptance.is_some());
    assert!(saved.financial.is_none());
    drop(journal);
    std::fs::remove_dir_all(store).unwrap();
    std::fs::remove_dir_all(work).unwrap();
}
