use super::*;
use mayhem_proxy::setup::{Flow, FlowAction, RateChoice};
use publication::{owned, signer, Peer};

fn choices(offers: &[ProxyOffer]) -> Vec<RateChoice> {
    offers
        .iter()
        .map(|o| {
            let mut c = RateChoice::from_offer(o).unwrap();
            c.per_request_au += 10;
            c.rates[0].per_unit_au += 2;
            c
        })
        .collect()
}
async fn published(f: &mut Fixture, seed: u8) -> (Peer, mayhem_proxy::setup::Review) {
    let peer = Peer::start(f).await;
    f.store().create(f.input.clone()).unwrap();
    f.store().check(1).unwrap();
    let plan = f.store().publication_plan(2, false).unwrap();
    let permit = peer.permit(&plan, "fiat").await;
    let review = f
        .store()
        .publish(
            2,
            &peer.rpc(),
            10000,
            plan.authorize(&signer(seed), Some(permit)).unwrap(),
        )
        .await
        .unwrap();
    peer.call(
        "rate-readiness",
        json!({"provider":f.input.provider_pubkey,"policy":f.input.settlement_policy}),
    )
    .await;
    (peer, review)
}
#[tokio::test]
async fn guided_rates_all_endpoints_exact_slots_derived_revisions_restart_and_no_second_fee() {
    for (endpoint, seed) in [
        (ProxyEndpoint::Chat, 141),
        (ProxyEndpoint::Completions, 142),
        (ProxyEndpoint::Responses, 143),
        (ProxyEndpoint::Decisions, 144),
    ] {
        let mut f = owned(endpoint, seed);
        let mut second = f.input.offers[0].clone();
        second.ctx_bracket = "ctx2k".into();
        second.revision = 7;
        f.input.offers.push(second);
        let backend = if endpoint == ProxyEndpoint::Chat {
            Some(probes::backend(&mut f, 200,
            serde_json::to_vec(&json!({"id":"fixture","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}]})).unwrap(), false, std::time::Duration::ZERO))
        } else {
            None
        };
        let (peer, mut original) = published(&mut f, seed).await;
        if backend.is_some() {
            original = f.store().probe(original.revision, probes::plan(&f, json!({"model":"public","messages":[{"role":"user","content":"synthetic"}],"max_tokens":16}))).await.unwrap();
            assert_eq!(original.probe_status, "protocol_validated");
        }
        let before = std::fs::read(f.store.join("draft.json")).unwrap();
        let mut cfg = flow::config(&f);
        cfg.peer_rpc = Some(peer.rpc());
        cfg.timeout_ms = 10000;
        let flow = Flow::open(cfg.clone()).unwrap();
        let planned = flow
            .execute(
                FlowAction::RatePlan {
                    expected_revision: original.revision,
                    choices: choices(&original.offers),
                },
                None,
            )
            .await
            .unwrap();
        let report = planned.view.rates.as_ref().unwrap();
        assert_eq!(report.state, "needs_confirmation");
        assert_eq!(
            std::fs::read(f.store.join("draft.json")).unwrap(),
            before,
            "review does not alter retained authority"
        );
        assert_eq!(report.plan.publication.operations[0].sequence, 4);
        assert_eq!(report.plan.publication.operations[1].sequence, 5);
        for (operation, revision) in report.plan.publication.operations.iter().zip([2, 8]) {
            let ProxyAction::SetOffer { offer } = &operation.action else {
                panic!("offers only")
            };
            assert_eq!(offer.revision, revision);
            assert_eq!(offer.accepted_rails, original.offers[0].accepted_rails);
        }
        let restarted = Flow::open(cfg).unwrap();
        assert_eq!(
            restarted.view().unwrap().rates.unwrap().plan.plan_digest,
            report.plan.plan_digest
        );
        // Lost ACK after a real append: retained publisher owns the original operation.
        peer.call("mode", json!({"hide_after_submit":true})).await;
        let pending = restarted
            .execute(
                FlowAction::PublishRates {
                    expected_revision: original.revision,
                    plan_digest: report.plan.plan_digest.clone(),
                },
                Some(&signer(seed)),
            )
            .await
            .unwrap();
        let pending_revision = pending.view.review.as_ref().unwrap().revision;
        assert_eq!(
            pending.view.rates.as_ref().unwrap().state,
            "recover_original_publication"
        );
        assert!(f
            .store()
            .plan_rates(
                pending_revision,
                choices(&original.offers),
                &peer.rpc(),
                10000
            )
            .await
            .is_err());
        peer.call("mode", json!({})).await;
        let final_review = f
            .store()
            .publish_rates(
                pending_revision,
                &report.plan.plan_digest,
                &peer.rpc(),
                10000,
                &signer(seed),
            )
            .await
            .unwrap();
        assert_eq!(
            final_review.publication_status,
            "canonical_operations_confirmed"
        );
        assert_eq!(
            f.store().rates().unwrap().unwrap().state,
            "canonical_rates_confirmed"
        );
        assert_eq!(final_review.market, original.market);
        assert_eq!(final_review.membership, original.membership);
        assert_eq!(final_review.settlement_policy, original.settlement_policy);
        assert_eq!(final_review.probe_status, original.probe_status);
        let original_record: Value = serde_json::from_slice(&before).unwrap();
        let final_record: Value =
            serde_json::from_slice(&std::fs::read(f.store.join("draft.json")).unwrap()).unwrap();
        assert_eq!(original_record["probe"], final_record["probe"]);
        assert_eq!(original_record["probe_scope"], final_record["probe_scope"]);
        if let Some(backend) = &backend {
            assert_eq!(backend.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
        assert!(final_review.admission_handoff.is_none());
        let status = peer
            .call("status", json!({"provider":f.input.provider_pubkey}))
            .await;
        assert_eq!(status["permits"], 1);
        assert_eq!(status["appends"], 5);
        assert_eq!(status["model_calls"], 0);
        assert_eq!(
            status["native_balance"],
            json!({"fiat":"10","tnk":"20","tap":"30"})
        );
        assert_eq!(
            status["native_payout"],
            json!({"status":"prepared","native":true})
        );
        // Accepted publication replay precedes fresh canonical reads, including outage.
        peer.call("mode", json!({"hidden":true})).await;
        let replay = f
            .store()
            .publish_rates(
                final_review.revision,
                &report.plan.plan_digest,
                &peer.rpc(),
                10000,
                &signer(seed),
            )
            .await
            .unwrap();
        assert_eq!(replay.revision, final_review.revision);
        assert_eq!(peer.call("status", json!({})).await["appends"], 5);
        if let Some(path) = std::env::var_os("MAYHEM_RATES_FIXTURE") {
            if endpoint == ProxyEndpoint::Chat {
                std::fs::write(path, serde_json::to_vec_pretty(&json!({"test_only":true,"planned":planned,"pending":pending,"completed":restarted.view().unwrap()})).unwrap()).unwrap();
            }
        }
        f.no_network_or_secret();
        peer.close().await;
    }
}
#[tokio::test]
async fn guided_rates_stale_canonical_review_invalid_choices_and_local_conflicts_fail_closed() {
    let mut f = owned(ProxyEndpoint::Chat, 145);
    let (peer, original) = published(&mut f, 145).await;
    let before = std::fs::read(f.store.join("draft.json")).unwrap();
    for (index, mut bad) in [choices(&original.offers), choices(&original.offers)]
        .into_iter()
        .enumerate()
    {
        if index == 0 {
            bad[0].rates.pop();
        } else {
            bad[0].slot_id = d(255);
        }
        assert!(f
            .store()
            .plan_rates(original.revision, bad, &peer.rpc(), 10000)
            .await
            .is_err());
    }
    assert!(f
        .store()
        .plan_rates(
            original.revision,
            original
                .offers
                .iter()
                .map(|o| RateChoice::from_offer(o).unwrap())
                .collect(),
            &peer.rpc(),
            10000
        )
        .await
        .is_err());
    let plan = f
        .store()
        .plan_rates(
            original.revision,
            choices(&original.offers),
            &peer.rpc(),
            10000,
        )
        .await
        .unwrap()
        .plan;
    assert!(matches!(
        f.store()
            .publish_rates(
                original.revision - 1,
                &plan.plan_digest,
                &peer.rpc(),
                10000,
                &signer(145)
            )
            .await,
        Err(Error::Conflict)
    ));
    assert!(f
        .store()
        .publish_rates(original.revision, &d(255), &peer.rpc(), 10000, &signer(145))
        .await
        .is_err());
    assert!(f
        .store()
        .publish_rates(
            original.revision,
            &plan.plan_digest,
            &peer.rpc(),
            10000,
            &signer(146)
        )
        .await
        .is_err());
    assert_eq!(std::fs::read(f.store.join("draft.json")).unwrap(), before);
    // Another owned draft publishes concurrently, changing the same canonical slot.
    let other = f.dir.path().join("other-draft");
    std::fs::create_dir(&other).unwrap();
    std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Store::open(other).unwrap();
    let mut changed = f.input.clone();
    changed.sequence = 3;
    changed.offers[0].revision = 9;
    changed.offers[0].per_request_au = 999;
    store.create(changed).unwrap();
    store.check(1).unwrap();
    store
        .publish(
            2,
            &peer.rpc(),
            10000,
            store
                .publication_plan(2, true)
                .unwrap()
                .authorize(&signer(145), None)
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(f
        .store()
        .publish_rates(
            original.revision,
            &plan.plan_digest,
            &peer.rpc(),
            10000,
            &signer(145)
        )
        .await
        .is_err());
    assert_eq!(std::fs::read(f.store.join("draft.json")).unwrap(), before);
    let refreshed = f
        .store()
        .plan_rates(
            original.revision,
            choices(&original.offers),
            &peer.rpc(),
            10000,
        )
        .await
        .unwrap();
    assert_eq!(refreshed.plan.publication.operations[0].sequence, 4);
    let ProxyAction::SetOffer { offer } = &refreshed.plan.publication.operations[0].action else {
        panic!()
    };
    assert_eq!(offer.revision, 10);
    f.connection["revision"] = json!(2);
    private(
        &f.input.connection_file,
        &serde_json::to_vec(&f.connection).unwrap(),
    );
    assert!(f
        .store()
        .publish_rates(
            original.revision,
            &refreshed.plan.plan_digest,
            &peer.rpc(),
            10000,
            &signer(145)
        )
        .await
        .is_err());
    assert_eq!(std::fs::read(f.store.join("draft.json")).unwrap(), before);
    peer.close().await;
}
#[test]
fn guided_rate_action_cannot_supply_execution_rails_revision_or_sequence() {
    let f = Fixture::new(ProxyEndpoint::Chat);
    let body =
        json!({"action":"rate_plan","expected_revision":1,"choices":choices(&f.input.offers)});
    for field in [
        "sequence",
        "revision",
        "accepted_rails",
        "membership",
        "connection_file",
    ] {
        let mut bad = body.clone();
        bad["choices"][0][field] = json!(1);
        assert!(
            serde_json::from_value::<FlowAction>(bad).is_err(),
            "{field}"
        );
    }
}
