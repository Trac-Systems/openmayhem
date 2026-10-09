use super::estimation::{assert_no_purchase, ESTIMATE_CASE};
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn wanted(f: &Fixture) -> Value {
    let mut input = resolver::start(f);
    input["retail_ranking"] = json!({"margin_bps":0,"max_cost_micro":"1000000"});
    input["request"]["proxy"]["require_verified_operator"] = json!(true);
    input["request"]["proxy"]["profile"]["providers"]["require_verified_operator"] = json!(true);
    input
}

#[tokio::test]
async fn operator_canonical_evidence_selects_quotes_and_replays_four_families_after_revocation() {
    let _case = ESTIMATE_CASE.lock().await;
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let mut f = Fixture::start_with(endpoint, ProxyRail::Fiat).await;
        let (_, _, unconstrained) = estimation::estimate(&f, f.body(), "owner-fixture-key").await;
        assert!(unconstrained.get("estimate_hash").is_some());
        assert_eq!(
            f.harness.status().await["operator_calls"],
            0,
            "unfiltered quote adds no canonical read"
        );
        f.harness.command("operator_verify").await;
        let input = wanted(&f);
        let (status, selected) =
            resolver::send(f.router.clone(), input.clone(), "owner-fixture-key").await;
        assert_eq!(status, StatusCode::OK, "{selected}");
        assert_eq!(selected["status"], "selected", "{selected}");
        assert_eq!(selected["unresolved_candidates"], 0);
        let request = selected["selection"]["request"].clone();
        assert_eq!(request["proxy"]["require_verified_operator"], true);
        assert_eq!(
            request["proxy"]["profile"]["providers"]["require_verified_operator"],
            true
        );
        let (status, _, quote) =
            estimation::estimate(&f, request.clone(), "owner-fixture-key").await;
        assert_eq!(status, StatusCode::OK, "{quote}");
        assert!(
            quote["expires_at_ms"].as_u64().unwrap()
                <= super::super::super::now_millis_u64() + mayhem_proxy::operator::MAX_AGE_MS
        );
        assert_no_purchase(&mut f).await;
        let (status, id, result) = f
            .post(
                "verified-original",
                request.clone(),
                "owner-fixture-key",
                false,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(f.harness.backend_calls(), 1);
        f.harness.command("operator_revoke").await;
        let before = f.harness.status().await;
        let (status, again, replay) = f
            .post(
                "verified-original",
                request.clone(),
                "owner-fixture-key",
                false,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{replay}");
        assert_eq!(again, id);
        assert_eq!(replay, result);
        assert_eq!(
            f.harness.status().await["operator_calls"],
            before["operator_calls"],
            "accepted replay never rereads KYB"
        );
        let (status, _, rejected) = f
            .post("verified-new", request, "owner-fixture-key", false)
            .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{rejected}");
        assert_eq!(f.harness.backend_calls(), 1);
        assert_eq!(
            f.harness.status().await["publications"],
            before["publications"]
        );
        f.harness.command("operator_unavailable").await;
        assert_eq!(
            f.post(
                "verified-original",
                selected["selection"]["request"].clone(),
                "owner-fixture-key",
                false
            )
            .await
            .0,
            StatusCode::OK
        );
        if endpoint == ProxyEndpoint::Chat {
            if let Some(path) = std::env::var_os("MAYHEM_TEST_PROXY_OPERATOR_FIXTURE") {
                use std::{io::Write, os::unix::fs::OpenOptionsExt};
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(path)
                    .unwrap();
                file.write_all(&serde_json::to_vec_pretty(&json!({"test_only":true,"request":input,"selected":selected,"estimate":quote,
                    "result":result,"replay":replay,"backend_calls":f.harness.backend_calls(),
                    "revoked_new_request":rejected})).unwrap()).unwrap();
            }
        }
        f.stop().await;
    }
}

#[tokio::test]
async fn operator_negative_evidence_excludes_but_unavailable_unknown_never_proves_scope_cheapest() {
    let _case = ESTIMATE_CASE.lock().await;
    for command in [
        "operator_absent",
        "operator_revoke",
        "operator_inactive",
        "operator_unknown",
        "operator_unavailable",
    ] {
        let mut f = Fixture::start_with(ProxyEndpoint::Chat, ProxyRail::Fiat).await;
        f.harness.command("operator_verify").await;
        f.harness.command(command).await;
        let (status, result) =
            resolver::send(f.router.clone(), wanted(&f), "owner-fixture-key").await;
        assert_eq!(status, StatusCode::OK, "{command}: {result}");
        let unknown = ["operator_unknown", "operator_unavailable"].contains(&command);
        assert_eq!(
            result["status"],
            if unknown { "incomplete" } else { "no_match" },
            "{result}"
        );
        assert_eq!(result["unresolved_candidates"], if unknown { 1 } else { 0 });
        assert_eq!(result["ranking_claim"], "none");
        assert!(result["selection"].is_null());
        let exclusions = result["exclusions"].to_string();
        assert!(
            exclusions.contains(if unknown {
                "profile_evidence_unavailable"
            } else {
                "operator_not_verified"
            }),
            "{result}"
        );
        assert_no_purchase(&mut f).await;
        f.stop().await;
    }
}

#[derive(Default)]
struct Owner(AtomicUsize);
impl buyer_controller::AuthorizationGate for Owner {
    fn retain_non_admission<'a>(
        &'a self,
        _: &'a mayhem_proxy::financial::negotiation::NonAdmission,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), buyer_controller::GateError>> + Send + 'a>,
    > {
        Box::pin(async { Ok(()) })
    }
    fn retain_verified_output<'a>(
        &'a self,
        _: buyer_controller::VerifiedOutput<'a>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), buyer_controller::GateError>> + Send + 'a>,
    > {
        Box::pin(async { Ok(()) })
    }
    fn authorize<'a>(
        &'a self,
        _: &'a mayhem_proxy::financial::quote::PreparedPurchase,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), buyer_controller::GateError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}
#[tokio::test]
async fn operator_revocation_after_quote_refuses_before_owner_hold_callback() {
    let _case = ESTIMATE_CASE.lock().await;
    let mut f = Fixture::start_with(ProxyEndpoint::Chat, ProxyRail::Fiat).await;
    f.harness.command("operator_verify").await;
    let mut body = f.body();
    body["proxy"]["require_verified_operator"] = json!(true);
    assert_eq!(
        estimation::estimate(&f, body.clone(), "owner-fixture-key")
            .await
            .0,
        StatusCode::OK
    );
    let request = Arc::new(
        proxy_request::Request::parse(ProxyEndpoint::Chat, body, &f.runtime.policy)
            .unwrap()
            .unwrap(),
    );
    let owner = Arc::new(Owner::default());
    let gate = super::super::evidence::gate(f.control.control.clone(), request, owner.clone())
        .await
        .unwrap();
    let purchase = f.harness.prepare().await;
    gate.authorize(&purchase).await.unwrap();
    assert_eq!(owner.0.load(Ordering::SeqCst), 1);
    f.harness.command("operator_revoke").await;
    assert!(gate.authorize(&purchase).await.is_err());
    assert_eq!(
        owner.0.load(Ordering::SeqCst),
        1,
        "revocation must stop before the hold owner"
    );
    assert_eq!(f.harness.backend_calls(), 0);
    assert_eq!(f.harness.status().await["publications"], 0);
    assert_eq!(f.harness.status().await["submissions"], 0);
    f.stop().await;
}
