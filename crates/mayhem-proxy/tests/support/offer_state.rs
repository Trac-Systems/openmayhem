use super::*;
use mayhem_proxy::financial::offer::Query;

fn query(f: &Fixture) -> Query {
    Query {
        offer: f.auth.terms.offer.clone(),
        rail: f.auth.terms.rail,
        settlement_policy_hash: f.auth.terms.settlement_policy_hash.clone(),
    }
}

#[tokio::test]
async fn availability_follows_rate_revisions_without_repricing_accepted_work() {
    for family in ["llm", "decisions"] {
        for rail in ["fiat", "tnk", "tap"] {
            let mut f = Fixture::new(rail, family).await;
            let query = query(&f);
            let before = f.client.current_rate_state(&query).await.unwrap();
            assert_eq!(before.offer().unwrap(), &query.offer);
            let mut changed = query.offer.clone();
            changed.revision += 1;
            for rate in &mut changed.rates { rate.per_unit_au += 10; }
            changed.per_request_au += 2;
            assert_eq!(f.request(&serde_json::json!({"set_offer":changed}).to_string()).await["done"], "set_offer");
            assert!(f.client.offer_state(&query).await.is_err());
            let current = f.client.current_rate_state(&query).await.unwrap();
            assert_eq!(current.offer().unwrap(), &changed);
            assert!(current.check_terms(&f.auth.terms, &f.policy).is_err(), "new offer cannot reprice original terms");
            assert!(f.buyer_client.current_rate_state(&query).await.is_err());
            let mut newer = query.clone();
            newer.offer = changed;
            assert_eq!(f.client.offer_state(&newer).await.unwrap().offer().unwrap(), &newer.offer);
            newer.offer.revision += 1;
            assert!(f.client.current_rate_state(&newer).await.is_err(), "cannot move a newer observation backwards");
            f.command("final").await;
            let state = f.request("state").await;
            let usage = query.offer.rates.iter().map(|r| (r.unit.clone(), 4)).collect();
            let original_cost = query.offer.cost(&usage).unwrap();
            assert_eq!(state["summary"]["reserved_au"], (50 + original_cost).to_string(),
                "only original payable amount remains reserved for epoch settlement");
            assert_eq!(state["billing"]["spent_au"], original_cost.to_string());
            assert!(state["billing"]["active_reservation_id"].is_null());
            f.stop().await;
        }
    }
}

#[tokio::test]
async fn provider_offer_observation_checks_current_terms_and_operator_policy_across_families_rails()
{
    for family in ["llm", "decisions"] {
        for rail in ["fiat", "tnk", "tap"] {
            let mut f = Fixture::new_with_mode(rail, family, "unreserved").await;
            let q = f.client.offer_state(&query(&f)).await.unwrap();
            assert_eq!(q.epoch().unwrap(), 101);
            assert_eq!(q.policy().unwrap(), &f.policy);
            assert_eq!(
                q.membership().unwrap().provider_pubkey,
                f.auth.terms.offer.provider_pubkey
            );
            q.check_terms(&f.auth.terms, &f.policy).unwrap();
            let mut not_approved = f.policy.clone();
            not_approved.allow_checkpoints = !not_approved.allow_checkpoints;
            assert!(
                q.check_terms(&f.auth.terms, &not_approved).is_err(),
                "globally enabled is not operator approval"
            );
            for field in [
                "provider",
                "offer",
                "payment",
                "payout",
                "rail",
                "epoch",
                "context",
                "recipe",
                "connection",
                "network",
                "rules",
            ] {
                let mut terms = f.auth.terms.clone();
                match field {
                    "provider" => terms.offer.provider_pubkey = "0".repeat(64),
                    "offer" => terms.offer.revision += 1,
                    "payment" => terms.payment_terms_hash = "0".repeat(64),
                    "payout" => terms.payout_revision = "0".repeat(64),
                    "rail" => {
                        terms.rail = if rail == "tnk" {
                            mayhem_proto::proxy::ProxyRail::Fiat
                        } else {
                            mayhem_proto::proxy::ProxyRail::Tnk
                        }
                    }
                    "epoch" => terms.billing_epoch += 1,
                    "context" => terms.served_context += 1,
                    "recipe" => terms.recipe_hash = "0".repeat(64),
                    "connection" => terms.connection_revision += 1,
                    "network" => terms.network_id = "other-network".into(),
                    "rules" => terms.rules_ver += 1,
                    _ => unreachable!(),
                }
                assert!(
                    q.check_terms(&terms, &f.policy).is_err(),
                    "{family}/{rail}/{field}"
                );
            }
            assert_eq!(f.request("status").await["publications"], 0);
            f.stop().await;
        }
    }
}

#[tokio::test]
async fn provider_offer_rpc_rejects_foreign_requester_stale_offer_and_unbound_replies() {
    let mut f = Fixture::new("tnk", "llm").await;
    let query = query(&f);
    let q = f.client.offer_state(&query).await.unwrap();
    q.check_terms(&f.auth.terms, &f.policy).unwrap();
    assert!(f.buyer_client.offer_state(&query).await.is_err());
    let mut stale = query.clone();
    stale.offer.revision += 1;
    assert!(f.client.offer_state(&stale).await.is_err());
    for fault in ["nonce", "network", "unknown"] {
        f.command(fault).await;
        assert!(f.client.offer_state(&query).await.is_err(), "{fault}");
        f.command("reset").await;
    }
    f.command("close").await;
    let current = f.client.offer_state(&query).await.unwrap();
    assert!(current.proof().follows(q.proof()));
    assert_eq!(current.epoch().unwrap(), q.epoch().unwrap());
    current.check_terms(&f.auth.terms, &f.policy).unwrap();
    assert_eq!(f.request("status").await["publications"], 0);
    f.stop().await;
}

#[tokio::test]
async fn provider_offer_cannot_renew_expired_observation_or_accept_in_a_later_epoch() {
    let mut f = Fixture::new_with_mode("fiat", "llm", "unreserved").await;
    let q = f.client.offer_state(&query(&f)).await.unwrap();
    q.check_terms(&f.auth.terms, &f.policy).unwrap();
    f.request(r#"{"epoch":101}"#).await;
    let later = f.client.offer_state(&query(&f)).await.unwrap();
    assert_eq!(later.epoch().unwrap(), 102);
    assert!(later.check_terms(&f.auth.terms, &f.policy).is_err());
    tokio::time::sleep(Duration::from_secs(16)).await;
    assert!(q.check_terms(&f.auth.terms, &f.policy).is_err());
    assert!(q.epoch().is_err());
    assert!(q.policy().is_err());
    assert!(q.membership().is_err());
    f.stop().await;
}
