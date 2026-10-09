use super::*;
use mayhem_proxy::financial::quote::{PriceLimits, Query};

fn query(f: &Fixture) -> Query {
    Query {
        offer: f.auth.terms.offer.clone(),
        rail: f.auth.terms.rail,
        settlement_policy_hash: f.auth.terms.settlement_policy_hash.clone(),
        billing_id: f.auth.terms.billing_id.clone(),
    }
}
fn limits(f: &Fixture) -> PriceLimits {
    let t = &f.auth.terms;
    PriceLimits {
        rates: t.offer.rates.clone(),
        per_request_au: t.offer.per_request_au,
        min_session_au: t.offer.min_session_au,
        max_total_spend_au: t.max_total_spend_au,
    }
}
#[tokio::test]
async fn quote_real_signed_rpc_covers_families_rails_and_never_reserves_or_dispatches() {
    for family in ["llm", "decisions"] {
        for rail in ["fiat", "tnk", "tap"] {
            let mut f = Fixture::new_with_mode(rail, family, "unreserved").await;
            let q = f.buyer_client.quote(&query(&f)).await.unwrap();
            assert_eq!(q.funding().unwrap().reserved_au, 50);
            assert_eq!(q.epoch().unwrap(), 101);
            assert!(q.billing().unwrap().is_none());
            assert_eq!(q.policy().unwrap(), &f.policy);
            assert_eq!(q.payment_binding().unwrap().0, f.auth.terms.payout_revision);
            q.check_terms(&f.auth.terms, &limits(&f)).unwrap();
            let changed = f.auth.terms.clone();
            for field in [
                "rate", "fixed", "total", "payment", "buyer", "rail", "epoch", "prior", "payout",
            ] {
                let mut t = changed.clone();
                let mut cap = limits(&f);
                match field {
                    "rate" => cap.rates[0].per_unit_au = 0,
                    "fixed" => t.offer.per_request_au += 1,
                    "total" => cap.max_total_spend_au += 1,
                    "payment" => t.payment_terms_hash = "0".repeat(64),
                    "buyer" => t.buyer_pubkey = "0".repeat(64),
                    "rail" => {
                        t.rail = if t.rail == mayhem_proto::proxy::ProxyRail::Fiat {
                            mayhem_proto::proxy::ProxyRail::Tap
                        } else {
                            mayhem_proto::proxy::ProxyRail::Fiat
                        }
                    }
                    "epoch" => t.billing_epoch += 1,
                    "prior" => t.prior_reserved_au = 1,
                    "payout" => t.payout_revision = "0".repeat(64),
                    _ => unreachable!(),
                }
                assert!(q.check_terms(&t, &cap).is_err(), "{field}");
            }
            assert_eq!(f.request("status").await["publications"], 0);
            f.stop().await;
        }
    }
}

#[tokio::test]
async fn quote_observation_rejects_stale_offers_wrong_nonce_network_and_foreign_billing() {
    let mut f = Fixture::new("tnk", "llm").await;
    let q = f.buyer_client.quote(&query(&f)).await.unwrap();
    assert!(q
        .billing()
        .unwrap()
        .unwrap()
        .active_reservation_id
        .is_some());
    assert!(
        q.check_terms(&f.auth.terms, &limits(&f)).is_err(),
        "already reserved; recover original purchase"
    );
    assert!(
        f.client.quote(&query(&f)).await.is_err(),
        "provider cannot inspect buyer's billing"
    );
    let mut old = query(&f);
    old.offer.revision += 1;
    assert!(f.buyer_client.quote(&old).await.is_err());
    for fault in ["nonce", "network", "unknown"] {
        f.command(fault).await;
        assert!(f.buyer_client.quote(&query(&f)).await.is_err(), "{fault}");
        f.command("reset").await;
    }
    f.command("close").await;
    let closed = f.buyer_client.quote(&query(&f)).await.unwrap();
    assert!(closed
        .billing()
        .unwrap()
        .unwrap()
        .active_reservation_id
        .is_none());
    assert!(closed.proof().follows(q.proof()));
    f.stop().await;
}

#[test]
fn quote_price_caps_compare_fractions_exactly_without_overflow_or_missing_units() {
    let source: Value = serde_json::from_str(include_str!(
        "../../../mayhem-proto/tests/fixtures/proxy-finance-v1.json"
    ))
    .unwrap();
    let mut offer: mayhem_proto::proxy::ProxyOffer =
        serde_json::from_value(source["cases"][0]["terms"]["offer"].clone()).unwrap();
    let mut caps = PriceLimits {
        rates: offer.rates.clone(),
        per_request_au: offer.per_request_au,
        min_session_au: offer.min_session_au,
        max_total_spend_au: 1,
    };
    offer.rates[0].per_unit_au = 3;
    offer.rates[0].granularity = 7;
    caps.rates[0].per_unit_au = 6;
    caps.rates[0].granularity = 14;
    caps.permits(&offer).unwrap();
    caps.rates[0].per_unit_au = 5;
    assert!(caps.permits(&offer).is_err());
    offer.rates[0].per_unit_au = u128::MAX;
    offer.rates[0].granularity = mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER;
    caps.rates[0] = offer.rates[0].clone();
    caps.permits(&offer).unwrap();
    caps.rates[0].per_unit_au -= 1;
    assert!(caps.permits(&offer).is_err());
    caps.rates[0] = offer.rates[0].clone();
    caps.rates.pop();
    assert!(caps.permits(&offer).is_err());
}
