use super::*;
use n::provider::{Controller as Proposals, Limits as ProposalLimits};

fn limits() -> ProposalLimits {
    ProposalLimits {
        pending: 4,
        per_buyer: 2,
        request_bytes: 512 * 1024,
        total_request_bytes: 1024 * 1024,
        storage_operations: 4,
        unsigned_lifetime: Duration::from_secs(30),
    }
}
struct Controlled {
    proposals: Proposals,
    runtime: Arc<Runtime>,
    journal: Arc<Journal>,
    provider: Arc<ProviderNegotiation>,
    buyer: BuyerNegotiation,
    buyer_signer: Arc<Authority>,
}
impl Controlled {
    fn restart(self, f: &Fixture, peer: &Peer) -> Self {
        let policy = self.runtime.approved_policy.clone();
        let Self {
            proposals,
            runtime,
            journal,
            provider,
            buyer,
            buyer_signer,
        } = self;
        drop(proposals);
        drop(runtime);
        let capacity = Arc::new(
            capacity::Authority::open(
                f._store.path().join("controlled-capacity"),
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
        let runtime = Arc::new(Runtime {
            adapter: f.adapter.clone(),
            connection: f.connection.clone(),
            capacity,
            route: d(201),
            approved_policy: policy,
        });
        let proposals = Proposals::new(
            runtime.clone(),
            provider.clone(),
            peer.client.clone(),
            limits(),
        )
        .unwrap();
        Self {
            proposals,
            runtime,
            journal,
            provider,
            buyer,
            buyer_signer,
        }
    }
    async fn new(f: &Fixture, peer: &mut Peer, limits: ProposalLimits, journal_bytes: u64) -> Self {
        let (buyer_signer, _, provider_key) = signer(peer).await;
        let provider_signer = Arc::new(
            Authority::from_unlocked_wallet(
                SigningKey::from_bytes(&provider_key),
                peer.identity.clone(),
            )
            .unwrap(),
        );
        let capacity = Arc::new(
            capacity::Authority::open(
                f._store.path().join("controlled-capacity"),
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
        // Ephemeral test fixture policy; production supplies operator-approved policy.
        let q = query(peer);
        let policy = peer
            .client
            .offer_state(&financial::offer::Query {
                offer: q.offer,
                rail: q.rail,
                settlement_policy_hash: q.settlement_policy_hash,
            })
            .await
            .unwrap()
            .policy()
            .unwrap()
            .clone();
        let runtime = Arc::new(Runtime {
            adapter: f.adapter.clone(),
            connection: f.connection.clone(),
            capacity,
            route: d(201),
            approved_policy: policy,
        });
        let journal = Arc::new(
            Journal::open(
                f._store.path().join("controlled-journal"),
                peer.identity.clone(),
                journal_limits(journal_bytes),
            )
            .unwrap(),
        );
        let provider =
            Arc::new(ProviderNegotiation::new(journal.clone(), provider_signer, 4).unwrap());
        let proposals = Proposals::new(
            runtime.clone(),
            provider.clone(),
            peer.client.clone(),
            limits,
        )
        .unwrap();
        Self {
            proposals,
            runtime,
            journal,
            provider,
            buyer: controller(f, peer, 8),
            buyer_signer,
        }
    }
    async fn start(
        &self,
        peer: &Peer,
        context: n::Context,
        bytes: &[u8],
    ) -> mayhem_proxy::Result<(Pair, n::Proposal)> {
        let mut pair = open(peer, context.clone(), 1024 * 1024).await;
        let received = transfer(
            &mut pair.buyer,
            &mut pair.provider,
            n::Message::Request {
                request: serde_json::from_slice(bytes).unwrap(),
            },
        )
        .await;
        let proposal = self.proposals.propose(context, received).await?;
        Ok((pair, proposal))
    }
    async fn offer(
        &self,
        peer: &Peer,
        bytes: &[u8],
        pair: &mut Pair,
        proposal: n::Proposal,
    ) -> (SavedPurchase, n::Received) {
        let message = transfer(
            &mut pair.provider,
            &mut pair.buyer,
            n::Message::Proposal { proposal },
        )
        .await
        .into_message(&pair.context, Role::Buyer)
        .unwrap();
        let n::Message::Proposal { proposal } = message else {
            panic!("proposal required")
        };
        let output = (self.runtime.adapter.endpoint() != ProxyEndpoint::Decisions).then_some(37);
        let intent = PurchaseRequest::new(
            proposal.adapter.clone(),
            bytes.to_vec(),
            prices(peer),
            output,
            lifetimes(),
        )
        .unwrap();
        let quote = peer.buyer_client.quote(&query(peer)).await.unwrap();
        let purchase = quote
            .prepare_purchase(&intent, &proposal.session_binding(&pair.context).unwrap())
            .unwrap();
        let saved = self
            .buyer
            .sign(purchase, quote, self.buyer_signer.clone(), 1000)
            .await
            .unwrap();
        let received = transfer(
            &mut pair.buyer,
            &mut pair.provider,
            n::Message::Offer {
                offer: saved.offer(),
            },
        )
        .await;
        (saved, received)
    }
}

#[tokio::test]
async fn provider_controller_negotiates_funds_and_dispatches_once_on_every_endpoint_and_rail() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for (endpoint, bytes, response) in cases() {
            let backend = backend(200, response, Duration::ZERO).await;
            let f = Fixture::new(&backend.base, endpoint);
            let mut peer = Peer::start(rail, &f, &bytes, false, None).await;
            let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
            let ctx = context(&peer);
            let (mut pair, proposal) = s.start(&peer, ctx.clone(), &bytes).await.unwrap();
            let lease = proposal.capacity_lease.clone();
            assert_eq!(
                s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
                1
            );
            let (saved, offer) = s.offer(&peer, &bytes, &mut pair, proposal).await;
            let signed = s.proposals.accept(ctx.clone(), offer, 1001).await.unwrap();
            assert!(
                s.runtime
                    .capacity
                    .signing_intent(&lease)
                    .unwrap()
                    .unwrap()
                    .buyer()
                    == &saved.offer()
            );
            assert_eq!(s.proposals.status().await.unwrap().request_bytes, 0);
            assert!(!s.proposals.cancel_unsigned(ctx.clone()).await.unwrap());
            assert_eq!(
                s.proposals
                    .recover(&ctx)
                    .await
                    .unwrap()
                    .unwrap()
                    .authorization,
                signed.authorization
            );
            assert!(
                s.runtime.capacity.lease(&lease).unwrap().is_some(),
                "signing is not execution completion"
            );
            assert_eq!(s.proposals.expire_unsigned().await.unwrap(), 0);
            let (_bridge, mut buyer_channel, mut provider_channel, value) =
                pair.finish(&peer, signed).await;
            s.buyer
                .retain_provider_acceptance(value.authorization)
                .await
                .unwrap();
            let executor = make_executor(&f, &peer, &s.runtime, &s.journal);
            let request = deliver(&mut buyer_channel, &mut provider_channel, &bytes).await;
            assert!(provider_channel
                .session()
                .execute_json(request, &executor, &Cancellation::default())
                .await
                .is_err());
            assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
            let recovery = recovery(&f, &peer);
            s.buyer
                .publish(saved.key().clone(), &recovery, 1002)
                .await
                .unwrap();
            let request = deliver(&mut buyer_channel, &mut provider_channel, &bytes).await;
            let done = provider_channel
                .session()
                .execute_json(request, &executor, &Cancellation::default())
                .await
                .unwrap();
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            assert!(executor
                .reconcile_capacity(&done.attempt.invocation, done.attempt.attempt)
                .await
                .unwrap());
            assert_eq!(s.runtime.capacity.status(&d(201)).unwrap().available, 2);
            assert!(s.runtime.capacity.signing_intent(&lease).unwrap().is_none());
            peer.stop().await;
        }
    }
}

#[tokio::test]
async fn provider_proposals_recover_lost_reply_apply_quotas_and_cancel_only_unsigned_work() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &chat(), false, None).await;
    let mut bounds = limits();
    bounds.per_buyer = 1;
    let s = Controlled::new(&f, &mut peer, bounds, 128 * 1024 * 1024).await;
    let ctx = context(&peer);
    let (first, one) = s.start(&peer, ctx.clone(), &chat()).await.unwrap();
    drop(first); // caller lost proposal delivery; the controller still owns it
    let (_, same) = s.start(&peer, ctx.clone(), &chat()).await.unwrap();
    assert_eq!(one.capacity_lease, same.capacity_lease);
    assert_eq!(one.reservation_id, same.reservation_id);
    assert_eq!(s.proposals.status().await.unwrap().unsigned, 1);
    assert_eq!(
        s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
        1
    );
    let mut other = ctx.clone();
    other.billing_id = d(800);
    assert!(s.start(&peer, other, &chat()).await.is_err());
    let mut altered = ctx.clone();
    altered.session_id = d(801);
    assert!(s.start(&peer, altered.clone(), &chat()).await.is_err());
    assert!(s.proposals.cancel_unsigned(altered).await.is_err());
    assert!(s.proposals.cancel_unsigned(ctx.clone()).await.unwrap());
    assert!(!s.proposals.cancel_unsigned(ctx.clone()).await.unwrap());
    assert_eq!(s.proposals.status().await.unwrap().request_bytes, 0);
    assert_eq!(s.runtime.capacity.status(&d(201)).unwrap().available, 2);
    let (_, next) = s.start(&peer, ctx, &chat()).await.unwrap();
    assert_ne!(next.capacity_lease, one.capacity_lease);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(peer.command("status").await["publications"], 0);
    peer.stop().await;
}

#[tokio::test]
async fn provider_proposals_reject_malformed_requests_and_mismatched_runtime_before_allocation() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Fiat, &f, &chat(), false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let mut bad: Value = serde_json::from_slice(&chat()).unwrap();
    bad["messages"] = json!("not-an-array");
    let mut ctx = context(&peer);
    ctx.request_hash = Digest::new(mayhem_proto::endpoint_request_fingerprint(&bad)).unwrap();
    assert!(s
        .start(&peer, ctx, &serde_json::to_vec(&bad).unwrap())
        .await
        .is_err());
    assert_eq!(
        s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
        0
    );
    let wrong = Arc::new(Runtime {
        adapter: s.runtime.adapter.clone(),
        connection: s.runtime.connection.clone(),
        capacity: s.runtime.capacity.clone(),
        route: d(201),
        approved_policy: {
            let mut p = s.runtime.approved_policy.clone();
            p.allow_checkpoints = !p.allow_checkpoints;
            p
        },
    });
    let control = Proposals::new(wrong, s.provider.clone(), peer.client.clone(), limits()).unwrap();
    let ctx = context(&peer);
    let mut pair = open(&peer, ctx.clone(), 1024 * 1024).await;
    let request = transfer(
        &mut pair.buyer,
        &mut pair.provider,
        n::Message::Request {
            request: serde_json::from_slice(&chat()).unwrap(),
        },
    )
    .await;
    assert!(control.propose(ctx, request).await.is_err());
    assert_eq!(
        s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
        0
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    peer.stop().await;
}

#[tokio::test]
async fn provider_unsigned_proposal_expiry_releases_slot_but_failed_signing_does_not() {
    for fail_sign in [false, true] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut peer = Peer::start(ProxyRail::Tap, &f, &chat(), false, None).await;
        let mut bounds = limits();
        bounds.unsigned_lifetime = Duration::from_millis(250);
        let s = Controlled::new(
            &f,
            &mut peer,
            bounds,
            if fail_sign { 1 } else { 128 * 1024 * 1024 },
        )
        .await;
        let ctx = context(&peer);
        let (mut pair, proposal) = s.start(&peer, ctx.clone(), &chat()).await.unwrap();
        if fail_sign {
            let (_, offer) = s.offer(&peer, &chat(), &mut pair, proposal).await;
            assert!(s.proposals.accept(ctx.clone(), offer, 1001).await.is_err());
            assert_eq!(s.proposals.status().await.unwrap().signing_or_uncertain, 1);
            assert!(s.proposals.cancel_unsigned(ctx).await.is_err());
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            s.proposals.expire_unsigned().await.unwrap(),
            usize::from(!fail_sign)
        );
        assert_eq!(
            s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
            u32::from(fail_sign)
        );
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        assert_eq!(peer.command("status").await["publications"], 0);
        peer.stop().await;
    }
}

#[tokio::test]
async fn provider_concurrent_proposals_share_one_lease_and_cannot_recreate_signed_attempt() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &chat(), false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let ctx = context(&peer);
    let bytes = chat();
    let (a, b) = tokio::join!(
        s.start(&peer, ctx.clone(), &bytes),
        s.start(&peer, ctx.clone(), &bytes)
    );
    let (mut pair, a) = a.unwrap();
    let (_, b) = b.unwrap();
    assert_eq!(a.capacity_lease, b.capacity_lease);
    assert_eq!(s.proposals.status().await.unwrap().unsigned, 1);
    assert_eq!(
        s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
        1
    );
    let (_, offer) = s.offer(&peer, &bytes, &mut pair, a).await;
    let signed = s.proposals.accept(ctx.clone(), offer, 1001).await.unwrap();
    assert!(s.start(&peer, ctx.clone(), &bytes).await.is_err());
    assert_eq!(
        s.proposals
            .recover(&ctx)
            .await
            .unwrap()
            .unwrap()
            .authorization,
        signed.authorization
    );
    assert_eq!(
        s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
        1
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    peer.stop().await;
}

#[tokio::test]
async fn provider_proposals_share_native_capacity_and_failed_cleanup_keeps_recovery_entry() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Fiat, &f, &chat(), false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let cap = &s.runtime.capacity;
    cap.configure_route(capacity::Route {
        id: d(202),
        group: d(200),
        lane: capacity::Lane::Native,
        max_concurrency: 2,
    })
    .unwrap();
    capacity_ready(cap, capacity::Scope::Route(d(202)));
    let native = cap
        .reserve(
            &d(202),
            capacity::Work {
                invocation: d(830),
                request_hash: d(831),
            },
        )
        .unwrap();
    let ctx = context(&peer);
    let (_, proposal) = s.start(&peer, ctx.clone(), &chat()).await.unwrap();
    assert_eq!(cap.status(&d(201)).unwrap().available, 0);
    let mut next = ctx.clone();
    next.billing_id = d(832);
    assert!(s.start(&peer, next, &chat()).await.is_err());
    assert_eq!(cap.status(&d(202)).unwrap().route_occupied, 1);
    // Dispatch cannot bypass the durable signing fence. Then inject conflicting
    // trusted cleanup to verify that failed cancellation keeps its diagnostics.
    let lease = cap.lease(&proposal.capacity_lease).unwrap().unwrap();
    assert!(cap
        .dispatch_accepted(&lease.id, &lease.work, &d(201))
        .is_err());
    cap.complete(capacity::VerifiedCompletion {
        lease,
        evidence: d(888),
    })
    .unwrap();
    assert!(s.proposals.cancel_unsigned(ctx).await.is_err());
    assert_eq!(s.proposals.status().await.unwrap().unsigned, 1);
    assert_eq!(cap.status(&d(201)).unwrap().group_occupied, 1);
    cap.cancel_reserved(native).unwrap();
    assert_eq!(cap.status(&d(201)).unwrap().group_occupied, 0);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    peer.stop().await;
}

#[tokio::test]
async fn provider_proposal_request_budget_rejects_before_occupying_more_capacity() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tap, &f, &chat(), false, None).await;
    let mut bounds = limits();
    bounds.request_bytes = chat().len();
    bounds.total_request_bytes = chat().len();
    let s = Controlled::new(&f, &mut peer, bounds, 128 * 1024 * 1024).await;
    let ctx = context(&peer);
    let _ = s.start(&peer, ctx.clone(), &chat()).await.unwrap();
    let mut next = ctx;
    next.billing_id = d(833);
    assert!(s.start(&peer, next, &chat()).await.is_err());
    assert_eq!(
        s.proposals.status().await.unwrap().request_bytes,
        chat().len()
    );
    assert_eq!(
        s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
        1
    );
    peer.stop().await;
}

#[tokio::test]
async fn provider_controller_preserves_streaming_requests_on_all_llm_endpoints_and_rails() {
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
            let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
            let ctx = context(&peer);
            let (mut pair, proposal) = s.start(&peer, ctx.clone(), &bytes).await.unwrap();
            let (_, offer) = s.offer(&peer, &bytes, &mut pair, proposal).await;
            let signed = s.proposals.accept(ctx.clone(), offer, 1001).await.unwrap();
            signed
                .authorization
                .verify(mayhem_proxy::receipts::verify_signature)
                .unwrap();
            let record = s.journal.get(&ctx.invocation().unwrap()).unwrap().unwrap();
            assert_eq!(
                s.journal
                    .recover(&record.invocation, record.attempt)
                    .unwrap()
                    .request
                    .unwrap()
                    .body,
                bytes
            );
            assert_eq!(
                s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
                1
            );
            assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
            peer.stop().await;
        }
    }
}

#[tokio::test]
async fn provider_controller_restart_cannot_guess_an_orphaned_proposal_was_cancelled() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &chat(), false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let ctx = context(&peer);
    let (_, proposal) = s.start(&peer, ctx.clone(), &chat()).await.unwrap();
    let s = s.restart(&f, &peer);
    let cap = &s.runtime.capacity;
    let control = &s.proposals;
    assert_eq!(
        cap.lease(&proposal.capacity_lease).unwrap().unwrap().phase,
        capacity::Phase::Uncertain
    );
    assert_eq!(
        cap.status(&d(201)).unwrap().available,
        0,
        "restart requires fresh health independently"
    );
    capacity_ready(cap, capacity::Scope::Group(d(200)));
    capacity_ready(cap, capacity::Scope::Route(d(201)));
    assert_eq!(control.expire_unsigned().await.unwrap(), 0);
    let mut pair = open(&peer, ctx.clone(), 1024 * 1024).await;
    let request = transfer(
        &mut pair.buyer,
        &mut pair.provider,
        n::Message::Request {
            request: serde_json::from_slice(&chat()).unwrap(),
        },
    )
    .await;
    assert!(control.propose(ctx.clone(), request).await.is_err());
    assert_eq!(cap.status(&d(201)).unwrap().group_occupied, 1);
    let proof = control.reconcile_unsigned(None, 1).await.unwrap();
    assert_eq!((proof.examined, proof.released, proof.retained), (1, 1, 0));
    assert!(proof.next_after.is_none());
    assert!(cap.lease(&proposal.capacity_lease).unwrap().is_none());
    assert_eq!(
        control.reconcile_unsigned(None, 1).await.unwrap().released,
        0
    );
    let (_, replacement) = s.start(&peer, ctx, &chat()).await.unwrap();
    assert_ne!(replacement.capacity_lease, proposal.capacity_lease);
    assert_eq!(peer.command("status").await["publications"], 0);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    peer.stop().await;
}

#[tokio::test]
async fn unsigned_restart_reconciliation_is_paged_and_preserves_native_and_legacy_reservations() {
    for native in [false, true] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut peer = Peer::start(ProxyRail::Fiat, &f, &chat(), false, None).await;
        let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
        let cap = &s.runtime.capacity;
        let route = if native {
            cap.configure_route(capacity::Route {
                id: d(202),
                group: d(200),
                lane: capacity::Lane::Native,
                max_concurrency: 2,
            })
            .unwrap();
            capacity_ready(cap, capacity::Scope::Route(d(202)));
            d(202)
        } else {
            d(201)
        };
        let protected = cap
            .reserve(
                &route,
                capacity::Work {
                    invocation: d(870),
                    request_hash: d(871),
                },
            )
            .unwrap();
        let (_, proposal) = s.start(&peer, context(&peer), &chat()).await.unwrap();
        assert_eq!(
            s.proposals
                .reconcile_unsigned(None, 64)
                .await
                .unwrap()
                .released,
            0,
            "live proposals cannot be reclaimed through startup recovery"
        );
        let s = s.restart(&f, &peer);
        let mut cursor = None;
        let mut released = 0;
        let mut retained = 0;
        let mut ended = false;
        for _ in 0..3 {
            let page = s.proposals.reconcile_unsigned(cursor, 1).await.unwrap();
            assert!(page.examined <= 1);
            released += page.released;
            retained += page.retained;
            cursor = page.next_after;
            if cursor.is_none() {
                ended = true;
                break;
            }
        }
        assert!(ended);
        assert_eq!((released, retained), (1, 1));
        let cap = &s.runtime.capacity;
        assert!(cap.lease(&proposal.capacity_lease).unwrap().is_none());
        assert!(cap.lease(&protected.lease().id).unwrap().is_some());
        assert_eq!(cap.status(&d(201)).unwrap().group_occupied, 1);
        assert!(s.proposals.reconcile_unsigned(None, 0).await.is_err());
        assert!(s.proposals.reconcile_unsigned(None, 65).await.is_err());
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        assert_eq!(peer.command("status").await["publications"], 0);
        peer.stop().await;
    }
}

#[tokio::test]
async fn restart_reconciliation_preserves_committed_and_uncertain_signing_obligations() {
    for failed in [false, true] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut peer = Peer::start(ProxyRail::Tap, &f, &chat(), false, None).await;
        let s = Controlled::new(
            &f,
            &mut peer,
            limits(),
            if failed { 1 } else { 128 * 1024 * 1024 },
        )
        .await;
        let ctx = context(&peer);
        let (mut pair, proposal) = s.start(&peer, ctx.clone(), &chat()).await.unwrap();
        let lease = proposal.capacity_lease.clone();
        assert_eq!(
            s.runtime.capacity.lease(&lease).unwrap().unwrap().phase,
            capacity::Phase::Proposed
        );
        let (saved, offer) = s.offer(&peer, &chat(), &mut pair, proposal).await;
        let result = s.proposals.accept(ctx.clone(), offer, 1001).await;
        assert_eq!(result.is_err(), failed);
        // The capacity fence commits before the journal or signer can fail.
        assert_eq!(
            s.runtime.capacity.lease(&lease).unwrap().unwrap().phase,
            capacity::Phase::Reserved
        );
        drop(pair);
        let s = s.restart(&f, &peer);
        assert!(
            s.runtime
                .capacity
                .signing_intent(&lease)
                .unwrap()
                .unwrap()
                .buyer()
                == &saved.offer()
        );
        let page = s.proposals.reconcile_unsigned(None, 64).await.unwrap();
        assert_eq!((page.released, page.retained), (0, 1));
        assert_eq!(
            s.runtime.capacity.status(&d(201)).unwrap().group_occupied,
            1
        );
        let recovered = s.proposals.recover(&ctx).await.unwrap();
        if let Ok(signed) = result {
            assert_eq!(recovered.unwrap().authorization, signed.authorization);
        } else {
            assert!(recovered.is_none());
        }
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        assert_eq!(peer.command("status").await["publications"], 0);
        peer.stop().await;
    }
}

#[tokio::test]
async fn canonical_non_admission_releases_signed_and_failed_signing_on_all_endpoints_and_rails() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for (endpoint, bytes, _) in cases() {
            for failed in [false, true] {
                let backend = backend(200, answer(), Duration::ZERO).await;
                let f = Fixture::new(&backend.base, endpoint);
                let mut peer = Peer::start(rail, &f, &bytes, false, None).await;
                let s = Controlled::new(
                    &f,
                    &mut peer,
                    limits(),
                    if failed { 1 } else { 128 * 1024 * 1024 },
                )
                .await;
                let ctx = context(&peer);
                let (mut pair, proposal) = s.start(&peer, ctx.clone(), &bytes).await.unwrap();
                let lease = proposal.capacity_lease.clone();
                let (saved, offer) = s.offer(&peer, &bytes, &mut pair, proposal).await;
                assert_eq!(
                    s.proposals.accept(ctx.clone(), offer, 1001).await.is_err(),
                    failed
                );
                let page = s.proposals.reconcile_signing(None, 64, 1002).await.unwrap();
                assert_eq!((page.released, page.retained, page.failed), (0, 1, 0));
                peer.command(&json!({"epoch":saved.offer().terms.billing_epoch}).to_string())
                    .await;
                // A bad proof cannot release a slot, sign, dispatch or change money.
                peer.command("nonce").await;
                let page = s.proposals.reconcile_signing(None, 64, 2000).await.unwrap();
                assert_eq!((page.released, page.retained, page.failed), (0, 1, 1));
                peer.command("reset").await;
                if failed {
                    let page = s.proposals.reconcile_signing(None, 64, 2000).await.unwrap();
                    assert_eq!(
                        (page.released, page.failed),
                        (0, 1),
                        "retirement must fit durable storage first"
                    );
                }
                let money = peer.command("state").await;
                drop(pair);
                drop(s);
                // Reopen every store/controller, restoring storage headroom after
                // the injected signing failure. No old in-memory capability survives.
                let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
                assert!(
                    s.runtime
                        .capacity
                        .signing_intent(&lease)
                        .unwrap()
                        .unwrap()
                        .buyer()
                        == &saved.offer()
                );
                let page = s.proposals.reconcile_signing(None, 64, 2001).await.unwrap();
                assert_eq!((page.released, page.retained, page.failed), (1, 0, 0));
                assert!(s.runtime.capacity.lease(&lease).unwrap().is_none());
                assert!(s.runtime.capacity.signing_intent(&lease).unwrap().is_none());
                assert_eq!(s.runtime.capacity.status(&d(201)).unwrap().available, 2);
                assert!(s.proposals.recover(&ctx).await.unwrap().is_none());
                assert!(
                    s.start(&peer, ctx.clone(), &bytes).await.is_err(),
                    "retired invocation cannot be proposed again"
                );
                let record = s.journal.get(&ctx.invocation().unwrap()).unwrap().unwrap();
                assert_eq!(record.phase, attempts::Phase::Closed);
                assert!(s
                    .journal
                    .begin_dispatch(&record.invocation, record.generation, 2002)
                    .is_err());
                assert_eq!(
                    s.proposals
                        .reconcile_retirements(None, 64, 2002)
                        .await
                        .unwrap()
                        .examined,
                    0
                );
                assert_eq!(peer.command("state").await, money);
                assert_eq!(peer.command("status").await["publications"], 0);
                assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
                assert_eq!(s.journal.prune_closed(3002, 64).unwrap(), 1);
                assert_eq!(s.journal.allocated_payload_bytes().unwrap(), 0);
                assert!(s.journal.get(&record.invocation).unwrap().is_none());
                peer.stop().await;
            }
        }
    }
}

#[tokio::test]
async fn provider_retirement_preserves_admitted_and_possible_execution_on_every_rail() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for admitted in [false, true] {
            let backend = backend(200, answer(), Duration::ZERO).await;
            let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
            let mut peer = Peer::start(rail, &f, &chat(), false, None).await;
            let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
            let ctx = context(&peer);
            let (mut pair, proposal) = s.start(&peer, ctx.clone(), &chat()).await.unwrap();
            let lease = proposal.capacity_lease.clone();
            let (saved, offer) = s.offer(&peer, &chat(), &mut pair, proposal).await;
            let signed = s.proposals.accept(ctx.clone(), offer, 1001).await.unwrap();
            if admitted {
                s.buyer
                    .retain_provider_acceptance(signed.authorization)
                    .await
                    .unwrap();
                s.buyer
                    .publish(saved.key().clone(), &recovery(&f, &peer), 1002)
                    .await
                    .unwrap();
            } else {
                // Even contradictory canonical absence cannot override local
                // possible-dispatch evidence. This must go to execution recovery.
                let record = s.journal.get(&ctx.invocation().unwrap()).unwrap().unwrap();
                s.journal
                    .begin_dispatch(&record.invocation, record.generation, 1002)
                    .unwrap();
            }
            peer.command(&json!({"epoch":saved.offer().terms.billing_epoch}).to_string())
                .await;
            let money = peer.command("state").await;
            let page = s.proposals.reconcile_signing(None, 64, 2000).await.unwrap();
            assert_eq!((page.released, page.retained), (0, 1));
            assert_eq!(page.failed, usize::from(!admitted));
            assert!(s.runtime.capacity.lease(&lease).unwrap().is_some());
            assert_eq!(peer.command("state").await, money);
            assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
            peer.stop().await;
        }
    }
}

// Reconstruct the durable boundary immediately before the final closure commit,
// retaining the real canonical proof and real signed buyer terms from the test.
fn reopen_retirement_closure(path: &std::path::Path, record: &attempts::Record) {
    use redb::{ReadableTable, TableDefinition};
    let db = redb::Database::open(path).unwrap();
    let tx = db.begin_write().unwrap();
    let key = format!("{}:{:020}", record.invocation.as_str(), record.attempt);
    let mut pending = record.clone();
    pending.phase = attempts::Phase::Resolved;
    pending.closure = None;
    pending.expires_at_ms = None;
    tx.open_table(TableDefinition::<&str, &[u8]>::new(
        "proxy_attempt_records_v1",
    ))
    .unwrap()
    .insert(
        key.as_str(),
        serde_json::to_vec(&pending).unwrap().as_slice(),
    )
    .unwrap();
    tx.open_table(TableDefinition::<&str, &str>::new(
        "proxy_attempt_expiry_v1",
    ))
    .unwrap()
    .remove(format!("{:020}:{}", record.expires_at_ms.unwrap(), key).as_str())
    .unwrap();
    tx.open_table(TableDefinition::<&str, u64>::new(
        "proxy_attempt_unfinished_v1",
    ))
    .unwrap()
    .insert(record.invocation.as_str(), record.attempt)
    .unwrap();
    {
        let mut table = tx
            .open_table(TableDefinition::<&str, &[u8]>::new("proxy_attempt_meta_v1"))
            .unwrap();
        let mut meta: Value =
            serde_json::from_slice(table.get("state").unwrap().unwrap().value()).unwrap();
        meta["unfinished"] = (meta["unfinished"].as_u64().unwrap() + 1).into();
        table
            .insert("state", serde_json::to_vec(&meta).unwrap().as_slice())
            .unwrap();
    }
    tx.commit().unwrap();
}

#[tokio::test]
async fn provider_retirement_recovers_both_sides_of_capacity_release_without_new_finance() {
    for restore_capacity in [false, true] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut peer = Peer::start(ProxyRail::Tnk, &f, &chat(), false, None).await;
        let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
        let ctx = context(&peer);
        let (mut pair, proposal) = s.start(&peer, ctx.clone(), &chat()).await.unwrap();
        let lease = proposal.capacity_lease.clone();
        let (saved, offer) = s.offer(&peer, &chat(), &mut pair, proposal).await;
        s.proposals.accept(ctx.clone(), offer, 1001).await.unwrap();
        peer.command(&json!({"epoch":saved.offer().terms.billing_epoch}).to_string())
            .await;
        drop(pair);
        drop(s);
        let path = f._store.path().join("controlled-capacity");
        let backup = f._store.path().join("capacity-before-retirement");
        if restore_capacity {
            std::fs::copy(&path, &backup).unwrap();
        }
        let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
        assert_eq!(
            s.proposals
                .reconcile_signing(None, 64, 2000)
                .await
                .unwrap()
                .released,
            1
        );
        let record = s.journal.get(&ctx.invocation().unwrap()).unwrap().unwrap();
        drop(s);
        reopen_retirement_closure(&f._store.path().join("controlled-journal"), &record);
        if restore_capacity {
            std::fs::copy(&backup, &path).unwrap();
        }
        let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
        assert_eq!(
            s.runtime.capacity.lease(&lease).unwrap().is_some(),
            restore_capacity
        );
        // Canonical endpoint deliberately unavailable: historical proof was
        // committed already. The journal cursor must finish with no fresh I/O.
        peer.command("nonce").await;
        let status = peer.command("status").await;
        let page = s
            .proposals
            .reconcile_retirements(None, 1, 2001)
            .await
            .unwrap();
        assert_eq!((page.examined, page.closed, page.failed), (1, 1, 0));
        assert!(s.runtime.capacity.lease(&lease).unwrap().is_none());
        assert!(s.proposals.recover(&ctx).await.unwrap().is_none());
        assert_eq!(
            s.journal.get(&record.invocation).unwrap().unwrap().phase,
            attempts::Phase::Closed
        );
        assert_eq!(
            s.proposals
                .reconcile_retirements(None, 1, 2002)
                .await
                .unwrap()
                .examined,
            0
        );
        assert_eq!(peer.command("status").await, status);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        assert_eq!(s.journal.prune_closed(3002, 64).unwrap(), 1);
        assert_eq!(s.journal.allocated_payload_bytes().unwrap(), 0);
        peer.stop().await;
    }
}

#[tokio::test]
async fn provider_reconciliation_is_paged_and_never_claims_unsigned_or_native_slots() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Fiat, &f, &chat(), false, None).await;
    let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
    let ctx = context(&peer);
    let (mut pair, proposal) = s.start(&peer, ctx.clone(), &chat()).await.unwrap();
    let lease = proposal.capacity_lease.clone();
    let (saved, offer) = s.offer(&peer, &chat(), &mut pair, proposal).await;
    s.proposals.accept(ctx, offer, 1001).await.unwrap();
    s.runtime
        .capacity
        .configure_route(capacity::Route {
            id: d(202),
            group: d(200),
            lane: capacity::Lane::Native,
            max_concurrency: 2,
        })
        .unwrap();
    capacity_ready(&s.runtime.capacity, capacity::Scope::Route(d(202)));
    let protected = s
        .runtime
        .capacity
        .reserve(
            &d(202),
            capacity::Work {
                invocation: d(300),
                request_hash: d(301),
            },
        )
        .unwrap();
    peer.command(&json!({"epoch":saved.offer().terms.billing_epoch}).to_string())
        .await;
    let mut after = None;
    let mut counts = (0, 0, 0);
    loop {
        let page = s.proposals.reconcile_signing(after, 1, 2000).await.unwrap();
        assert_eq!(page.examined, 1);
        assert_eq!(page.failed, 0);
        counts.0 += page.examined;
        counts.1 += page.released;
        counts.2 += page.retained;
        match page.next_after {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    assert_eq!(counts, (2, 1, 1));
    assert!(s.runtime.capacity.lease(&lease).unwrap().is_none());
    assert!(s
        .runtime
        .capacity
        .lease(&protected.lease().id)
        .unwrap()
        .is_some());
    assert!(s.proposals.reconcile_signing(None, 0, 2000).await.is_err());
    assert!(s.proposals.reconcile_signing(None, 65, 2000).await.is_err());
    assert!(s
        .proposals
        .reconcile_retirements(None, 65, 2000)
        .await
        .is_err());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    peer.stop().await;
}

#[tokio::test]
async fn caller_disconnect_does_not_abandon_signing_retirement_owner_task() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tap, &f, &chat(), false, None).await;
    let mut bounds = limits();
    bounds.storage_operations = 1;
    let s = Controlled::new(&f, &mut peer, bounds, 128 * 1024 * 1024).await;
    let ctx = context(&peer);
    let (mut pair, proposal) = s.start(&peer, ctx.clone(), &chat()).await.unwrap();
    let lease = proposal.capacity_lease.clone();
    let (saved, offer) = s.offer(&peer, &chat(), &mut pair, proposal).await;
    s.proposals.accept(ctx.clone(), offer, 1001).await.unwrap();
    peer.command(&json!({"epoch":saved.offer().terms.billing_epoch}).to_string())
        .await;
    peer.command("delay").await;
    let before = peer.command("status").await["calls"].as_u64().unwrap();
    let controller = s.proposals.clone();
    let caller = tokio::spawn(async move { controller.reconcile_signing(None, 64, 2000).await });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if peer.command("status").await["calls"].as_u64().unwrap() > before {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        s.proposals.reconcile_signing(None, 64, 2000).await.is_err(),
        "only one recovery owner per route"
    );
    caller.abort();
    assert!(matches!(caller.await, Err(e) if e.is_cancelled()));
    // Slow canonical recovery cannot occupy the only ordinary admission slot.
    let mut other = ctx.clone();
    other.billing_id = d(991);
    let (_other_pair, _) = s.start(&peer, other.clone(), &chat()).await.unwrap();
    assert!(s.proposals.cancel_unsigned(other).await.unwrap());
    loop {
        let closed = s
            .journal
            .get(&ctx.invocation().unwrap())
            .unwrap()
            .is_some_and(|r| r.phase == attempts::Phase::Closed);
        if closed && s.proposals.status().await.unwrap().signing_or_uncertain == 0 {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(s.runtime.capacity.lease(&lease).unwrap().is_none());
    assert_eq!(s.proposals.status().await.unwrap().request_bytes, 0);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(peer.command("status").await["publications"], 0);
    peer.stop().await;
}
