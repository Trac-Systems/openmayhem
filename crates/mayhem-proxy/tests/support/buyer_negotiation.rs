use super::*;
use financial::negotiation::{
    BuyerNegotiation, Limits as NegotiationLimits, Store as NegotiationStore,
};
use mayhem_proxy::signing::Authority;
fn identity(peer: &Peer) -> Identity {
    let mut id = peer.identity.clone();
    id.controller_pubkey = Digest::new(&peer.template.terms.buyer_pubkey).unwrap();
    id
}
fn bounds(max_records: u64) -> NegotiationLimits {
    NegotiationLimits {
        max_records,
        max_payload_bytes: 256 * 1024,
        max_record_bytes: 128 * 1024,
        closed_retention_ms: 1000,
    }
}
fn controller(f: &Fixture, peer: &Peer, max_records: u64) -> BuyerNegotiation {
    BuyerNegotiation::new(
        Arc::new(
            NegotiationStore::open(
                f._store.path().join("negotiation"),
                identity(peer),
                bounds(max_records),
            )
            .unwrap(),
        ),
        peer.buyer_client.clone(),
        4,
    )
    .unwrap()
}
async fn prepare(
    peer: &Peer,
    f: &Fixture,
    bytes: &[u8],
    binding: &SessionBinding,
) -> (
    financial::quote::Observation,
    financial::quote::PreparedPurchase,
) {
    let q = peer.buyer_client.quote(&query(peer)).await.unwrap();
    let output = (f.adapter.endpoint() != ProxyEndpoint::Decisions).then_some(37);
    let intent = PurchaseRequest::new(
        f.adapter.snapshot(),
        bytes.to_vec(),
        prices(peer),
        output,
        lifetimes(),
    )
    .unwrap();
    let p = q.prepare_purchase(&intent, binding).unwrap();
    (q, p)
}
async fn signer(peer: &mut Peer) -> (Arc<Authority>, [u8; 32], [u8; 32]) {
    let keys = peer.command("ephemeral_test_wallet_seeds").await;
    let b: [u8; 32] = serde_json::from_value(keys["buyer"].clone()).unwrap();
    let p: [u8; 32] = serde_json::from_value(keys["provider"].clone()).unwrap();
    (
        Arc::new(
            Authority::from_unlocked_wallet(SigningKey::from_bytes(&b), identity(peer)).unwrap(),
        ),
        b,
        p,
    )
}
fn accepted(offer: financial::negotiation::BuyerOffer, p: &[u8; 32]) -> ProxySpendAuthorization {
    let provider_sig = signature(p, &offer.terms.provider_signing_bytes().unwrap());
    ProxySpendAuthorization {
        terms: offer.terms,
        buyer_sig: offer.buyer_sig,
        provider_sig,
    }
}
fn recovery(f: &Fixture, peer: &Peer) -> BuyerRecovery {
    BuyerRecovery::new(
        Arc::new(
            Store::open(
                f._store.path().join("negotiation-recovery"),
                identity(peer),
                RecoveryLimits {
                    max_records: 8,
                    closed_retention_ms: 1000,
                },
            )
            .unwrap(),
        ),
        peer.buyer_client.clone(),
        4,
    )
    .unwrap()
}
async fn waive(peer: &mut Peer, auth: &ProxySpendAuthorization) {
    use mayhem_proto::proxy::finance::{
        ProxyClosureBody, ProxyClosureOutcome, ProxyReservationClosure,
    };
    let body = ProxyClosureBody {
        schema_version: 1,
        lane: mayhem_proto::proxy::ProxyLane::Proxy,
        accepted_terms: auth.terms.digest().unwrap(),
        outcome: ProxyClosureOutcome::NotExecuted,
        evidence_hash: "e".repeat(64),
        at_ms: 2000,
    };
    let sigs = peer
        .command(&json!({"sign_waiver": body}).to_string())
        .await;
    let closure = ProxyReservationClosure {
        body,
        buyer_sig: sigs["buyer_sig"].as_str().unwrap().into(),
        provider_sig: sigs["provider_sig"].as_str().unwrap().into(),
    };
    peer.client.submit_waiver(auth, &closure).await.unwrap();
}
#[tokio::test]
async fn negotiation_signs_owned_purchases_reopens_and_publishes_once_on_every_endpoint_and_rail() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        for (endpoint, bytes, _) in cases() {
            let backend = backend(200, answer(), Duration::ZERO).await;
            let f = Fixture::new(&backend.base, endpoint);
            let mut peer = Peer::start(rail, &f, &bytes, false, None).await;
            let (signer, _, provider_key) = signer(&mut peer).await;
            let c = controller(&f, &peer, 1);
            let (q, p) = prepare(&peer, &f, &bytes, &session(&peer)).await;
            let saved = c.sign(p, q, signer, 1000).await.unwrap();
            let offer = saved.offer();
            offer.verify().unwrap();
            assert_eq!(saved.request(), bytes);
            assert!(!saved.confirmed());
            assert!(saved.authorization().is_none());
            assert_eq!(
                c.prune(999_999, 64).await.unwrap(),
                0,
                "time cannot delete a signed unresolved purchase"
            );
            let key = saved.key().clone();
            drop(c);
            let c = controller(&f, &peer, 1);
            let saved = c
                .lookup(
                    Digest::new(&offer.terms.billing_id).unwrap(),
                    offer.terms.billing_attempt,
                )
                .await
                .unwrap()
                .unwrap();
            assert_eq!(saved.offer().buyer_sig, offer.buyer_sig);
            assert_eq!(saved.request(), bytes);
            let auth = accepted(offer, &provider_key);
            let b = recovery(&f, &peer);
            assert!(
                c.publish(key.clone(), &b, 1001).await.is_err(),
                "provider signature is mandatory"
            );
            c.retain_provider_acceptance(auth.clone()).await.unwrap();
            let saved = c.publish(key.clone(), &b, 1002).await.unwrap();
            assert!(saved.confirmed() && !saved.closed());
            c.publish(key.clone(), &b, 1003).await.unwrap();
            assert_eq!(peer.command("status").await["publications"], 1);
            assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
            assert_eq!(c.pending(None, 1).await.unwrap(), vec![key.clone()]);
            assert!(c.pending(Some(key.clone()), 1).await.unwrap().is_empty());
            waive(&mut peer, &auth).await;
            let closed = c.refresh(key.clone(), 2000).await.unwrap();
            assert!(closed.closed());
            assert!(c.pending(None, 64).await.unwrap().is_empty());
            assert_eq!(c.prune(2999, 64).await.unwrap(), 0);
            assert_eq!(c.prune(3000, 1).await.unwrap(), 1);
            assert!(c.recover(key).await.unwrap().is_none());
            peer.stop().await;
        }
    }
}

#[tokio::test]
async fn negotiation_serializes_conflicting_signatures_and_rejects_provider_substitution_and_quota_overrun(
) {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let bytes = chat();
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &bytes, false, None).await;
    let (signer, buyer_key, provider_key) = signer(&mut peer).await;
    let c = controller(&f, &peer, 1);
    assert!(
        NegotiationStore::open(
            f._store.path().join("negotiation"),
            identity(&peer),
            bounds(1)
        )
        .is_err(),
        "single writer"
    );
    let (qa, a) = prepare(&peer, &f, &bytes, &session(&peer)).await;
    let (qb, b) = prepare(&peer, &f, &bytes, &session(&peer)).await;
    let (a, b) = tokio::join!(
        c.sign(a, qa, signer.clone(), 1000),
        c.sign(b, qb, signer.clone(), 1001)
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.key(), b.key());
    assert_eq!(a.offer().buyer_sig, b.offer().buyer_sig);
    let mut alternate = session(&peer);
    alternate.reservation_id = d(2000);
    let (q, p) = prepare(&peer, &f, &bytes, &alternate).await;
    assert!(
        c.sign(p, q, signer.clone(), 1002).await.is_err(),
        "same billing attempt must not authorize another reservation"
    );
    let mut changed = a.offer();
    changed.terms.reservation_id = d(2001).as_str().into();
    changed.buyer_sig = signature(&buyer_key, &changed.terms.buyer_signing_bytes().unwrap());
    assert!(
        c.retain_provider_acceptance(accepted(changed, &provider_key))
            .await
            .is_err(),
        "even valid alternate signatures cannot replace owned terms"
    );
    let mut invalid = accepted(a.offer(), &provider_key);
    invalid.provider_sig = "0".repeat(128);
    assert!(c.retain_provider_acceptance(invalid).await.is_err());
    let mut another = query(&peer);
    another.billing_id = d(2002).as_str().into();
    let q = peer.buyer_client.quote(&another).await.unwrap();
    let intent = PurchaseRequest::new(
        f.adapter.snapshot(),
        bytes.clone(),
        prices(&peer),
        Some(37),
        lifetimes(),
    )
    .unwrap();
    let p = q.prepare_purchase(&intent, &session(&peer)).unwrap();
    assert!(
        c.sign(p, q, signer.clone(), 1003).await.is_err(),
        "new intent must respect quota"
    );
    let (q, p) = prepare(&peer, &f, &bytes, &session(&peer)).await;
    let foreign = Arc::new(
        Authority::from_unlocked_wallet(
            SigningKey::from_bytes(&provider_key),
            peer.identity.clone(),
        )
        .unwrap(),
    );
    assert!(c.sign(p, q, foreign, 1003).await.is_err());
    assert_eq!(c.pending(None, 64).await.unwrap(), vec![a.key().clone()]);
    let b = recovery(&f, &peer);
    peer.command("publish_lost_ack").await;
    let auth = accepted(a.offer(), &provider_key);
    c.retain_provider_acceptance(auth.clone()).await.unwrap();
    assert!(c
        .publish(a.key().clone(), &b, 1100)
        .await
        .unwrap()
        .confirmed());
    peer.command("flush_publication").await;
    assert_eq!(peer.command("status").await["publications"], 1);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    peer.stop().await;
}

#[tokio::test]
async fn negotiation_pending_publication_survives_reopen_without_a_second_hold() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let bytes = chat();
    let mut peer = Peer::start(ProxyRail::Tap, &f, &bytes, false, None).await;
    let (signer, _, provider_key) = signer(&mut peer).await;
    let c = controller(&f, &peer, 1);
    let (q, p) = prepare(&peer, &f, &bytes, &session(&peer)).await;
    let saved = c.sign(p, q, signer, 1000).await.unwrap();
    let key = saved.key().clone();
    let auth = accepted(saved.offer(), &provider_key);
    c.retain_provider_acceptance(auth.clone()).await.unwrap();
    let b = recovery(&f, &peer);
    peer.command("publish_pending").await;
    assert!(c.publish(key.clone(), &b, 1001).await.is_err());
    drop(c);
    drop(b);
    let c = controller(&f, &peer, 1);
    let b = recovery(&f, &peer);
    assert_eq!(
        c.recover(key.clone())
            .await
            .unwrap()
            .unwrap()
            .authorization()
            .unwrap(),
        auth
    );
    peer.command("flush_publication").await;
    assert!(c.publish(key, &b, 1002).await.unwrap().confirmed());
    assert_eq!(peer.command("status").await["publications"], 1);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    peer.stop().await;
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "subprocess helper, executed by the abrupt-exit parent test"]
async fn negotiation_abrupt_exit_child() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("MAYHEM_TEST_NEGOTIATION_CRASH").expect("private test directory"),
    );
    let auth: ProxySpendAuthorization =
        serde_json::from_slice(&std::fs::read(directory.join("acceptance.json")).unwrap()).unwrap();
    let t = &auth.terms;
    let id = Identity {
        network_id: t.network_id.clone(),
        msb_bootstrap: Digest::new(&t.msb_bootstrap).unwrap(),
        subnet_bootstrap: Digest::new(&t.subnet_bootstrap).unwrap(),
        controller_pubkey: Digest::new(&t.buyer_pubkey).unwrap(),
    };
    let client = Arc::new(
        financial::Client::new(
            "http://127.0.0.1:9/v1",
            discovery::Identity {
                network_id: t.network_id.clone(),
                msb_bootstrap: t.msb_bootstrap.clone(),
                subnet_bootstrap: t.subnet_bootstrap.clone(),
                contract_version: t.contract_version,
            },
            t.buyer_pubkey.clone(),
            1,
        )
        .unwrap(),
    );
    let c = BuyerNegotiation::new(
        Arc::new(NegotiationStore::open(directory.join("negotiation"), id, bounds(1)).unwrap()),
        client,
        1,
    )
    .unwrap();
    c.retain_provider_acceptance(auth).await.unwrap();
    // Deliberately bypass Rust destructors and graceful DB shutdown.
    std::process::exit(73);
}

#[cfg(unix)]
#[tokio::test]
async fn negotiation_abrupt_process_exit_keeps_original_signatures_and_owned_request() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let bytes = chat();
    let mut peer = Peer::start(ProxyRail::Fiat, &f, &bytes, false, None).await;
    let (signer, _, provider_key) = signer(&mut peer).await;
    let c = controller(&f, &peer, 1);
    let (q, p) = prepare(&peer, &f, &bytes, &session(&peer)).await;
    let saved = c.sign(p, q, signer, 1000).await.unwrap();
    let key = saved.key().clone();
    let auth = accepted(saved.offer(), &provider_key);
    drop(c);
    std::fs::write(
        f._store.path().join("acceptance.json"),
        serde_json::to_vec(&auth).unwrap(),
    )
    .unwrap();
    let mut process = tokio::process::Command::new(std::env::current_exe().unwrap());
    process
        .args([
            "--ignored",
            "--exact",
            "paid_acceptance::purchase::negotiation::negotiation_abrupt_exit_child",
            "--nocapture",
        ])
        .env("MAYHEM_TEST_NEGOTIATION_CRASH", f._store.path())
        .kill_on_drop(true);
    let result = tokio::time::timeout(Duration::from_secs(30), process.output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(73),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let c = controller(&f, &peer, 1);
    let saved = c.recover(key.clone()).await.unwrap().unwrap();
    assert_eq!(saved.authorization().unwrap(), auth);
    assert_eq!(saved.request(), bytes);
    assert!(!saved.confirmed());
    let b = recovery(&f, &peer);
    assert!(c.publish(key, &b, 1001).await.unwrap().confirmed());
    assert_eq!(peer.command("status").await["publications"], 1);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    peer.stop().await;
}

#[tokio::test]
async fn negotiation_reserved_storage_can_finish_after_limits_are_lowered() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let bytes = chat();
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &bytes, false, None).await;
    let (signer, _, provider_key) = signer(&mut peer).await;
    let c = controller(&f, &peer, 4);
    let (q, p) = prepare(&peer, &f, &bytes, &session(&peer)).await;
    let first = c.sign(p, q, signer.clone(), 1000).await.unwrap();
    let mut other = query(&peer);
    other.billing_id = d(4000).as_str().into();
    let q = peer.buyer_client.quote(&other).await.unwrap();
    let intent = PurchaseRequest::new(
        f.adapter.snapshot(),
        bytes.clone(),
        prices(&peer),
        Some(37),
        lifetimes(),
    )
    .unwrap();
    let p = q.prepare_purchase(&intent, &session(&peer)).unwrap();
    c.sign(p, q, signer.clone(), 1001).await.unwrap();
    assert_eq!(c.pending(None, 64).await.unwrap().len(), 2);
    drop(c);
    let reduced = NegotiationLimits {
        max_records: 1,
        max_payload_bytes: 1024,
        max_record_bytes: 1024,
        closed_retention_ms: 1000,
    };
    let c = BuyerNegotiation::new(
        Arc::new(
            NegotiationStore::open(
                f._store.path().join("negotiation"),
                identity(&peer),
                reduced,
            )
            .unwrap(),
        ),
        peer.buyer_client.clone(),
        4,
    )
    .unwrap();
    let auth = accepted(first.offer(), &provider_key);
    c.retain_provider_acceptance(auth.clone()).await.unwrap();
    let b = recovery(&f, &peer);
    assert!(c
        .publish(first.key().clone(), &b, 1100)
        .await
        .unwrap()
        .confirmed());
    waive(&mut peer, &auth).await;
    assert!(c.refresh(first.key().clone(), 2000).await.unwrap().closed());
    assert_eq!(c.prune(3000, 64).await.unwrap(), 1);
    assert_eq!(
        c.pending(None, 64).await.unwrap().len(),
        1,
        "unsigned second purchase remains unresolved"
    );
    other.billing_id = d(4001).as_str().into();
    let q = peer.buyer_client.quote(&other).await.unwrap();
    let p = q.prepare_purchase(&intent, &session(&peer)).unwrap();
    assert!(
        c.sign(p, q, signer, 3001).await.is_err(),
        "lower limits still prevent new obligations"
    );
    peer.stop().await;
}
