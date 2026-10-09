use super::*;
use mayhem_proxy::negotiation as n;
#[path = "proposal_control.rs"]
mod control;

pub(super) fn context(peer: &Peer) -> n::Context {
    let t = &peer.template.terms;
    let d = |s: &str| Digest::new(s).unwrap();
    n::Context {
        schema_version: 1,
        network_id: t.network_id.clone(),
        msb_bootstrap: d(&t.msb_bootstrap),
        subnet_bootstrap: d(&t.subnet_bootstrap),
        contract_version: t.contract_version,
        session_id: d(&t.session_id),
        buyer: d(&t.buyer_pubkey),
        billing_id: d(&t.billing_id),
        billing_attempt: t.billing_attempt,
        request_hash: d(&t.request_hash),
        offer: t.offer.clone(),
        rail: t.rail,
        settlement_policy_hash: d(&t.settlement_policy_hash),
    }
}
pub(super) struct Pair {
    bridge: Bridge,
    buyer: n::Channel,
    provider: n::Channel,
    context: n::Context,
}
async fn open(peer: &Peer, context: n::Context, bound: usize) -> Pair {
    let bridge = Bridge::start(context.buyer.as_str(), &context.offer.provider_pubkey).await;
    let provider = n::Channel::connect(
        bridge.config(false),
        context.clone(),
        &peer.identity,
        Role::Provider,
        mayhem_proxy::exchange::Limits {
            max_message_bytes: bound,
        },
    )
    .await
    .unwrap();
    let buyer = n::Channel::connect(
        bridge.config(true),
        context.clone(),
        &identity(peer),
        Role::Buyer,
        mayhem_proxy::exchange::Limits {
            max_message_bytes: bound,
        },
    )
    .await
    .unwrap();
    Pair {
        bridge,
        buyer,
        provider,
        context,
    }
}
async fn transfer(
    sender: &mut n::Channel,
    receiver: &mut n::Channel,
    message: n::Message,
) -> n::Received {
    let (sent, received) = tokio::join!(
        sender.send(&message),
        receiver.receive(Duration::from_secs(5))
    );
    sent.unwrap();
    received.unwrap()
}
pub(super) async fn begin(
    peer: &Peer,
    f: &Fixture,
    bytes: &[u8],
    binding: &SessionBinding,
    buyer: &BuyerNegotiation,
    signer: Arc<Authority>,
) -> (SavedPurchase, Pair) {
    let context = context(peer);
    let mut pair = open(peer, context.clone(), 1024 * 1024).await;
    assert_eq!(
        context.invocation().unwrap(),
        mayhem_proxy::exchange::invocation(&peer.template).unwrap()
    );
    let request = transfer(
        &mut pair.buyer,
        &mut pair.provider,
        n::Message::Request {
            request: serde_json::from_slice(bytes).unwrap(),
        },
    )
    .await
    .into_message(&context, Role::Provider)
    .unwrap();
    let n::Message::Request { request } = request else {
        panic!("request required")
    };
    assert_eq!(
        mayhem_proto::endpoint_request_fingerprint(&request),
        context.request_hash.as_str()
    );
    let proposal = n::Proposal {
        adapter: f.adapter.public_snapshot(),
        reservation_id: binding.reservation_id.clone(),
        connection_digest: binding.connection_digest.clone(),
        connection_revision: f.connection.revision(),
        capacity_lease: binding.capacity_lease.clone(),
    };
    let proposal = transfer(
        &mut pair.provider,
        &mut pair.buyer,
        n::Message::Proposal { proposal },
    )
    .await
    .into_message(&context, Role::Buyer)
    .unwrap();
    let n::Message::Proposal { mut proposal } = proposal else {
        panic!("proposal required")
    };
    // Resource allowances belong to the buyer, not the remote provider.
    proposal.adapter.limits = Limits {
        request_bytes: 512 * 1024,
        response_bytes: 512 * 1024,
        choices: 8,
        tools: 16,
        questions: 16,
        decision_options: 32,
    };
    let output = (f.adapter.endpoint() != ProxyEndpoint::Decisions).then_some(37);
    let intent = PurchaseRequest::new(
        proposal.adapter.clone(),
        bytes.to_vec(),
        prices(peer),
        output,
        lifetimes(),
    )
    .unwrap();
    let q = peer.buyer_client.quote(&query(peer)).await.unwrap();
    let p = q
        .prepare_purchase(&intent, &proposal.session_binding(&context).unwrap())
        .unwrap();
    let saved = buyer.sign(p, q, signer, 1000).await.unwrap();
    let offer = transfer(
        &mut pair.buyer,
        &mut pair.provider,
        n::Message::Offer {
            offer: saved.offer(),
        },
    )
    .await
    .into_message(&context, Role::Provider)
    .unwrap();
    let n::Message::Offer { offer } = offer else {
        panic!("offer required")
    };
    assert_eq!(offer.terms, saved.offer().terms);
    assert_eq!(offer.buyer_sig, saved.offer().buyer_sig);
    (saved, pair)
}
impl Pair {
    pub(super) async fn finish(
        mut self,
        peer: &Peer,
        value: attempts::SignedProviderAcceptance,
    ) -> (Bridge, Channel, Channel, attempts::SignedProviderAcceptance) {
        let received = transfer(
            &mut self.provider,
            &mut self.buyer,
            n::Message::Accepted { value },
        )
        .await
        .into_message(&self.context, Role::Buyer)
        .unwrap();
        let n::Message::Accepted { value } = received else {
            panic!("acceptance required")
        };
        let buyer = self.buyer.into_paid(&identity(peer)).unwrap();
        let provider = self.provider.into_paid(&peer.identity).unwrap();
        (self.bridge, buyer, provider, value)
    }
}

#[tokio::test]
async fn negotiation_recovery_returns_original_saved_signatures_without_funding_or_inference() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &chat(), false, None).await;
    let mut s = Setup::new(&f, &mut peer, &chat()).await;
    let approval = s.approve(&peer, s.saved.offer(), chat()).await.unwrap();
    let invocation = approval.invocation().clone();
    let signed = s.provider.accept(approval, 1001).await.unwrap();
    // The acceptance reaches the buyer but the provider loses its transport ACK.
    // Neither side may create a fresh purchase or assume no obligation exists.
    let mut original = s.negotiation.take().unwrap();
    *original.bridge.attack.lock().await = Some("lost_ack");
    let message = n::Message::Accepted {
        value: signed.clone(),
    };
    let (sent, received) = tokio::join!(
        original.provider.send(&message),
        original.buyer.receive(Duration::from_secs(5))
    );
    assert!(sent.is_err());
    let n::Message::Accepted { value } = received
        .unwrap()
        .into_message(&original.context, Role::Buyer)
        .unwrap()
    else {
        panic!("acceptance required")
    };
    let retained = s
        .buyer
        .retain_provider_acceptance(value.authorization)
        .await
        .unwrap();
    assert_eq!(retained.authorization(), Some(signed.authorization.clone()));
    assert!(matches!(
        original.provider.send(&message).await,
        Err(mayhem_proxy::exchange::Error::Interrupted)
    ));
    drop(original);
    // A new transport recovers exactly the original saved signatures, without
    // signing, publishing a reservation, or starting inference again.
    let mut pair = open(&peer, context(&peer), 1024 * 1024).await;
    let received = transfer(&mut pair.buyer, &mut pair.provider, n::Message::Recover).await;
    assert!(matches!(
        received
            .into_message(&pair.context, Role::Provider)
            .unwrap(),
        n::Message::Recover
    ));
    let saved = s
        .provider
        .recover(invocation.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.authorization, signed.authorization);
    let (_, buyer, provider, received) = pair.finish(&peer, saved).await;
    assert_eq!(received.authorization, signed.authorization);
    assert_eq!(buyer.session().invocation(), &invocation);
    assert_eq!(provider.session().invocation(), &invocation);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(peer.command("status").await["publications"], 0);
    peer.stop().await;
}

#[tokio::test]
async fn negotiation_rejects_wrong_stage_request_context_identity_and_premature_promotion() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tap, &f, &chat(), false, None).await;
    let s = Setup::new(&f, &mut peer, &chat()).await;
    let ctx = context(&peer);
    let bridge = Bridge::start(ctx.buyer.as_str(), &ctx.offer.provider_pubkey).await;
    let limits = mayhem_proxy::exchange::Limits {
        max_message_bytes: 1024 * 1024,
    };
    assert!(n::Channel::connect(
        bridge.config(true),
        ctx.clone(),
        &peer.identity,
        Role::Buyer,
        limits
    )
    .await
    .is_err());
    let mut pair = open(&peer, ctx.clone(), 1024 * 1024).await;
    assert!(pair
        .buyer
        .send(&n::Message::Offer {
            offer: s.saved.offer()
        })
        .await
        .is_err());
    assert!(pair.provider.send(&n::Message::Recover).await.is_err());
    let mut bad: Value = serde_json::from_slice(&chat()).unwrap();
    bad["model"] = json!("wrong-request");
    assert!(pair
        .buyer
        .send(&n::Message::Request { request: bad })
        .await
        .is_err());
    let received = transfer(
        &mut pair.buyer,
        &mut pair.provider,
        n::Message::Request {
            request: serde_json::from_slice(&chat()).unwrap(),
        },
    )
    .await;
    let mut wrong = ctx.clone();
    wrong.billing_id = d(777);
    assert!(received.into_message(&wrong, Role::Provider).is_err());
    assert!(pair.buyer.into_paid(&identity(&peer)).is_err());
    assert!(pair.provider.into_paid(&peer.identity).is_err());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    peer.stop().await;
}

#[tokio::test]
async fn negotiation_rejects_tampered_frames_and_payloads_and_poisoned_channel_cannot_continue() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let peer = Peer::start(ProxyRail::Fiat, &f, &chat(), false, None).await;
    for attack in [
        "remote",
        "terms",
        "negotiation",
        "purpose",
        "offset",
        "sequence",
        "size",
        "digest",
        "extra",
        "lineage",
        "request_payload",
        "duplicate",
    ] {
        let mut pair = open(&peer, context(&peer), 1024 * 1024).await;
        *pair.bridge.attack.lock().await = Some(attack);
        let message = n::Message::Request {
            request: serde_json::from_slice(&chat()).unwrap(),
        };
        let (sent, received) = tokio::join!(
            pair.buyer.send(&message),
            pair.provider.receive(Duration::from_secs(5))
        );
        sent.unwrap();
        if attack == "duplicate" {
            assert!(received.is_ok());
            assert!(pair.provider.receive(Duration::from_secs(5)).await.is_err());
        } else {
            assert!(received.is_err(), "{attack}");
        }
        assert!(
            matches!(
                pair.provider.receive(Duration::from_millis(1)).await,
                Err(mayhem_proxy::exchange::Error::Interrupted)
            ),
            "{attack}"
        );
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    peer.stop().await;
}

#[tokio::test]
async fn negotiation_checks_real_signatures_even_when_transport_digest_is_valid() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tap, &f, &chat(), false, None).await;
    let mut s = Setup::new(&f, &mut peer, &chat()).await;
    let approved = s.approve(&peer, s.saved.offer(), chat()).await.unwrap();
    let signed = s.provider.accept(approved, 1001).await.unwrap();
    let mut pair = s.negotiation.take().unwrap();
    *pair.bridge.attack.lock().await = Some("signature_payload");
    let message = n::Message::Accepted { value: signed };
    let (sent, received) = tokio::join!(
        pair.provider.send(&message),
        pair.buyer.receive(Duration::from_secs(5))
    );
    sent.unwrap();
    assert!(matches!(
        received,
        Err(mayhem_proxy::exchange::Error::Identity)
    ));
    assert!(pair.buyer.into_paid(&identity(&peer)).is_err());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(peer.command("status").await["publications"], 0);
    peer.stop().await;
}

#[tokio::test]
async fn negotiation_fragments_large_requests_with_bounded_frames_and_enforces_control_deadline() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut request: Value = serde_json::from_slice(&chat()).unwrap();
    request["messages"][0]["content"] = json!("large-request-".repeat(20000));
    let bytes = serde_json::to_vec(&request).unwrap();
    let peer = Peer::start(ProxyRail::Tnk, &f, &bytes, false, None).await;
    let ctx = context(&peer);
    let mut pair = open(&peer, ctx.clone(), 512 * 1024).await;
    let received = transfer(
        &mut pair.buyer,
        &mut pair.provider,
        n::Message::Request {
            request: request.clone(),
        },
    )
    .await;
    let n::Message::Request { request: received } =
        received.into_message(&ctx, Role::Provider).unwrap()
    else {
        panic!("request required")
    };
    assert_eq!(received, request);
    let frames = pair.bridge.frames.lock().await;
    assert!(frames.len() > 1);
    assert!(frames
        .iter()
        .all(|v| serde_json::to_vec(&v["frame"]).unwrap().len() <= 64 * 1024));
    assert!(frames
        .iter()
        .all(|v| v["frame"].get("accepted_terms").is_none()));
    drop(frames);
    drop(pair);
    let mut small = open(&peer, ctx, 1024).await;
    assert!(small
        .buyer
        .send(&n::Message::Request { request })
        .await
        .is_err());
    assert!(small.bridge.frames.lock().await.is_empty());
    assert!(small
        .provider
        .receive(Duration::from_millis(20))
        .await
        .is_err());
    assert!(matches!(
        small.provider.receive(Duration::from_millis(20)).await,
        Err(mayhem_proxy::exchange::Error::Interrupted)
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    peer.stop().await;
}
