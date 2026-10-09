use super::*;
use mayhem_proxy::buyer::{Evidence, Snapshot};

#[tokio::test]
async fn public_buyer_contract_rejects_substitution_and_never_serializes_private_mapping() {
    for (endpoint, bytes, _) in cases() {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let f = Fixture::new(&backend.base, endpoint);
        let peer = Peer::start(ProxyRail::Tap, &f, &bytes, false, None).await;
        let (_, purchase) = prepare(&peer, &f, &bytes, &session(&peer)).await;
        let t = purchase.terms();
        let digest = |s: &str| Digest::new(s).unwrap();
        let binding = Binding {
            request_hash: digest(&t.request_hash),
            endpoint: t.offer.endpoint,
            contract_version: t.contract_version,
            provider_pubkey: digest(&t.offer.provider_pubkey),
            market_id: digest(&t.offer.market_id),
            offer_digest: digest(&t.offer.digest().unwrap()),
            endpoint_contract: digest(&t.endpoint_contract),
            metering_policy: digest(&t.offer.metering_policy_hash),
            accepted_terms: digest(&t.digest().unwrap()),
            reservation: digest(&t.reservation_id),
            capacity_lease: digest(&t.capacity_lease),
            connection_digest: digest(&t.connection_digest),
            connection_revision: t.connection_revision,
            recipe_digest: digest(&t.recipe_hash),
            rail: t.rail,
        };
        let snapshot = purchase.snapshot().public_snapshot().unwrap();
        snapshot.verify_request(&binding, &bytes).unwrap();
        let encoded = serde_json::to_value(&snapshot).unwrap();
        assert!(encoded["adapter"].get("upstream_model").is_none());
        assert_eq!(snapshot.adapter.recipe_hash, *f.adapter.recipe_hash());
        assert_ne!(
            snapshot.adapter.limits.request_bytes,
            f.adapter.limits().request_bytes
        );
        for fault in [
            "version",
            "adapter_version",
            "contract",
            "recipe",
            "price",
            "limit",
        ] {
            let mut wrong = snapshot.clone();
            match fault {
                "version" => wrong.version += 1,
                "adapter_version" => wrong.adapter.version += 1,
                "contract" => wrong.adapter.contract.family = "not-the-agreed-contract".into(),
                "recipe" => wrong.adapter.recipe_hash = d(211),
                "price" => wrong.offer.per_request_au += 1,
                "limit" => wrong.adapter.limits.request_bytes = 1,
                _ => unreachable!(),
            }
            assert!(
                wrong.verify_request(&binding, &bytes).is_err(),
                "{endpoint:?}/{fault}"
            );
        }
        let mut changed: Value = serde_json::from_slice(&bytes).unwrap();
        changed["model"] = json!("different-request");
        assert!(snapshot
            .verify_request(&binding, &serde_json::to_vec(&changed).unwrap())
            .is_err());
        let mut with_private_mapping = encoded.clone();
        with_private_mapping["adapter"]["upstream_model"] = json!("private-backend");
        assert!(serde_json::from_value::<Snapshot>(with_private_mapping).is_err());
        assert!(serde_json::from_value::<attempts::AcceptanceSnapshot>(encoded).is_err());
        let private = json!({"adapter": f.adapter.snapshot(), "offer": purchase.terms().offer});
        let restored: Snapshot = serde_json::from_value(private.clone()).unwrap();
        assert!(matches!(restored, Snapshot::Legacy(_)));
        assert_eq!(serde_json::to_value(&restored).unwrap(), private);
        restored.verify_request(&binding, &bytes).unwrap();
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        peer.stop().await;
    }
}

#[tokio::test]
async fn legacy_signed_buyer_purchase_reopens_without_rewriting_commitment_and_can_finish() {
    use redb::{ReadableDatabase, ReadableTable, TableDefinition};
    const TABLE: TableDefinition<&str, &[u8]> =
        TableDefinition::new("proxy_negotiation_records_v1");
    let backend = backend(200, answer(), Duration::ZERO).await;
    let f = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let mut peer = Peer::start(ProxyRail::Tnk, &f, &chat(), false, None).await;
    let (signer, _, provider_key) = signer(&mut peer).await;
    let c = controller(&f, &peer, 1);
    let (q, p) = prepare(&peer, &f, &chat(), &session(&peer)).await;
    let saved = c.sign(p, q, signer, 1000).await.unwrap();
    let key = saved.key().clone();
    let offer = saved.offer();
    drop(c);

    // Recreate the pre-public-verifier on-disk schema with its original
    // commitment algorithm. Terms and signature stay exactly unchanged.
    let path = f._store.path().join("negotiation");
    let db = redb::Database::open(&path).unwrap();
    let tx = db.begin_write().unwrap();
    let original = {
        let mut table = tx.open_table(TABLE).unwrap();
        let mut r: Value =
            serde_json::from_slice(table.get(key.as_str()).unwrap().unwrap().value()).unwrap();
        r["purchase"]["snapshot"] =
            json!({"adapter":f.adapter.snapshot(), "offer":offer.terms.offer});
        let bytes = mayhem_proto::stable_json_bytes(&r["purchase"]).unwrap();
        let mut h = blake3::Hasher::new_derive_key("mayhem/proxy/purchase-intent/v1");
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(&bytes);
        r["commitment"] = json!(h.finalize().to_hex().to_string());
        let encoded = serde_json::to_vec(&r).unwrap();
        assert!(encoded.len() as u64 <= r["allocated_bytes"].as_u64().unwrap());
        table.insert(key.as_str(), encoded.as_slice()).unwrap();
        r
    };
    tx.commit().unwrap();
    drop(db);

    let c = controller(&f, &peer, 1);
    let restored = c.recover(key.clone()).await.unwrap().unwrap();
    assert!(matches!(restored.snapshot(), Snapshot::Legacy(_)));
    assert_eq!(restored.offer().buyer_sig, offer.buyer_sig);
    assert_eq!(restored.request(), chat());
    c.retain_provider_acceptance(accepted(offer, &provider_key))
        .await
        .unwrap();
    let b = recovery(&f, &peer);
    let restored = c.publish(key.clone(), &b, 1002).await.unwrap();
    assert!(restored.confirmed());
    assert_eq!(peer.command("status").await["publications"], 1);
    drop(c);
    let db = redb::Database::open(&path).unwrap();
    let tx = db.begin_read().unwrap();
    let table = tx.open_table(TABLE).unwrap();
    let now: Value =
        serde_json::from_slice(table.get(key.as_str()).unwrap().unwrap().value()).unwrap();
    assert_eq!(now["purchase"], original["purchase"]);
    assert_eq!(now["commitment"], original["commitment"]);
    assert_eq!(now["buyer_sig"], original["buyer_sig"]);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    peer.stop().await;
}
