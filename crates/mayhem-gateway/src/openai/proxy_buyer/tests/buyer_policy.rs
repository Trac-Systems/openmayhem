use super::*;

const PATH: &str = "/v1/proxy/buyer-policy";

#[tokio::test]
async fn public_policy_is_authenticated_exact_current_and_read_only() {
    let mut f = Fixture::start().await;
    let native = f.get("/v1/models", "owner-fixture-key").await;
    for token in [None, Some("invalid-fixture-key")] {
        let mut request = Request::builder().uri(PATH);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = f
            .router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()["cache-control"], "private, no-store");
    }
    // A read discloses no spending authority and does not require spare budget.
    f.tokens.tokens[0].budget_au = Some(0);
    std::fs::write(
        f.directory.path().join("tokens.json"),
        serde_json::to_vec(&f.tokens).unwrap(),
    )
    .unwrap();
    let (status, value) = f.get(PATH, "owner-fixture-key").await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(
        value,
        json!({"schema_version":1,
        "settlement_policy_hash":f.harness.policy.digest().unwrap(),
        "settlement_policy":f.harness.policy})
    );
    assert_eq!(value.as_object().unwrap().len(), 3);
    let public: ProxySettlementPolicy =
        serde_json::from_value(value["settlement_policy"].clone()).unwrap();
    assert_eq!(public.digest().unwrap(), value["settlement_policy_hash"]);

    let mut changed = f.harness.policy.clone();
    changed.allow_checkpoints = !changed.allow_checkpoints;
    let policy = proxy_request::Policy::new(
        support::digest(99),
        Digest::new(changed.digest().unwrap()).unwrap(),
        f.runtime.policy.lifetimes(),
        512 * 1024,
    )
    .unwrap();
    let runtime =
        Arc::new(Runtime::new(f.harness.buyer.clone(), policy, changed.clone(), 2).unwrap());
    f.router = openai_router(f.state.clone().with_proxy_buyer(runtime).unwrap());
    let (status, current) = f.get(PATH, "owner-fixture-key").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(current["settlement_policy_hash"], changed.digest().unwrap());
    assert_eq!(
        current["settlement_policy"],
        serde_json::to_value(changed).unwrap()
    );
    assert_ne!(
        current["settlement_policy_hash"],
        value["settlement_policy_hash"]
    );

    let mut disabled = f.state.clone();
    disabled.proxy_buyer = None;
    f.router = openai_router(disabled);
    let (status, error) = f.get(PATH, "owner-fixture-key").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["error"]["code"], "proxy_buyer_disabled");
    assert_eq!(error["error"]["category"], "proxy_discovery");
    assert_eq!(error["error"]["retryable"], true);
    f.tokens.tokens[0].budget_au = Some(1_000_000);
    std::fs::write(
        f.directory.path().join("tokens.json"),
        serde_json::to_vec(&f.tokens).unwrap(),
    )
    .unwrap();
    let after = f.get("/v1/models", "owner-fixture-key").await;
    assert_eq!(after.0, native.0);
    let identities = |response: &Value| {
        response["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| {
                (
                    model["id"].clone(),
                    model["owned_by"].clone(),
                    model["mayhem"]["source"].clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(identities(&after.1), identities(&native.1));
    f.tokens.tokens[0].revoked_at = Some(now_secs());
    std::fs::write(
        f.directory.path().join("tokens.json"),
        serde_json::to_vec(&f.tokens).unwrap(),
    )
    .unwrap();
    assert_eq!(
        f.get(PATH, "owner-fixture-key").await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(f.harness.backend_calls(), 0);
    assert_eq!(f.harness.status().await["publications"], 0);
    assert!(f
        .state
        .access_control
        .pending_key_budgets(None, 64)
        .unwrap()
        .is_empty());
    assert!(f
        .state
        .jobs
        .lock()
        .unwrap()
        .pending_proxy(None, 64)
        .unwrap()
        .is_empty());
    f.stop().await;
}

#[tokio::test]
async fn public_policy_requires_key_even_when_native_gateway_allows_anonymous_reads() {
    let router = openai_router(GatewayState::fixture());
    for (path, status) in [
        ("/v1/models", StatusCode::OK),
        (PATH, StatusCode::UNAUTHORIZED),
    ] {
        let response = router
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
}
