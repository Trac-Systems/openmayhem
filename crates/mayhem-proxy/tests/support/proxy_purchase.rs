use super::*;
#[path = "buyer_negotiation.rs"]
mod negotiation;
use ed25519_dalek::{Signer, SigningKey};
use financial::{
    quote::{Lifetimes, PriceLimits, PurchaseRequest, Query, SessionBinding},
    recovery::{BuyerRecovery, Limits as RecoveryLimits, Store},
};

fn prices(p: &Peer) -> PriceLimits {
    let t = &p.template.terms;
    PriceLimits {
        rates: t.offer.rates.clone(),
        per_request_au: t.offer.per_request_au,
        min_session_au: t.offer.min_session_au,
        max_total_spend_au: t.max_total_spend_au,
    }
}
fn query(p: &Peer) -> Query {
    let t = &p.template.terms;
    Query {
        offer: t.offer.clone(),
        rail: t.rail,
        settlement_policy_hash: t.settlement_policy_hash.clone(),
        billing_id: t.billing_id.clone(),
    }
}
fn session(p: &Peer) -> SessionBinding {
    let t = &p.template.terms;
    SessionBinding {
        session_id: Digest::new(&t.session_id).unwrap(),
        reservation_id: Digest::new(&t.reservation_id).unwrap(),
        connection_digest: Digest::new(&t.connection_digest).unwrap(),
        capacity_lease: Digest::new(&t.capacity_lease).unwrap(),
    }
}
fn lifetimes() -> Lifetimes {
    Lifetimes {
        acceptance_epochs: 0,
        reservation_epochs: 19,
        receipt_grace_epochs: 2,
    }
}
fn signature(key: &[u8; 32], bytes: &[u8]) -> String {
    SigningKey::from_bytes(key)
        .sign(bytes)
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[tokio::test]
async fn purchase_uses_owned_request_and_explicit_budget_then_reserves_exact_cost_across_endpoints_rails(
) {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        let mut requests: Vec<_> = cases().into_iter().map(|(e, b, _)| (e, b, false)).collect();
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
            requests.push((endpoint, serde_json::to_vec(&body).unwrap(), true));
        }
        for (endpoint, bytes, stream) in requests {
            let backend = backend(200, answer(), Duration::ZERO).await;
            let f = Fixture::new(&backend.base, endpoint);
            let mut peer = Peer::start(rail, &f, &bytes, stream, None).await;
            let q = peer.buyer_client.quote(&query(&peer)).await.unwrap();
            let output = (endpoint != ProxyEndpoint::Decisions).then_some(37);
            let intent = PurchaseRequest::new(
                f.adapter.public_snapshot(),
                bytes.clone(),
                prices(&peer),
                output,
                lifetimes(),
            )
            .unwrap();
            let purchase = q.prepare_purchase(&intent, &session(&peer)).unwrap();
            q.recheck_purchase(&purchase).unwrap();
            let terms = purchase.terms();
            assert_eq!(
                terms.request_hash,
                mayhem_proto::endpoint_request_fingerprint(
                    &serde_json::from_slice::<Value>(&bytes).unwrap()
                )
            );
            assert_eq!(purchase.request(), bytes);
            if endpoint == ProxyEndpoint::Decisions {
                assert_eq!(
                    terms.max_usage,
                    std::collections::BTreeMap::from([("decision".into(), 1)])
                );
            } else {
                assert_eq!(terms.max_usage["output_token"], 37);
                assert!(terms.max_usage["input_token"] > 0 && terms.max_usage["input_token"] < 100);
            }
            assert_eq!(
                terms.max_spend_au,
                terms.offer.cost(&terms.max_usage).unwrap()
            );
            assert!(
                terms.max_spend_au < peer.template.terms.max_spend_au,
                "no fixture's arbitrary maximum hold"
            );
            assert_eq!(peer.command("status").await["publications"], 0);
            assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
            // Only this ephemeral fixture constructs both signatures; production
            // negotiation must persist each party's intent before exposing one.
            let keys = peer.command("ephemeral_test_wallet_seeds").await;
            let buyer_key: [u8; 32] = serde_json::from_value(keys["buyer"].clone()).unwrap();
            let provider_key: [u8; 32] = serde_json::from_value(keys["provider"].clone()).unwrap();
            let auth = ProxySpendAuthorization {
                terms: terms.clone(),
                buyer_sig: signature(&buyer_key, &terms.buyer_signing_bytes().unwrap()),
                provider_sig: signature(&provider_key, &terms.provider_signing_bytes().unwrap()),
            };
            let mut identity = peer.identity.clone();
            identity.controller_pubkey = Digest::new(&terms.buyer_pubkey).unwrap();
            let recovery = BuyerRecovery::new(
                Arc::new(
                    Store::open(
                        f._store.path().join("purchase"),
                        identity,
                        RecoveryLimits {
                            max_records: 8,
                            closed_retention_ms: 1000,
                        },
                    )
                    .unwrap(),
                ),
                peer.buyer_client.clone(),
                2,
            )
            .unwrap();
            let key = recovery
                .retain_reservation(auth.clone(), purchase.policy().clone(), 1000)
                .await
                .unwrap();
            let admitted = recovery.publish_reservation(key, 1001).await.unwrap();
            purchase
                .snapshot()
                .validate_for(&admitted.initial_binding().unwrap())
                .unwrap();
            assert_eq!(admitted.accepted().authorization, auth);
            let held = peer.buyer_client.quote(&query(&peer)).await.unwrap();
            assert!(held.recheck_purchase(&purchase).is_err());
            assert_eq!(held.funding().unwrap().reserved_au, 50 + terms.max_spend_au);
            assert!(
                held.prepare_purchase(&intent, &session(&peer)).is_err(),
                "an active hold must be recovered"
            );
            assert_eq!(
                backend.calls.load(Ordering::SeqCst),
                0,
                "no model call from quoting/reserving"
            );
            peer.stop().await;
        }
    }
}

#[tokio::test]
async fn purchase_rejects_incompatible_recipe_budget_lifetime_and_oversized_or_invalid_requests() {
    for (endpoint, bytes, _) in cases() {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let f = Fixture::new(&backend.base, endpoint);
        let peer = Peer::start(ProxyRail::Tnk, &f, &bytes, false, None).await;
        let q = peer.buyer_client.quote(&query(&peer)).await.unwrap();
        let output = (endpoint != ProxyEndpoint::Decisions).then_some(37);
        let intent = |adapter, body, cap, budget, time| {
            PurchaseRequest::new(adapter, body, cap, budget, time)
        };
        for budget in if endpoint == ProxyEndpoint::Decisions {
            vec![Some(1), Some(0)]
        } else {
            vec![None, Some(0), Some(u64::MAX)]
        } {
            assert!(intent(
                f.adapter.public_snapshot(),
                bytes.clone(),
                prices(&peer),
                budget,
                lifetimes()
            )
            .is_err());
        }
        for bad in [
            b"not json".to_vec(),
            b"{}".to_vec(),
            vec![b' '; f.adapter.limits().request_bytes + 1],
        ] {
            assert!(intent(
                f.adapter.public_snapshot(),
                bad,
                prices(&peer),
                output,
                lifetimes()
            )
            .is_err());
        }
        assert!(intent(
            f.adapter.public_snapshot(),
            bytes.clone(),
            prices(&peer),
            output,
            Lifetimes {
                acceptance_epochs: 2,
                reservation_epochs: 2,
                receipt_grace_epochs: 0
            }
        )
        .is_err());
        for fault in ["price", "budget", "recipe", "expiry", "funding"] {
            let mut adapter = f.adapter.public_snapshot();
            let mut cap = prices(&peer);
            let mut time = lifetimes();
            let mut allowance = output;
            match fault {
                "price" => cap.rates[0].per_unit_au = 0,
                "budget" => cap.max_total_spend_au = 0,
                "recipe" => adapter.recipe_hash = d(211),
                "expiry" => time.receipt_grace_epochs = mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER,
                "funding" if endpoint != ProxyEndpoint::Decisions => {
                    allowance = Some(1_000_000);
                    cap.max_total_spend_au = u128::MAX;
                }
                "funding" => continue,
                _ => unreachable!(),
            }
            let changed = intent(adapter, bytes.clone(), cap, allowance, time).unwrap();
            assert!(
                q.prepare_purchase(&changed, &session(&peer)).is_err(),
                "{fault} / {endpoint:?}"
            );
        }
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        peer.stop().await;
    }
}
