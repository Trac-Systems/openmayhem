use super::*;
use mayhem_proxy::{
    declaration,
    registry::{Definition, Support, TypedValue},
};
use std::sync::atomic::{AtomicUsize, Ordering};
fn definition() -> Definition {
    serde_json::from_value(json!({"schema_version":1,"field_id":"privacy.training","schema_revision":1,
        "labels":{"en":"Operator promises no training"},"help":{},"units":null,"group":"privacy","order":0,
        "endpoints":["mayhem_decisions","openai_chat_completions","openai_completions","openai_responses"],
        "value_schema":{"type":"boolean"},"default":null,"operators":["eq"],"minimum_assurance":"declared",
        "max_evidence_age_ms":60000,"usage":{"kind":"filter_only"},"rules":[]})).unwrap()
}
fn wanted(f: &Fixture) -> Value {
    let mut value = resolver::start(f);
    value["retail_ranking"] = json!({"margin_bps":0,"max_cost_micro":"1000000"});
    value["request"]["proxy"]["profile"]["constraints"]["data_handling"] = json!([{"field_id":"privacy.training","schema_revision":1,
        "operator":"eq","value":{"type":"boolean","value":false},"evidence":"declared","max_age_ms":60000}]);
    value
}
fn install(f: &Fixture) -> (mayhem_proxy::directory::PublishedOffer, declaration::Signed) {
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
    let subject = declaration::Subject::new(
        f.harness.network.clone(),
        &published.offer,
        &published.membership,
    )
    .unwrap();
    let now = super::super::super::now_millis_u64();
    let record = f.harness.sign_declaration(declaration::Body {
        schema_version: 1,
        subject,
        revision: 1,
        issued_at_ms: now,
        expires_at_ms: now + 60000,
        claims: vec![declaration::Claim {
            field_id: "privacy.training".into(),
            schema_revision: 1,
            definition_digest: Digest::new(definition().digest().unwrap()).unwrap(),
            status: Support::Supported,
            value: Some(TypedValue::Boolean(false)),
        }],
    });
    f.harness
        .declarations
        .install_declarations(vec![record.clone()])
        .unwrap();
    (published, record)
}
async fn prepare(f: &Fixture, body: Value) -> (StatusCode, Value) {
    let response = f.router.clone().oneshot(Request::builder().method("POST").uri("/v1/proxy/profile/prepare")
        .header("content-type", "application/json").header("authorization", "Bearer owner-fixture-key")
        .body(Body::from(serde_json::to_vec(&json!({"schema_version":1,"endpoint":f.harness.adapter.endpoint(),"request":body})).unwrap())).unwrap()).await.unwrap();
    let status = response.status();
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
async fn data_handling_four_endpoints_declared_only_preserves_quotes_filters_and_replay() {
    let _case = estimation::ESTIMATE_CASE.lock().await;
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let registry = profile::Registry::with_definition(definition()).await;
        let mut f = Fixture::start_configured(
            endpoint,
            ProxyRail::Fiat,
            None,
            "owner-fixture-key",
            None,
            Some(registry.config()),
            None,
        )
        .await;
        let input = wanted(&f);
        let (status, missing) =
            resolver::send(f.router.clone(), input.clone(), "owner-fixture-key").await;
        assert_eq!(status, StatusCode::OK, "{missing}");
        assert_eq!(missing["status"], "incomplete", "{missing}");
        let (publication, record) = install(&f);
        let mut exact = input["request"].clone();
        exact["model"] = json!(model(&f.harness));
        let (status, prepared) = prepare(&f, exact).await;
        assert_eq!(status, StatusCode::OK, "{prepared}");
        let ready = prepared["request"].clone();
        assert_eq!(
            ready["proxy"]["profile"]["constraints"]["data_handling"],
            input["request"]["proxy"]["profile"]["constraints"]["data_handling"]
        );
        let (status, selected) =
            resolver::send(f.router.clone(), input.clone(), "owner-fixture-key").await;
        assert_eq!(status, StatusCode::OK, "{selected}");
        assert_eq!(selected["status"], "selected", "{selected}");
        let (status, _, quote) = estimation::estimate(&f, ready.clone(), "owner-fixture-key").await;
        assert_eq!(status, StatusCode::OK, "{quote}");
        assert!(quote["expires_at_ms"].as_u64().unwrap() <= record.body.expires_at_ms);
        let mut changed = input.clone();
        changed["request"]["proxy"]["profile"]["constraints"]["data_handling"][0]["value"]
            ["value"] = json!(true);
        let (_, excluded) = resolver::send(f.router.clone(), changed, "owner-fixture-key").await;
        assert_eq!(excluded["status"], "no_match", "{excluded}");
        f.harness.command("operator_verify").await;
        for assurance in ["probed", "verified"] {
            let mut changed = input.clone();
            changed["request"]["proxy"]["profile"]["constraints"]["data_handling"][0]["evidence"] =
                json!(assurance);
            changed["request"]["proxy"]["require_verified_operator"] = json!(true);
            changed["request"]["proxy"]["profile"]["providers"]["require_verified_operator"] =
                json!(true);
            let (_, excluded) =
                resolver::send(f.router.clone(), changed, "owner-fixture-key").await;
            assert_eq!(excluded["status"], "incomplete", "{excluded}");
        }
        estimation::assert_no_purchase(&mut f).await;
        let (status, id, result) = f
            .post(
                "declaration-original",
                ready.clone(),
                "owner-fixture-key",
                false,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(f.harness.backend_calls(), 1);
        if endpoint == ProxyEndpoint::Chat {
            if let Ok(path) = std::env::var("MAYHEM_TEST_DECLARATION_FIXTURE") {
                std::fs::write(path,serde_json::to_vec_pretty(&json!({"test_only":true,"publication":publication,"declaration":record,"input":input,"prepared":prepared,"selected":selected,"quote":quote})).unwrap()).unwrap();
            }
        }
        f.harness.stop_descriptors();
        let before = f.harness.status().await;
        let (status, replay_id, replay) = f
            .post("declaration-original", ready, "owner-fixture-key", false)
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(id, replay_id);
        assert_eq!(result, replay);
        assert_eq!(f.harness.backend_calls(), 1);
        assert_eq!(
            before["publications"],
            f.harness.status().await["publications"]
        );
        f.stop().await;
    }
}
#[tokio::test]
async fn data_handling_gate_checks_live_exact_evidence_before_owner_authorization() {
    let _case = estimation::ESTIMATE_CASE.lock().await;
    let registry = profile::Registry::with_definition(definition()).await;
    let mut f = Fixture::start_configured(
        ProxyEndpoint::Chat,
        ProxyRail::Fiat,
        None,
        "owner-fixture-key",
        None,
        Some(registry.config()),
        None,
    )
    .await;
    install(&f);
    let mut exact = wanted(&f)["request"].clone();
    exact["model"] = json!(model(&f.harness));
    let (status, prepared) = prepare(&f, exact).await;
    assert_eq!(status, StatusCode::OK, "{prepared}");
    let request = Arc::new(
        proxy_request::Request::parse(
            ProxyEndpoint::Chat,
            prepared["request"].clone(),
            &f.runtime.policy,
        )
        .unwrap()
        .unwrap(),
    );
    let owner = Arc::new(Owner::default());
    let gate = super::super::data_handling::gate(
        f.control.control.clone(),
        f.runtime.controller.clone(),
        request,
        owner.clone(),
    )
    .await
    .unwrap();
    let purchase = f.harness.prepare().await;
    gate.authorize(&purchase).await.unwrap();
    assert_eq!(owner.0.load(Ordering::SeqCst), 1);
    f.harness.stop_descriptors();
    assert!(gate.authorize(&purchase).await.is_err());
    assert_eq!(owner.0.load(Ordering::SeqCst), 1);
    assert_eq!(f.harness.backend_calls(), 0);
    assert_eq!(f.harness.status().await["publications"], 0);
    f.stop().await;
}
