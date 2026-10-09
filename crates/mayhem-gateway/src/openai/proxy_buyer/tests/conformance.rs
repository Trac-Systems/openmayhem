use super::*;
use mayhem_proxy::{
    conformance::{Assertion, Class, Config, Lookup, Mapping, Tokenizer},
    registry::Definition,
};
use std::{os::unix::fs::PermissionsExt, sync::atomic::Ordering};

#[derive(Default)]
struct CountingOwner(std::sync::atomic::AtomicUsize);
impl buyer_controller::AuthorizationGate for CountingOwner {
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

fn definition() -> Definition {
    serde_json::from_value(json!({"schema_version":1,"field_id":"fixture.endpoint_valid","schema_revision":1,
    "labels":{"en":"Observed endpoint output"},"help":{},"units":null,"group":"protocol","order":0,
    "endpoints":["openai_chat_completions"],"value_schema":{"type":"boolean"},"default":null,"operators":["eq"],
    "minimum_assurance":"probed","max_evidence_age_ms":60000,"usage":{"kind":"filter_only"},"rules":[]})).unwrap()
}
fn config(def: Option<&Definition>) -> Config {
    Config {
        schema_version: 1,
        tester: support::digest(1),
        ttl_ms: 60000,
        maximum_records: 16,
        maximum_bytes: 16 * 16384,
        minimum_interval_tokens: 2,
        minimum_interval_us: 1000,
        mappings: def
            .map(|d| {
                vec![Mapping {
                    field_id: d.field_id.clone(),
                    schema_revision: d.schema_revision,
                    definition_digest: Digest::new(d.digest().unwrap()).unwrap(),
                    assertion: Assertion::ValidEndpointOutput,
                }]
            })
            .unwrap_or_default(),
        tokenizer: None,
    }
}
async fn evidence_fixture(
    registry: Option<crate::openai::proxy_control::RegistryConfig>,
    c: Config,
) -> Fixture {
    Fixture::start_evidence(
        ProxyEndpoint::Chat,
        ProxyRail::Fiat,
        None,
        "owner-fixture-key",
        None,
        registry,
        None,
        Some(c),
    )
    .await
}
fn predicate_body(f: &Fixture) -> Value {
    let mut request = profile::body(f);
    request["messages"] = f.body()["messages"].clone();
    request["proxy"]["profile"]["constraints"]["request_controls"] = json!([]);
    request["proxy"]["profile"]["constraints"]["capabilities"] = json!([{"field_id":"fixture.endpoint_valid","schema_revision":1,
        "operator":"eq","value":{"type":"boolean","value":true},"evidence":"probed","max_age_ms":60000}]);
    request
}
fn record(f: &Fixture, body: &Value) -> mayhem_proxy::conformance::Signed {
    let catalog = f.control.control.catalog().read().unwrap();
    let published = catalog
        .proxy_offer(
            model(&f.harness).strip_prefix("proxy/offer/").unwrap(),
            super::super::super::now_millis_u64(),
        )
        .unwrap()
        .unwrap();
    let subject = super::super::evidence::subject(&published).unwrap();
    let Lookup::Present(record) = f
        .control
        .control
        .conformance()
        .unwrap()
        .lookup(&subject, &Class::request(body).unwrap())
        .unwrap()
    else {
        panic!("actual completion did not produce evidence")
    };
    *record
}
#[tokio::test]
async fn conformance_actual_completion_prepare_quote_execution_and_replay_preserve_authority() {
    let _case = estimation::ESTIMATE_CASE.lock().await;
    let def = definition();
    let registry = profile::Registry::with_definition(def.clone()).await;
    let mut f = evidence_fixture(Some(registry.config()), config(Some(&def))).await;
    let wanted = predicate_body(&f);
    let (status, missing) = profile::prepare(&f, wanted.clone(), "owner-fixture-key").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{missing}");
    assert_eq!(f.harness.backend_calls(), 0);
    let (status, _, initial) = f
        .post("evidence-origin", f.body(), "owner-fixture-key", false)
        .await;
    assert_eq!(status, StatusCode::OK, "{initial}");
    let observed = record(&f, &f.body());
    assert_eq!(
        observed.body.provenance,
        mayhem_proxy::conformance::Provenance::GatewayObservation
    );
    observed.verify().unwrap();
    assert!(observed.body.speed.is_none());
    let (status, prepared) = profile::prepare(&f, wanted, "owner-fixture-key").await;
    assert_eq!(status, StatusCode::OK, "{prepared}");
    let ready = prepared["request"].clone();
    let owner = Arc::new(CountingOwner::default());
    let parsed = Arc::new(
        proxy_request::Request::parse(ProxyEndpoint::Chat, ready.clone(), &f.runtime.policy)
            .unwrap()
            .unwrap(),
    );
    let gate = super::super::evidence::gate(f.control.control.clone(), parsed, owner.clone())
        .await
        .unwrap();
    let valid = f.harness.prepare().await;
    gate.authorize(&valid).await.unwrap();
    let altered = f.harness.prepare_connection(support::digest(999)).await;
    assert!(gate.authorize(&altered).await.is_err());
    assert_eq!(
        owner.0.load(Ordering::SeqCst),
        1,
        "changed connection must be refused before owner authorization"
    );
    let (status, _, quote) = estimation::estimate(&f, ready.clone(), "owner-fixture-key").await;
    assert_eq!(status, StatusCode::OK, "{quote}");
    assert!(quote["expires_at_ms"].as_u64().unwrap() <= observed.body.expires_at_ms);
    let (status, id, result) = f
        .post(
            "evidence-constrained",
            ready.clone(),
            "owner-fixture-key",
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(f.harness.backend_calls(), 2);
    let final_record = record(&f, &ready);
    let mut changed = ready.clone();
    changed["temperature"] = json!(0.2);
    let (status, _, unknown) = f
        .post("evidence-new-class", changed, "owner-fixture-key", false)
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{unknown}");
    assert_eq!(f.harness.backend_calls(), 2);
    let mut verified = ready.clone();
    verified["proxy"]["profile"]["constraints"]["capabilities"][0]["evidence"] = json!("verified");
    assert_eq!(
        estimation::estimate(&f, verified, "owner-fixture-key")
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let (status, policy) = f.get("/v1/proxy/conformance", "owner-fixture-key").await;
    assert_eq!(status, StatusCode::OK, "{policy}");
    assert_eq!(policy["maximum_assurance"], "probed");
    assert_eq!(policy["authorizes_execution"], false);
    assert!(!serde_json::to_string(&policy)
        .unwrap()
        .contains("tokenizer.json"));
    f.control.pause().await;
    let (status, replayed, replay) = f
        .post(
            "evidence-constrained",
            ready.clone(),
            "owner-fixture-key",
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(id, replayed);
    assert_eq!(result, replay);
    assert_eq!(
        record(&f, &ready).digest().unwrap(),
        final_record.digest().unwrap()
    );
    assert_eq!(f.harness.backend_calls(), 2);
    assert_eq!(
        f.state
            .access_control
            .pending_key_budgets(None, 64)
            .unwrap()
            .len(),
        0
    );
    if let Some(path) = std::env::var_os("MAYHEM_TEST_CONFORMANCE_FIXTURE") {
        let value = json!({"test_only":true,"transport":"bounded SC-Bridge protocol double and local signed canonical fixture", "policy":policy,"observation":observed,"preparation":prepared,"estimate":quote});
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    f.stop().await;
}
#[tokio::test]
async fn conformance_stream_speed_uses_actual_local_tokens_and_complete_scope() {
    let _case = estimation::ESTIMATE_CASE.lock().await;
    let directory = support::private_dir();
    let path = directory.path().join("tokenizer.json");
    let bytes=serde_json::to_vec(&json!({"version":"1.0","truncation":null,"padding":null,"added_tokens":[],"normalizer":null,
        "pre_tokenizer":{"type":"WhitespaceSplit"},"post_processor":null,"decoder":null,
        "model":{"type":"WordLevel","vocab":{"[UNK]":0,"one":1,"two":2},"unk_token":"[UNK]"}})).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut c = config(None);
    c.tokenizer = Some(Tokenizer {
        file: path,
        digest: Digest::new(blake3::hash(&bytes).to_hex().as_str()).unwrap(),
        limits: mayhem_proxy::health::native::Limits {
            artifact_bytes: 4096,
            output_bytes: 65536,
            channels: 8,
            workers: 2,
            minimum_tokens: 2,
        },
    });
    let f = evidence_fixture(None, c).await;
    f.harness.stream_backend.pieces.store(4, Ordering::SeqCst);
    f.harness.stream_backend.words.store(true, Ordering::SeqCst);
    f.harness
        .stream_backend
        .delay_ms
        .store(20, Ordering::SeqCst);
    let mut request = f.body();
    request["stream"] = json!(true);
    let response = f.stream_post_body("speed-origin", request.clone()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = streaming::collect(response.into_body()).await;
    assert!(
        !String::from_utf8_lossy(&bytes).contains("\"error\""),
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let observed = record(&f, &request);
    let speed = observed
        .body
        .speed
        .as_ref()
        .expect("actual separate output intervals");
    assert_eq!(speed.interval_tokens, 6);
    assert!(speed.interval_us >= 1000);
    assert_eq!(speed.remote_cache, "unknown");
    assert_eq!(speed.local_concurrency, 1);
    let mut start = resolver::start(&f);
    start["request"]["stream"] = json!(true);
    start["request"]["proxy"]["profile"]["ranking"] = json!("preferred_speed");
    let (status, selected) =
        resolver::send(f.router.clone(), start.clone(), "owner-fixture-key").await;
    assert_eq!(status, StatusCode::OK, "{selected}");
    assert_eq!(selected["status"], "selected", "{selected}");
    assert_eq!(
        selected["ranking_basis"],
        "locally_tokenized_generation_rate"
    );
    assert_eq!(
        selected["ranking_claim"],
        "highest_observed_comparable_rate"
    );
    assert_eq!(selected["scope_exhausted"], true);
    if let Some(path) = std::env::var_os("MAYHEM_TEST_CONFORMANCE_SPEED_FIXTURE") {
        let value = json!({"schema_version":1,"test_only":true,"request":start,"response":selected,"observation":observed});
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let (status, _, estimate) = estimation::estimate(
        &f,
        selected["selection"]["request"].clone(),
        "owner-fixture-key",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{estimate}");
    assert_eq!(f.harness.backend_calls(), 1);
    start["request"]["temperature"] = json!(0.2);
    let (status, unknown) =
        resolver::send(f.router.clone(), start.clone(), "owner-fixture-key").await;
    assert_eq!(status, StatusCode::OK, "{unknown}");
    assert_eq!(unknown["status"], "incomplete", "{unknown}");
    assert!(unknown["selection"].is_null());
    start["request"]["stream"] = json!(false);
    assert_eq!(
        resolver::send(f.router.clone(), start, "owner-fixture-key")
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    // Withhold the HTTP consumer until the bounded normalized-event queue fills.
    // The new completed result remains capability evidence but cannot rank speed.
    f.harness.stream_backend.pieces.store(12, Ordering::SeqCst);
    f.harness.stream_backend.emitted.store(0, Ordering::SeqCst);
    let response = f
        .stream_post_body("speed-backpressured", request.clone())
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.harness.stream_backend.emitted.load(Ordering::SeqCst) < 13 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let bytes = streaming::collect(response.into_body()).await;
    assert!(!String::from_utf8_lossy(&bytes).contains("\"error\""));
    assert!(
        record(&f, &request).body.speed.is_none(),
        "consumer backpressure cannot establish comparative generation rate"
    );
    f.stop().await;
}
