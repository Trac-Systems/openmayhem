use super::*;

fn path(f: &Fixture) -> String {
    format!(
        "/v1/proxy/offers/{}/contract",
        model(&f.harness).strip_prefix("proxy/offer/").unwrap()
    )
}
async fn get(
    f: &Fixture,
    query: &str,
    token: Option<&str>,
    body: Body,
) -> (StatusCode, HeaderMap, Value) {
    let mut request = Request::builder().uri(format!("{}{}", path(f), query));
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = f
        .router
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap())
}
async fn unchanged(f: &mut Fixture) {
    assert_eq!(f.harness.backend_calls(), 0);
    let financial = f.harness.status().await;
    assert_eq!(financial["submissions"], 0);
    assert_eq!(financial["publications"], 0);
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
    assert!(f
        .harness
        .negotiation
        .pending(None, 64)
        .await
        .unwrap()
        .is_empty());
    assert!(f
        .harness
        .session_frame_tags()
        .await
        .iter()
        .all(|t| matches!(t.as_str(), "p.describe.open" | "p.describe")));
}

async fn public_fixture(f: &Fixture, response: Value) -> Value {
    let detail = path(f).strip_suffix("/contract").unwrap().to_owned();
    let (status, publication) = f.get(&detail, "owner-fixture-key").await;
    assert_eq!(status, StatusCode::OK, "{publication}");
    assert_eq!(publication["digest"], response["offer_digest"]);
    assert_eq!(
        publication["membership"]["recipe_hash"],
        response["recipe_hash"]
    );
    json!({"uri":format!("{}?rail=fiat",path(f)),
        "canonical_contract_hash":response["endpoint_contract"],
        "response":response,"publication":publication})
}

fn export_fixture(extension: Option<&str>, value: Value) {
    if let Some(path) = std::env::var_os("MAYHEM_TEST_PROXY_CONTRACT_FIXTURE") {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut path = std::path::PathBuf::from(path);
        if let Some(extension) = extension {
            path.set_extension(extension);
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        file.write_all(&serde_json::to_vec_pretty(&value).unwrap())
            .unwrap();
        file.sync_all().unwrap();
    }
}

#[tokio::test]
async fn exact_contract_returns_all_four_normative_contracts_without_purchase() {
    let mut fixtures = Vec::new();
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let mut f = Fixture::start_with(endpoint, ProxyRail::Fiat).await;
        let (status, headers, value) =
            get(&f, "?rail=fiat", Some("owner-fixture-key"), Body::empty()).await;
        assert_eq!(status, StatusCode::OK, "{value}");
        assert_eq!(headers["cache-control"], "private, no-store");
        assert!(!headers.contains_key("x-mayhem-job-id"));
        assert_eq!(value.as_object().unwrap().len(), 16);
        assert_eq!(
            value["contract"],
            json!(f.harness.adapter.public_snapshot().contract)
        );
        assert_eq!(
            value["endpoint_contract"],
            f.harness.adapter.contract_hash().as_str()
        );
        assert_eq!(
            value["recipe_hash"],
            f.harness.adapter.recipe_hash().as_str()
        );
        assert_eq!(
            value["offer_digest"],
            f.harness.template.terms.offer.digest().unwrap()
        );
        assert_eq!(value["network"], json!(f.harness.network));
        assert_eq!(value["model"], model(&f.harness));
        assert_eq!(value["rail"], "fiat");
        assert_eq!(
            value["metering_policy"],
            mayhem_proxy::metering::Policy::for_endpoint(endpoint).definition()
        );
        assert!(
            value["observed_at_ms"].as_u64().unwrap() < value["expires_at_ms"].as_u64().unwrap()
        );
        for key in [
            "limits",
            "upstream_model",
            "connection",
            "base_url",
            "token",
            "availability",
            "prices",
            "request",
            "job_id",
        ] {
            assert!(
                value.get(key).is_none(),
                "private or irrelevant field: {key}"
            );
        }
        assert!(!serde_json::to_string(&value)
            .unwrap()
            .contains("upstream-model"));
        fixtures.push(public_fixture(&f, value).await);
        unchanged(&mut f).await;
        assert_eq!(f.harness.capacity_status().route_occupied, 0);
        f.stop().await;
    }
    export_fixture(
        Some("builtins.json"),
        json!({"schema_version":1,"test_only":true,"fixtures":fixtures}),
    );
}

#[tokio::test]
async fn custom_public_contract_is_canonical_and_independent_of_full_capacity() {
    let mut contract = mayhem_proto::endpoint_family_contract_template(
        mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
    )
    .unwrap();
    let temperature = contract
        .request_attribute_specs
        .get_mut("temperature")
        .unwrap();
    temperature.maximum = Some(0.5);
    temperature.calibration_values = vec![json!(0.2)];
    contract
        .request_attribute_specs
        .get_mut("user")
        .unwrap()
        .calibration_values = vec![json!("試験 café 🧪")];
    let hash = mayhem_proto::endpoint_contract_canonical_fingerprint(&contract);
    let mut f = Fixture::start_with_contract(
        ProxyEndpoint::Chat,
        ProxyRail::Fiat,
        None,
        "owner-fixture-key",
        Some(contract.clone()),
    )
    .await;
    let first = f.harness.reserve_test_capacity(1701);
    let second = f.harness.reserve_test_capacity(1703);
    assert_eq!(f.harness.capacity_status().available, 0);
    let slots = f.runtime.slots.clone().try_acquire_many_owned(2).unwrap();
    f.control.control.select_markets(Vec::new()).unwrap();
    let (status, _, value) = get(&f, "?rail=fiat", Some("owner-fixture-key"), Body::empty()).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["contract"], json!(contract));
    assert_eq!(value["endpoint_contract"], hash);
    assert_eq!(f.harness.capacity_status().route_occupied, 2);
    let mut fixture = public_fixture(&f, value).await;
    fixture["schema_version"] = json!(1);
    fixture["test_only"] = json!(true);
    export_fixture(None, fixture);
    unchanged(&mut f).await;
    drop(slots);
    f.harness.release_test_capacity(first);
    f.harness.release_test_capacity(second);
    f.stop().await;
}

#[tokio::test]
async fn contract_requires_auth_model_scope_explicit_rail_and_no_request_body() {
    let mut f = Fixture::start_with(ProxyEndpoint::Chat, ProxyRail::Fiat).await;
    for token in [None, Some("wrong-key")] {
        assert_eq!(
            get(&f, "?rail=fiat", token, Body::empty()).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    for query in [
        "",
        "?rail=other",
        "?rail=fiat&extra=true",
        "?rail=fiat&rail=tap",
    ] {
        let (status, _, error) = get(&f, query, Some("owner-fixture-key"), Body::empty()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {error}");
        assert_eq!(error["error"]["code"], "proxy_contract_invalid");
    }
    assert_eq!(
        get(
            &f,
            "?rail=fiat",
            Some("owner-fixture-key"),
            Body::from("{}")
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    f.tokens.tokens[0].models = vec!["another-model".into()];
    std::fs::write(
        f.directory.path().join("tokens.json"),
        serde_json::to_vec(&f.tokens).unwrap(),
    )
    .unwrap();
    assert_eq!(
        get(&f, "?rail=fiat", Some("owner-fixture-key"), Body::empty())
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    f.tokens.tokens[0].models = vec![];
    f.tokens.tokens[0].budget_au = Some(0);
    std::fs::write(
        f.directory.path().join("tokens.json"),
        serde_json::to_vec(&f.tokens).unwrap(),
    )
    .unwrap();
    assert_eq!(
        get(&f, "?rail=fiat", Some("owner-fixture-key"), Body::empty())
            .await
            .0,
        StatusCode::OK
    );
    for revoked in [true, false] {
        f.tokens.tokens[0].revoked_at = revoked.then(now_secs);
        f.tokens.tokens[0].expires_at = (!revoked).then(|| now_secs().saturating_sub(1));
        std::fs::write(
            f.directory.path().join("tokens.json"),
            serde_json::to_vec(&f.tokens).unwrap(),
        )
        .unwrap();
        assert_eq!(
            get(&f, "?rail=fiat", Some("owner-fixture-key"), Body::empty())
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    f.tokens.tokens[0].expires_at = None;
    std::fs::write(
        f.directory.path().join("tokens.json"),
        serde_json::to_vec(&f.tokens).unwrap(),
    )
    .unwrap();
    let missing = format!(
        "/v1/proxy/offers/{}/{}/{}/contract?rail=fiat",
        "a".repeat(64),
        "b".repeat(64),
        "c".repeat(64)
    );
    let (status, error) = f.get(&missing, "owner-fixture-key").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{error}");
    let mut disabled = f.state.clone();
    disabled.proxy_buyer = None;
    f.router = openai_router(disabled);
    let (status, _, error) = get(&f, "?rail=fiat", Some("owner-fixture-key"), Body::empty()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["error"]["code"], "proxy_buyer_disabled");
    unchanged(&mut f).await;
    f.stop().await;
}

#[tokio::test]
async fn contract_reads_require_keys_even_when_native_reads_are_anonymous_and_preserve_rails() {
    let router = openai_router(GatewayState::fixture());
    let contract_path = format!(
        "/v1/proxy/offers/{}/{}/{}/contract?rail=fiat",
        "a".repeat(64),
        "b".repeat(64),
        "c".repeat(64)
    );
    for (path, status) in [
        ("/v1/models", StatusCode::OK),
        (contract_path.as_str(), StatusCode::UNAUTHORIZED),
    ] {
        let response = router
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    for (rail, label) in [(ProxyRail::Tnk, "tnk"), (ProxyRail::Tap, "tap")] {
        let mut f = Fixture::start_with(ProxyEndpoint::Chat, rail).await;
        let (status, _, value) = get(
            &f,
            &format!("?rail={label}"),
            Some("owner-fixture-key"),
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{value}");
        assert_eq!(value["rail"], label);
        assert_eq!(
            value["settlement_policy_hash"],
            f.harness.policy.digest().unwrap()
        );
        unchanged(&mut f).await;
        f.stop().await;
    }
}

#[tokio::test]
async fn concurrent_contract_reads_are_bounded_and_old_peers_fail_unavailable() {
    let mut f = Fixture::start_with(ProxyEndpoint::Chat, ProxyRail::Fiat).await;
    let results = futures_util::future::join_all(
        (0..16).map(|_| get(&f, "?rail=fiat", Some("owner-fixture-key"), Body::empty())),
    )
    .await;
    let successes = results.iter().filter(|r| r.0 == StatusCode::OK).count();
    assert!(
        successes > 0 && successes <= mayhem_proxy::descriptor::READS,
        "{results:?}"
    );
    for (status, _, value) in results {
        if status != StatusCode::OK {
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{value}");
            assert_eq!(value["error"]["code"], "proxy_contract_busy");
        }
    }
    f.harness.stop_descriptors();
    let started = std::time::Instant::now();
    let (status, _, value) = get(&f, "?rail=fiat", Some("owner-fixture-key"), Body::empty()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{value}");
    assert_eq!(value["error"]["code"], "proxy_contract_unavailable");
    assert!(started.elapsed() < Duration::from_secs(7));
    unchanged(&mut f).await;
    f.stop().await;
}

#[tokio::test]
async fn catalog_offer_change_during_descriptor_does_not_substitute_new_revision() {
    use mayhem_proxy::discovery::{Entry, Mode, Page, QueryBinding, CATALOG_PREFIX};
    let mut f = Fixture::start_with(ProxyEndpoint::Chat, ProxyRail::Fiat).await;
    // Freeze background hydration so the deliberate local canonical-snapshot
    // test update cannot be immediately replaced by the fixture's older view.
    f.control.pause().await;
    f.harness.command("delay").await;
    let original = f.harness.template.terms.offer.clone();
    let response = {
        let pending = get(&f, "?rail=fiat", Some("owner-fixture-key"), Body::empty());
        tokio::pin!(pending);
        let change = async {
            tokio::time::timeout(Duration::from_secs(2), async {
                while !f
                    .harness
                    .session_frame_tags()
                    .await
                    .iter()
                    .any(|s| s == "p.describe.open")
                {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let catalog = f.control.control.catalog();
            let current = catalog.read().unwrap().status().committed.unwrap();
            let mut next = original.clone();
            next.revision += 1;
            let mut proof = current.proof.clone();
            proof.signed_length += 1;
            proof.tree_hash = format!("{:064x}", proof.signed_length);
            let row = Entry {
                key: format!(
                    "{CATALOG_PREFIX}offers/{}/{}/{}",
                    next.market_id,
                    next.provider_pubkey,
                    next.slot_id().unwrap()
                ),
                value: json!({"active":true,"revision":next.revision,"digest":next.digest().unwrap(),"offer":next}),
            };
            let page = Page {
                ok: true,
                lane: "proxy".into(),
                schema_version: 1,
                request_nonce: "c".repeat(64),
                query: QueryBinding::catalog(),
                context: current.context,
                proof,
                mode: Mode::Changes,
                base_proof: Some(current.proof),
                entries: vec![row],
                truncated: false,
                next_cursor: None,
                checkpoint: Some(format!("pdc1.contract.{}", "e".repeat(128))),
            };
            catalog
                .apply(
                    &catalog.refresh_ticket().unwrap(),
                    &page,
                    super::super::super::now_millis_u64(),
                )
                .unwrap();
        };
        let (response, ()) = tokio::join!(pending, change);
        response
    };
    assert_eq!(
        response.0,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        response.2
    );
    assert_eq!(response.2["error"]["code"], "proxy_contract_unavailable");
    unchanged(&mut f).await;
    f.stop().await;
}
