use super::*;

// The directory suite separately traverses 100,000 real indexed rows. Here the
// actual retained resolver accumulator crosses that size without a total cap,
// candidate history, precision loss, or a minimum claim before scope completion.
fn fixture() -> (Session, Best) {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../mayhem-proxy/tests/fixtures/routing-profiles-v1.json"
    )))
    .unwrap();
    let f = &fixture["cases"][0];
    let p = &f["publication"];
    let endpoint = serde_json::from_value(p["offer"]["endpoint"].clone()).unwrap();
    let policy = proxy_request::Policy::new(
        Digest::new("f".repeat(64)).unwrap(),
        serde_json::from_value(f["request"]["proxy"]["settlement_policy_hash"].clone()).unwrap(),
        mayhem_proxy::financial::quote::Lifetimes {
            acceptance_epochs: 0,
            reservation_epochs: 10,
            receipt_grace_epochs: 2,
        },
        16 * 1024 * 1024,
    )
    .unwrap();
    let request = Arc::new(
        proxy_request::Request::parse(endpoint, f["request"].clone(), &policy)
            .unwrap()
            .unwrap(),
    );
    let controls = request.controls().clone();
    let published = PublishedOffer {
        id: p["id"].as_str().unwrap().into(),
        lane: "proxy",
        market: serde_json::from_value(p["market"].clone()).unwrap(),
        membership: serde_json::from_value(p["membership"].clone()).unwrap(),
        offer: serde_json::from_value(p["offer"].clone()).unwrap(),
        digest: p["digest"].as_str().unwrap().into(),
        active: true,
        catalog_eligible: true,
        catalog_observed_at_ms: Some(100),
        operator_verification: "unknown",
        family_label: None,
    };
    let best = Best {
        evidence: None,
        model: request.selector().model(),
        body: request.provider_value().clone(),
        request,
        maximum: Maximum {
            request_hash: Digest::new("e".repeat(64)).unwrap(),
            metering_policy_hash: Digest::new(&published.offer.metering_policy_hash).unwrap(),
            max_usage: BTreeMap::new(),
            max_spend_au: u128::MAX,
        },
        score: u128::MAX,
        maximum_retail_cost_micro: None,
        published,
    };
    let session = Session {
        id: "a".repeat(64),
        revision: 0,
        last: None,
        complete: false,
        started_at_ms: 100,
        expires_at_ms: 600100,
        models: vec![],
        model_allowlist: None,
        model_allowlist_digest: "b".repeat(64),
        request_content_digest: "c".repeat(64),
        retail_ranking: None,
        endpoint,
        body: Value::Null,
        controls,
        previous: None,
        previous_checked: true,
        snapshot: "fixture:1".into(),
        query_key: None,
        catalog: None,
        cursor: None,
        considered: 0,
        scanned: 0,
        index_reads: 0,
        exclusions: BTreeMap::new(),
        unknown: 0,
        best: None,
        page: VecDeque::new(),
        traversal_done: false,
        taxonomy: None,
        waiting: None,
        lease_until: None,
        pending_reason: None,
    };
    (session, best)
}

#[test]
fn retained_progress_crosses_one_hundred_thousand_and_never_claims_a_partial_minimum() {
    let (mut session, template) = fixture();
    for i in 0..100_001u128 {
        let score = 100_001 - i;
        session.consider(Checked::Ready(Best {
            evidence: None,
            model: template.model.clone(),
            body: template.body.clone(),
            request: template.request.clone(),
            maximum: template.maximum.clone(),
            published: template.published.clone(),
            score,
            maximum_retail_cost_micro: None,
        }));
        if i % 10_000 == 0 {
            let response = pending(&mut session, "scope_remaining");
            assert_eq!(response["status"], "pending");
            assert_eq!(response["ranking_claim"], "none");
            assert!(response["selection"].is_null());
            assert!(!session.complete);
            assert!(session.page.is_empty());
        }
    }
    assert_eq!(session.considered, 100_001);
    assert_eq!(session.best.as_ref().unwrap().score, 1);
    assert_eq!(
        session.best.as_ref().unwrap().maximum.max_spend_au,
        u128::MAX,
        "retail score never rewrites wholesale terms"
    );
    session.consider(Checked::Excluded("descriptor_unavailable", true));
    assert_eq!(session.unknown, 1);
    assert!(status(&session, "incomplete", true, None)["selection"].is_null());
    assert!(better(u128::MAX - 1, "z", u128::MAX, "a"));
    assert!(better(1, "a", 1, "b"));
    assert!(!better(1, "b", 1, "a"));
}
