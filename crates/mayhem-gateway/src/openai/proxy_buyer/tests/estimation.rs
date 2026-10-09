use super::*;

async fn estimate(
    f: &Fixture,
    body: Value,
    token: &str,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let response = f
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/proxy/estimate")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(
                    serde_json::to_vec(&json!({"schema_version":1,
            "endpoint":f.harness.adapter.endpoint(),"request":body}))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap())
}
async fn assert_no_purchase(f: &mut Fixture) {
    assert_eq!(f.harness.backend_calls(), 0);
    assert!(
        f.harness
            .session_frame_tags()
            .await
            .iter()
            .all(|tag| matches!(tag.as_str(), "p.describe.open" | "p.describe")),
        "estimates cannot negotiate or execute"
    );
    let financial = f.harness.status().await;
    assert_eq!(financial["publications"], 0);
    assert_eq!(financial["submissions"], 0);
    assert_eq!(f.harness.capacity_status().route_occupied, 0);
    assert_eq!(f.harness.capacity_status().group_occupied, 0);
    assert!(f
        .harness
        .negotiation
        .pending(None, 16)
        .await
        .unwrap()
        .is_empty());
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
    assert_eq!(
        f.state.access_control.summary()["tokens"][0]["spent_total_au"],
        "0"
    );
}
fn validate_hash(value: &Value) {
    let mut content = value.clone();
    let expected = content
        .as_object_mut()
        .unwrap()
        .remove("estimate_hash")
        .unwrap();
    let bytes = mayhem_proto::stable_json_bytes(&content).unwrap();
    assert_eq!(
        expected,
        json!(digest("mayhem/proxy/maximum-estimate/v1", &[&bytes]))
    );
}

#[tokio::test]
async fn estimates_match_prepared_purchases_for_all_four_families_without_admission() {
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let mut f = Fixture::start_with(endpoint, ProxyRail::Fiat).await;
        let (status, headers, quote) = estimate(&f, f.body(), "owner-fixture-key").await;
        assert_eq!(status, StatusCode::OK, "{endpoint:?}: {quote}");
        assert_eq!(headers["cache-control"], "private, no-store");
        assert!(!headers.contains_key("x-mayhem-job-id"));
        assert_eq!(quote["availability"]["status"], "available");
        assert_eq!(quote["offer"], json!(f.harness.template.terms.offer));
        assert_eq!(
            quote["offer_digest"],
            f.harness.template.terms.offer.digest().unwrap()
        );
        assert_eq!(
            quote["endpoint_contract"],
            f.harness.adapter.contract_hash().as_str()
        );
        assert_eq!(
            quote["recipe_hash"],
            f.harness.adapter.recipe_hash().as_str()
        );
        assert!(
            quote["expires_at_ms"].as_u64().unwrap() > quote["observed_at_ms"].as_u64().unwrap()
        );
        validate_hash(&quote);
        // The separate canonical quote read builds the same unsent purchase.
        let prepared = f.harness.prepare().await;
        assert_eq!(quote["max_usage"], json!(prepared.terms().max_usage));
        assert_eq!(
            quote["max_spend_au"],
            prepared.terms().max_spend_au.to_string()
        );
        assert_no_purchase(&mut f).await;
        let (status, headers, result) = f
            .post("estimate-execution", f.body(), "owner-fixture-key", false)
            .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        let job = f
            .state
            .jobs
            .lock()
            .unwrap()
            .get(headers["x-mayhem-job-id"].to_str().unwrap(), now_secs())
            .unwrap()
            .unwrap();
        let terms = job.proxy.as_ref().unwrap().terms().unwrap();
        assert_eq!(quote["request_hash"], terms.request_hash);
        assert_eq!(quote["max_usage"], json!(terms.max_usage));
        assert_eq!(quote["max_spend_au"], terms.max_spend_au.to_string());
        assert_eq!(f.harness.backend_calls(), 1);
        f.stop().await;
    }
}

#[tokio::test]
async fn estimate_fetches_custom_contract_and_rejects_its_invalid_request() {
    let mut contract = mayhem_proto::endpoint_family_contract_template(
        mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
    )
    .unwrap();
    let original = mayhem_proto::endpoint_contract_canonical_fingerprint(&contract);
    contract
        .request_attribute_specs
        .get_mut("temperature")
        .unwrap()
        .maximum = Some(0.5);
    assert_ne!(
        original,
        mayhem_proto::endpoint_contract_canonical_fingerprint(&contract)
    );
    let mut f = Fixture::start_with_contract(
        ProxyEndpoint::Chat,
        ProxyRail::Fiat,
        None,
        "owner-fixture-key",
        Some(contract),
    )
    .await;
    let mut body = f.body();
    body["messages"][0]["content"] = json!("Unicode café 🙂");
    body["temperature"] = json!(0.2);
    let (status, _, quote) = estimate(&f, body.clone(), "owner-fixture-key").await;
    assert_eq!(status, StatusCode::OK, "{quote}");
    validate_hash(&quote);
    let mut provider_body = body.clone();
    provider_body.as_object_mut().unwrap().remove("proxy");
    assert_eq!(
        quote["request_content_digest"],
        retail_request_content_digest(&provider_body).unwrap()
    );
    assert_eq!(
        quote["request_hash"],
        mayhem_proto::endpoint_request_fingerprint(&provider_body)
    );
    if let Some(path) = std::env::var_os("MAYHEM_TEST_PROXY_ESTIMATE_FIXTURE") {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        file.write_all(&serde_json::to_vec_pretty(&json!({"schema_version":1,"test_only":true,
            "request":{"schema_version":1,"endpoint":ProxyEndpoint::Chat,"request":body},"response":quote})).unwrap()).unwrap();
        file.sync_all().unwrap();
    }
    body["temperature"] = json!(0.8);
    let (status, _, error) = estimate(&f, body, "owner-fixture-key").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "proxy_estimate_invalid");
    assert_no_purchase(&mut f).await;
    f.stop().await;
}

#[tokio::test]
async fn estimate_requires_auth_explicit_policy_prices_and_enabled_buyer() {
    let mut f = Fixture::start().await;
    assert_eq!(
        estimate(&f, f.body(), "wrong-key").await.0,
        StatusCode::UNAUTHORIZED
    );
    let mut forbidden = f.body();
    forbidden["model"] = json!("proxy/offer/foreign");
    assert_eq!(
        estimate(&f, forbidden, "owner-fixture-key").await.0,
        StatusCode::FORBIDDEN
    );
    let mut oversized = f.body();
    oversized["messages"][0]["content"] = json!("x".repeat(600 * 1024));
    assert_eq!(
        estimate(&f, oversized, "owner-fixture-key").await.0,
        StatusCode::BAD_REQUEST
    );
    let mut body = f.body();
    body["proxy"]["settlement_policy_hash"] = json!("f".repeat(64));
    let (status, _, error) = estimate(&f, body, "owner-fixture-key").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"]["code"], "proxy_settlement_policy_mismatch");
    let mut body = f.body();
    body["proxy"]["prices"]["max_total_spend_au"] = json!("1");
    assert_eq!(
        estimate(&f, body, "owner-fixture-key").await.0,
        StatusCode::BAD_REQUEST
    );
    let mut body = f.body();
    body["proxy"]["output_units"] = Value::Null;
    assert_eq!(
        estimate(&f, body, "owner-fixture-key").await.0,
        StatusCode::BAD_REQUEST
    );
    // Reading an estimate does not require unspent account budget.
    f.tokens.tokens[0].budget_au = Some(0);
    std::fs::write(
        f.directory.path().join("tokens.json"),
        serde_json::to_vec(&f.tokens).unwrap(),
    )
    .unwrap();
    let (status, _, quote) = estimate(&f, f.body(), "owner-fixture-key").await;
    assert_eq!(status, StatusCode::OK, "{quote}");
    let mut disabled = f.state.clone();
    disabled.proxy_buyer = None;
    f.router = openai_router(disabled);
    let (status, _, error) = estimate(&f, f.body(), "owner-fixture-key").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["error"]["code"], "proxy_buyer_disabled");
    assert_no_purchase(&mut f).await;
    f.stop().await;
}

#[tokio::test]
async fn required_profile_evidence_is_unavailable_without_admission_for_all_four_families() {
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let mut f = Fixture::start_with(endpoint, ProxyRail::Fiat).await;
        let mut body = f.body();
        let controls = body["proxy"].clone();
        body["proxy"]["profile"] = json!({
            "schema_version": 1, "lane": "proxy", "endpoint": endpoint,
            "target": {"kind": "exact_offer", "offer_id": model(&f.harness).strip_prefix("proxy/offer/").unwrap()},
            "providers": {"allow": null, "deny": [], "require_verified_operator": true},
            "allowed_rails": [controls["rail"]], "prices": controls["prices"],
            "max_retail_cost_micro": "1000",
            "settlement_policies": [{"rail": controls["rail"], "settlement_policy_hash": controls["settlement_policy_hash"]}],
            "constraints": {"minimum_context": null, "minimum_tokens_per_second": null,
                "output_units": controls["output_units"], "capabilities": [], "request_controls": [], "data_handling": []},
            "ranking": "lowest_estimated_cost", "continuity": "retain_compatible"
        });
        let (status, headers, error) = estimate(&f, body.clone(), "owner-fixture-key").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{endpoint:?}: {error}");
        assert_eq!(error["error"]["code"], "proxy_profile_evidence_unavailable");
        assert_eq!(error["error"]["category"], "proxy_estimate");
        assert_eq!(error["error"]["retryable"], true);
        assert_eq!(headers["cache-control"], "private, no-store");
        assert!(!headers.contains_key("x-mayhem-job-id"));
        assert!(f.harness.session_frame_tags().await.is_empty());
        assert_no_purchase(&mut f).await;

        // Invalid constraints stay a client error even with unresolved evidence.
        let mut invalid = body.clone();
        invalid["proxy"]["profile"]["prices"]["max_total_spend_au"] = json!("0");
        let (status, _, error) = estimate(&f, invalid, "owner-fixture-key").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(error["error"]["code"], "proxy_estimate_invalid");
        assert_eq!(error["error"]["retryable"], false);

        // The same otherwise valid profile can be estimated without this requirement.
        body["proxy"]["profile"]["providers"]["require_verified_operator"] = json!(false);
        let (status, _, quote) = estimate(&f, body, "owner-fixture-key").await;
        assert_eq!(status, StatusCode::OK, "{endpoint:?}: {quote}");
        assert_no_purchase(&mut f).await;
        f.stop().await;
    }
}

#[tokio::test]
async fn concurrent_estimates_are_bounded_and_never_purchase() {
    let mut f = Fixture::start().await;
    let responses = futures_util::future::join_all(
        (0..16).map(|_| estimate(&f, f.body(), "owner-fixture-key")),
    )
    .await;
    let successes = responses.iter().filter(|r| r.0 == StatusCode::OK).count();
    assert!(
        successes > 0 && successes <= mayhem_proxy::descriptor::READS,
        "{responses:?}"
    );
    for (status, _, response) in responses {
        if status != StatusCode::OK {
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{response}");
            assert_eq!(response["error"]["code"], "proxy_estimate_busy");
        }
    }
    assert_no_purchase(&mut f).await;
    f.stop().await;
}

#[tokio::test]
async fn unavailable_capacity_is_reported_without_inference_or_invented_lease() {
    let mut f = Fixture::start().await;
    f.control.control.select_markets(Vec::new()).unwrap();
    let (status, _, value) = estimate(&f, f.body(), "owner-fixture-key").await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["availability"]["status"], "heartbeat_missing");
    assert!(value["availability"]["expires_at_ms"].is_null());
    assert_no_purchase(&mut f).await;
    f.stop().await;
}

#[tokio::test]
async fn descriptors_reject_substituted_contract_recipe_and_unavailable_peer() {
    let mut f = Fixture::start().await;
    let terms = &f.harness.template.terms;
    for (contract, recipe) in [
        (support::digest(91), f.harness.adapter.recipe_hash().clone()),
        (
            f.harness.adapter.contract_hash().clone(),
            support::digest(92),
        ),
    ] {
        assert!(f
            .harness
            .buyer
            .describe(
                terms.offer.clone(),
                terms.rail,
                Digest::new(terms.settlement_policy_hash.clone()).unwrap(),
                &contract,
                &recipe
            )
            .await
            .is_err());
    }
    f.harness.stop_descriptors();
    let before = std::time::Instant::now();
    let (status, _, value) = estimate(&f, f.body(), "owner-fixture-key").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{value}");
    assert_eq!(value["error"]["code"], "proxy_estimate_unavailable");
    assert!(before.elapsed() < Duration::from_secs(7));
    assert_no_purchase(&mut f).await;
    f.stop().await;
}
