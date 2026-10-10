use super::estimation::{assert_no_purchase, ESTIMATE_CASE};
use super::*;

pub(super) fn start(f: &Fixture) -> Value {
    let mut request = f.body();
    request.as_object_mut().unwrap().remove("model");
    let controls = request["proxy"].clone();
    let published = f
        .control
        .control
        .catalog()
        .read()
        .unwrap()
        .proxy_offer(
            model(&f.harness).strip_prefix("proxy/offer/").unwrap(),
            super::super::super::now_millis_u64(),
        )
        .unwrap()
        .unwrap();
    request["proxy"]["profile"] = json!({"schema_version":1,"lane":"proxy","endpoint":f.harness.adapter.endpoint(),
        "target":{"kind":"category","family_ids":[published.market.model.family_id],"variants":[],"tags":[],"market_allowlist":null},
        "providers":{"allow":null,"deny":[],"require_verified_operator":false},"allowed_rails":[controls["rail"]],
        "prices":controls["prices"],"max_retail_cost_micro":"1000000",
        "settlement_policies":[{"rail":controls["rail"],"settlement_policy_hash":controls["settlement_policy_hash"]}],
        "constraints":{"minimum_context":null,"minimum_tokens_per_second":null,"output_units":controls["output_units"],"capabilities":[],"request_controls":[],"data_handling":[]},
        "ranking":"lowest_estimated_cost","continuity":"retain_compatible"});
    json!({"schema_version":1,"kind":"start","endpoint":f.harness.adapter.endpoint(),"request":request,"previous_model":null})
}
pub(super) async fn send(router: axum::Router, body: Value, token: &str) -> (StatusCode, Value) {
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/proxy/profile/resolve")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert!(!response.headers().contains_key("x-mayhem-job-id"));
    (
        status,
        serde_json::from_slice(
            &to_bytes(response.into_body(), 2 * 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap(),
    )
}
async fn configured(limits: ProfileResolutionLimits) -> Fixture {
    Fixture::start_configured(
        ProxyEndpoint::Chat,
        ProxyRail::Fiat,
        None,
        "owner-fixture-key",
        None,
        None,
        Some(limits),
    )
    .await
}
fn page_limits() -> ProfileResolutionLimits {
    ProfileResolutionLimits {
        candidates_per_step: 1,
        ..Default::default()
    }
}

#[tokio::test]
async fn category_resolution_uses_exact_shared_estimates_for_four_families_without_purchase() {
    let _case = ESTIMATE_CASE.lock().await;
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let mut f = Fixture::start_with(endpoint, ProxyRail::Fiat).await;
        let input = start(&f);
        let (code, value) = send(f.router.clone(), input.clone(), "owner-fixture-key").await;
        assert_eq!(code, StatusCode::OK, "{value}");
        assert_eq!(value["status"], "selected", "{value}");
        assert_eq!(value["scope_exhausted"], true);
        assert_eq!(
            value["request_content_digest"],
            retail_request_content_digest(&input["request"]).unwrap()
        );
        assert_eq!(
            value["model_allowlist_digest"],
            retail_request_content_digest(&Value::Null).unwrap()
        );
        assert_eq!(value["selection"]["model"], model(&f.harness));
        assert_eq!(value["ranking_basis"], "maximum_wholesale_au");
        assert_eq!(value["retail_pricing"], "not_applied");
        assert_eq!(value["authorizes_execution"], false);
        let (code, _, estimate) = super::estimation::estimate(
            &f,
            value["selection"]["request"].clone(),
            "owner-fixture-key",
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{estimate}");
        for key in [
            "request_hash",
            "request_content_digest",
            "controls",
            "offer",
            "max_usage",
            "max_spend_au",
            "membership_digest",
            "recipe_hash",
        ] {
            assert_eq!(value["selection"]["estimate"][key], estimate[key], "{key}");
        }
        if endpoint == ProxyEndpoint::Chat {
            if let Some(path) = std::env::var_os("MAYHEM_TEST_PROXY_RESOLVER_FIXTURE") {
                use std::{io::Write, os::unix::fs::OpenOptionsExt};
                let mut out = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path)
                    .unwrap();
                out.write_all(&serde_json::to_vec_pretty(&json!({"schema_version":1,"test_only":true,"request":input,"response":value})).unwrap()).unwrap();
                out.sync_all().unwrap();
            }
        }
        assert_no_purchase(&mut f).await;
        f.stop().await;
    }
}

#[tokio::test]
async fn continuation_owns_complete_progression_replay_and_key_scope() {
    let _case = ESTIMATE_CASE.lock().await;
    let mut f = configured(page_limits()).await;
    let input = start(&f);
    let (code, pending) = send(f.router.clone(), input, "owner-fixture-key").await;
    assert_eq!(code, StatusCode::OK, "{pending}");
    assert_eq!(pending["status"], "pending");
    assert_eq!(pending["scope_exhausted"], false);
    assert!(pending["selection"].is_null());
    assert_eq!(pending["ranking_claim"], "none");
    let continuation = pending["continuation"].clone();
    let (code, foreign) = send(f.router.clone(), continuation.clone(), "other-fixture-key").await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(foreign["error"]["code"], "proxy_profile_resolution_expired");
    let mut substituted = continuation.clone();
    substituted["request"] = json!({});
    assert_eq!(
        send(f.router.clone(), substituted, "owner-fixture-key")
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let mut skipped = continuation.clone();
    skipped["revision"] = json!(9);
    assert_eq!(
        send(f.router.clone(), skipped, "owner-fixture-key").await.0,
        StatusCode::BAD_REQUEST
    );
    let (a, b) = tokio::join!(
        send(f.router.clone(), continuation.clone(), "owner-fixture-key"),
        send(f.router.clone(), continuation.clone(), "owner-fixture-key")
    );
    let selected = if a.0 == StatusCode::OK {
        a.1
    } else {
        assert_eq!(b.0, StatusCode::OK);
        b.1
    };
    assert_eq!(selected["status"], "selected");
    assert_eq!(selected["considered_candidates"], 1);
    assert_eq!(selected["revision"], 2);
    let replay = send(f.router.clone(), continuation, "owner-fixture-key").await;
    assert_eq!(replay.0, StatusCode::OK);
    assert_eq!(replay.1, selected);
    assert_no_purchase(&mut f).await;
    f.stop().await;
}

#[tokio::test]
async fn restrictive_model_scope_continuity_and_unknown_evidence_never_invent_selection() {
    let _case = ESTIMATE_CASE.lock().await;
    let mut f = Fixture::start_with(ProxyEndpoint::Chat, ProxyRail::Fiat).await;
    let mut input = start(&f);
    input["model_allowlist"] = json!([]);
    let (_, excluded) = send(f.router.clone(), input.clone(), "owner-fixture-key").await;
    assert_eq!(excluded["status"], "no_match", "{excluded}");
    assert_eq!(excluded["exclusions"]["key_model_excluded"], 1);
    input["model_allowlist"] = json!([model(&f.harness), model(&f.harness)]);
    input["previous_model"] = json!(model(&f.harness));
    let (_, retained) = send(f.router.clone(), input.clone(), "owner-fixture-key").await;
    assert_eq!(retained["status"], "retained_compatible", "{retained}");
    assert_eq!(retained["scope_exhausted"], false);
    assert_eq!(retained["ranking_claim"], "continuity_retention");
    assert_eq!(
        retained["model_allowlist_digest"],
        retail_request_content_digest(&json!([model(&f.harness)])).unwrap()
    );
    input["request"]["proxy"]["profile"]["continuity"] = json!("reselect_per_request");
    let (_, selected) = send(f.router.clone(), input.clone(), "owner-fixture-key").await;
    assert_eq!(selected["status"], "selected");
    for (path, value, code) in [
        (
            "ranking",
            json!("preferred_speed"),
            "proxy_profile_speed_ranking_unavailable",
        ),
        (
            "evidence",
            json!(true),
            "proxy_profile_evidence_unavailable",
        ),
    ] {
        let mut changed = input.clone();
        if path == "ranking" {
            changed["request"]["proxy"]["profile"]["ranking"] = value;
        } else {
            changed["request"]["proxy"]["profile"]["providers"]["require_verified_operator"] =
                value;
        }
        let (status, error) = send(f.router.clone(), changed, "owner-fixture-key").await;
        if path == "evidence" {
            assert_eq!(status, StatusCode::OK);
            assert_eq!(error["status"], "incomplete");
            assert!(error["selection"].is_null());
            assert_eq!(error["ranking_claim"], "none");
        } else {
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(error["error"]["code"], code);
        }
    }
    f.harness.stop_descriptors();
    let (_, unresolved) = send(f.router.clone(), input, "owner-fixture-key").await;
    // The only supplier stopped its descriptor service. The complete catalog
    // has no usable offer; this is not missing catalog or trust evidence.
    assert_eq!(unresolved["status"], "no_match", "{unresolved}");
    assert_eq!(unresolved["unresolved_candidates"], 0);
    assert!(unresolved["selection"].is_null());
    assert_eq!(unresolved["ranking_claim"], "none");
    assert_eq!(unresolved["exclusions"]["descriptor_unavailable"], 1);
    assert_no_purchase(&mut f).await;
    f.stop().await;
}

#[tokio::test]
async fn current_key_permissions_and_retention_expiry_are_checked_before_continuation() {
    let _case = ESTIMATE_CASE.lock().await;
    let mut f = configured(ProfileResolutionLimits {
        ttl_ms: 1000,
        ..page_limits()
    })
    .await;
    let input = start(&f);
    let (_, pending) = send(f.router.clone(), input.clone(), "owner-fixture-key").await;
    assert_eq!(pending["status"], "pending");
    let mut tokens = f.tokens.clone();
    tokens.tokens[0].models.clear();
    std::fs::write(
        f.directory.path().join("tokens.json"),
        serde_json::to_vec(&tokens).unwrap(),
    )
    .unwrap();
    let (code, changed) = send(
        f.router.clone(),
        pending["continuation"].clone(),
        "owner-fixture-key",
    )
    .await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        changed["error"]["code"],
        "proxy_profile_resolution_permissions_changed"
    );
    tokens.tokens[0].revoked_at = Some(now_secs());
    std::fs::write(
        f.directory.path().join("tokens.json"),
        serde_json::to_vec(&tokens).unwrap(),
    )
    .unwrap();
    assert_eq!(
        send(
            f.router.clone(),
            pending["continuation"].clone(),
            "owner-fixture-key"
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    std::fs::write(
        f.directory.path().join("tokens.json"),
        serde_json::to_vec(&f.tokens).unwrap(),
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(1050)).await;
    let (code, expired) = send(
        f.router.clone(),
        pending["continuation"].clone(),
        "owner-fixture-key",
    )
    .await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(expired["error"]["code"], "proxy_profile_resolution_expired");
    let (_, restarted) = send(f.router.clone(), input, "owner-fixture-key").await;
    assert_eq!(restarted["status"], "pending");
    assert_ne!(restarted["resolution_id"], pending["resolution_id"]);
    assert_no_purchase(&mut f).await;
    f.stop().await;
}

#[tokio::test]
async fn custom_descriptor_controls_and_stricter_caps_remain_exact() {
    let _case = ESTIMATE_CASE.lock().await;
    let registry = super::profile::Registry::start().await;
    let mut contract = mayhem_proto::endpoint_family_contract_template(
        mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
    )
    .unwrap();
    contract
        .request_attribute_specs
        .get_mut("temperature")
        .unwrap()
        .maximum = Some(0.5);
    let mut f = Fixture::start_with_registry(
        ProxyEndpoint::Chat,
        ProxyRail::Fiat,
        None,
        "owner-fixture-key",
        Some(contract),
        Some(registry.config()),
    )
    .await;
    let mut input = start(&f);
    input["request"]["messages"] = json!([{"role":"user","content":"Café 🧠 𐀀"}]);
    input["request"]["proxy"]["profile"]["constraints"]["request_controls"] = json!([
        {"field_id":"fixture.temperature","schema_revision":1,"value":{"type":"decimal","value":"0.2"}}]);
    let (code, selected) = send(f.router.clone(), input.clone(), "owner-fixture-key").await;
    assert_eq!(code, StatusCode::OK, "{selected}");
    assert_eq!(selected["status"], "selected", "{selected}");
    assert_eq!(selected["selection"]["request"]["temperature"], 0.2);
    assert!(selected["registry_release"].is_object());
    assert_eq!(
        selected["request_content_digest"],
        retail_request_content_digest(&input["request"]).unwrap()
    );
    let (code, _, exact) = super::estimation::estimate(
        &f,
        selected["selection"]["request"].clone(),
        "owner-fixture-key",
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{exact}");
    assert_eq!(
        exact["request_hash"],
        selected["selection"]["estimate"]["request_hash"]
    );
    assert_eq!(
        exact["max_spend_au"],
        selected["selection"]["estimate"]["max_spend_au"]
    );
    input["request"]["proxy"]["prices"]["max_total_spend_au"] = json!("1");
    let (_, refused) = send(f.router.clone(), input, "owner-fixture-key").await;
    assert!(refused["selection"].is_null());
    assert_ne!(refused["status"], "selected");
    assert_no_purchase(&mut f).await;
    f.stop().await;
}

#[tokio::test]
async fn offer_changes_and_resource_budgets_cannot_complete_a_stale_or_partial_minimum() {
    use mayhem_proxy::discovery::{Entry, Mode, Page, QueryBinding, CATALOG_PREFIX};
    let _case = ESTIMATE_CASE.lock().await;
    let mut f = configured(ProfileResolutionLimits {
        retained_sessions: 1,
        ..page_limits()
    })
    .await;
    let input = start(&f);
    let (_, pending) = send(f.router.clone(), input.clone(), "owner-fixture-key").await;
    assert_eq!(pending["status"], "pending");
    let (code, busy) = send(f.router.clone(), input, "owner-fixture-key").await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(busy["error"]["code"], "proxy_profile_resolution_busy");
    let catalog = f.control.control.catalog();
    let current = catalog.read().unwrap().status().committed.unwrap();
    let mut next = f.harness.template.terms.offer.clone();
    next.revision += 1;
    let mut proof = current.proof.clone();
    proof.signed_length += 1;
    proof.tree_hash = format!("{:064x}", proof.signed_length);
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
        entries: vec![Entry {
            key: format!(
                "{CATALOG_PREFIX}offers/{}/{}/{}",
                next.market_id,
                next.provider_pubkey,
                next.slot_id().unwrap()
            ),
            value: json!({"active":true,"revision":next.revision,"digest":next.digest().unwrap(),"offer":next}),
        }],
        truncated: false,
        next_cursor: None,
        checkpoint: Some(format!("pdc1.resolver.{}", "e".repeat(128))),
    };
    catalog
        .apply(
            &catalog.refresh_ticket().unwrap(),
            &page,
            super::super::super::now_millis_u64(),
        )
        .unwrap();
    let (code, refresh) = send(
        f.router.clone(),
        pending["continuation"].clone(),
        "owner-fixture-key",
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(refresh["status"], "refresh_required", "{refresh}");
    assert!(refresh["selection"].is_null());
    assert_eq!(refresh["ranking_claim"], "none");
    assert_no_purchase(&mut f).await;
    f.stop().await;
}

#[tokio::test]
async fn retail_projection_is_bound_capped_and_keeps_original_wholesale_quote() {
    let _case = ESTIMATE_CASE.lock().await;
    let mut f = Fixture::start_with(ProxyEndpoint::Chat, ProxyRail::Fiat).await;
    let mut input = start(&f);
    input["retail_ranking"] = json!({"margin_bps":333,"max_cost_micro":"1000000"});
    let (code, selected) = send(f.router.clone(), input.clone(), "owner-fixture-key").await;
    assert_eq!(code, StatusCode::OK, "{selected}");
    assert_eq!(selected["status"], "selected");
    assert_eq!(selected["ranking_basis"], "maximum_retail_micro");
    assert_eq!(selected["retail_pricing"], "applied_projection");
    assert_eq!(selected["retail_ranking"], input["retail_ranking"]);
    assert!(selected["selection"]["maximum_retail_cost_micro"].is_string());
    let (code, _, wholesale) = super::estimation::estimate(
        &f,
        selected["selection"]["request"].clone(),
        "owner-fixture-key",
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        wholesale["offer"],
        selected["selection"]["estimate"]["offer"]
    );
    assert_eq!(
        wholesale["max_spend_au"],
        selected["selection"]["estimate"]["max_spend_au"]
    );
    if let Some(path) = std::env::var_os("MAYHEM_TEST_PROXY_RESOLVER_RETAIL_FIXTURE") {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        out.write_all(
            &serde_json::to_vec_pretty(
                &json!({"schema_version":1,"test_only":true,"request":input,"response":selected}),
            )
            .unwrap(),
        )
        .unwrap();
        out.sync_all().unwrap();
    }
    input["retail_ranking"]["max_cost_micro"] = json!("1000001");
    assert_eq!(
        send(f.router.clone(), input.clone(), "owner-fixture-key")
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    input["retail_ranking"]["max_cost_micro"] = json!("1000000");
    input["retail_ranking"]["margin_bps"] = json!(u64::MAX);
    assert_eq!(
        send(f.router.clone(), input, "owner-fixture-key").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_no_purchase(&mut f).await;
    f.stop().await;
}

#[tokio::test]
async fn observation_budget_resumes_and_winning_interest_expires_without_changing_explicit_selection(
) {
    let _case = ESTIMATE_CASE.lock().await;
    let mut f = configured(ProfileResolutionLimits {
        ttl_ms: 1000,
        ..Default::default()
    })
    .await;
    let explicit = vec![support::digest(901), support::digest(902)];
    f.control.control.select_markets(explicit.clone()).unwrap();
    let input = start(&f);
    let (_, pending) = send(f.router.clone(), input, "owner-fixture-key").await;
    assert_eq!(pending["status"], "pending", "{pending}");
    assert_eq!(pending["pending_reason"], "observation_budget");
    assert_eq!(pending["considered_candidates"], 0);
    assert!(pending["selection"].is_null());
    f.control
        .control
        .select_markets(vec![explicit[0].clone()])
        .unwrap();
    let (_, selected) = send(
        f.router.clone(),
        pending["continuation"].clone(),
        "owner-fixture-key",
    )
    .await;
    assert_eq!(selected["status"], "selected", "{selected}");
    assert!(
        selected["selection"]["estimate"]["expires_at_ms"]
            .as_u64()
            .unwrap()
            <= selected["retention_expires_at_ms"].as_u64().unwrap()
    );
    let offer = &f.harness.template.terms.offer;
    let status = || {
        f.control
            .control
            .presence()
            .status(
                &Digest::new(&offer.market_id).unwrap(),
                &Digest::new(&offer.provider_pubkey).unwrap(),
                &Digest::new(offer.slot_id().unwrap()).unwrap(),
                None,
            )
            .unwrap()
    };
    assert_eq!(status(), mayhem_proxy::presence::Eligibility::Available);
    assert!(f
        .control
        .control
        .presence()
        .observe_market(support::digest(903))
        .is_err());
    tokio::time::sleep(Duration::from_millis(1050)).await;
    f.runtime.resolver.expire();
    assert_eq!(
        status(),
        mayhem_proxy::presence::Eligibility::HeartbeatMissing
    );
    assert!(f
        .control
        .control
        .presence()
        .observe_market(support::digest(903))
        .is_ok());
    assert_no_purchase(&mut f).await;
    f.stop().await;
}

#[tokio::test]
async fn candidate_observation_deadline_is_unresolved_and_does_not_claim_empty_or_cheapest() {
    let _case = ESTIMATE_CASE.lock().await;
    let mut f = configured(ProfileResolutionLimits {
        observation_timeout_ms: 50,
        ..Default::default()
    })
    .await;
    f.control
        .control
        .select_markets(vec![support::digest(901), support::digest(902)])
        .unwrap();
    let (_, pending) = send(f.router.clone(), start(&f), "owner-fixture-key").await;
    assert_eq!(pending["pending_reason"], "observation_budget");
    tokio::time::sleep(Duration::from_millis(60)).await;
    let (_, incomplete) = send(
        f.router.clone(),
        pending["continuation"].clone(),
        "owner-fixture-key",
    )
    .await;
    assert_eq!(incomplete["status"], "incomplete", "{incomplete}");
    assert_eq!(incomplete["considered_candidates"], 1);
    assert_eq!(incomplete["exclusions"]["observation_deadline"], 1);
    assert_eq!(incomplete["scope_exhausted"], true);
    assert!(incomplete["selection"].is_null());
    assert_eq!(incomplete["ranking_claim"], "none");
    assert_no_purchase(&mut f).await;
    f.stop().await;
}

#[tokio::test]
async fn slow_continuation_uses_current_healthy_evidence_after_observation_deadline() {
    let _case = ESTIMATE_CASE.lock().await;
    let mut f = configured(ProfileResolutionLimits {
        observation_timeout_ms: 50,
        ..Default::default()
    })
    .await;
    f.control
        .control
        .select_markets(vec![support::digest(901), support::digest(902)])
        .unwrap();
    let (_, pending) = send(f.router.clone(), start(&f), "owner-fixture-key").await;
    assert_eq!(pending["pending_reason"], "observation_budget");
    tokio::time::sleep(Duration::from_millis(60)).await;
    let market = Digest::new(&f.harness.template.terms.offer.market_id).unwrap();
    f.control.control.select_markets(vec![market]).unwrap();
    let (_, selected) = send(
        f.router.clone(),
        pending["continuation"].clone(),
        "owner-fixture-key",
    )
    .await;
    assert_eq!(selected["status"], "selected", "{selected}");
    assert_eq!(selected["selection"]["model"], model(&f.harness));
    assert_eq!(selected["unresolved_candidates"], 0);
    assert_no_purchase(&mut f).await;
    f.stop().await;
}
