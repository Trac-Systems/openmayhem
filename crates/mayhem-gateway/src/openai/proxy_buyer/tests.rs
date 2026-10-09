mod buyer_policy;
mod conformance;
mod contract;
mod estimation;
mod operator;
mod local_lab;
mod profile;
mod resolver;
mod taxonomy;
mod retail;
mod streaming;
use super::*;
use crate::openai::proxy_owner::tests::support::{self, ControlFixture, Harness};
use crate::openai::{
    gateway_token_hash, openai_router, GatewayAccessControl, GatewayKeyBudgetLimits,
    GatewayTokenBudgetPeriod, GatewayTokenRecord, GatewayTokenStore,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use mayhem_proto::proxy::ProxyRail;
use serde_json::json;
use tower::ServiceExt;

struct Fixture {
    harness: Harness,
    control: ControlFixture,
    state: GatewayState,
    router: axum::Router,
    runtime: Arc<Runtime>,
    stopped: watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<(), String>>,
    directory: tempfile::TempDir,
    tokens: GatewayTokenStore,
}
impl Fixture {
    async fn start() -> Self {
        Self::start_with(ProxyEndpoint::Chat, ProxyRail::Tnk).await
    }
    async fn start_with(endpoint: ProxyEndpoint, rail: ProxyRail) -> Self {
        Self::start_with_retail(endpoint, rail, None).await
    }
    async fn start_with_retail(
        endpoint: ProxyEndpoint,
        rail: ProxyRail,
        retail: Option<RetailAuthorizationConfig>,
    ) -> Self {
        Self::start_with_retail_key(endpoint, rail, retail, "owner-fixture-key").await
    }
    async fn start_with_retail_key(
        endpoint: ProxyEndpoint,
        rail: ProxyRail,
        retail: Option<RetailAuthorizationConfig>,
        owner_key: &str,
    ) -> Self {
        Self::start_with_contract(endpoint, rail, retail, owner_key, None).await
    }
    async fn start_with_contract(
        endpoint: ProxyEndpoint,
        rail: ProxyRail,
        retail: Option<RetailAuthorizationConfig>,
        owner_key: &str,
        contract: Option<mayhem_proto::EndpointFamilyContract>,
    ) -> Self {
        Self::start_with_registry(endpoint, rail, retail, owner_key, contract, None).await
    }
    async fn start_with_registry(
        endpoint: ProxyEndpoint,
        rail: ProxyRail,
        retail: Option<RetailAuthorizationConfig>,
        owner_key: &str,
        contract: Option<mayhem_proto::EndpointFamilyContract>,
        registry: Option<crate::openai::proxy_control::RegistryConfig>,
    ) -> Self {
        Self::start_configured(endpoint, rail, retail, owner_key, contract, registry, None).await
    }
    async fn start_configured(
        endpoint: ProxyEndpoint,
        rail: ProxyRail,
        retail: Option<RetailAuthorizationConfig>,
        owner_key: &str,
        contract: Option<mayhem_proto::EndpointFamilyContract>,
        registry: Option<crate::openai::proxy_control::RegistryConfig>,
        resolution_limits: Option<ProfileResolutionLimits>,
    ) -> Self {
        Self::start_evidence(
            endpoint,
            rail,
            retail,
            owner_key,
            contract,
            registry,
            resolution_limits,
            None,
        )
        .await
    }
    async fn start_evidence(
        endpoint: ProxyEndpoint,
        rail: ProxyRail,
        retail: Option<RetailAuthorizationConfig>,
        owner_key: &str,
        contract: Option<mayhem_proto::EndpointFamilyContract>,
        registry: Option<crate::openai::proxy_control::RegistryConfig>,
        resolution_limits: Option<ProfileResolutionLimits>,
        mut evidence: Option<mayhem_proxy::conformance::Config>,
    ) -> Self {
        let harness =
            Harness::start_with_contract(&support::worker_path(), endpoint, rail, contract).await;
        if let Some(config) = &mut evidence {
            config.tester = harness.buyer.identity().controller_pubkey.clone();
        }
        let control = harness.control_with_evidence(registry, evidence).await;
        let directory = support::private_dir();
        let policy = proxy_request::Policy::new(
            support::digest(1),
            Digest::new(harness.policy.digest().unwrap()).unwrap(),
            mayhem_proxy::financial::quote::Lifetimes {
                acceptance_epochs: 0,
                reservation_epochs: 19,
                receipt_grace_epochs: 2,
            },
            512 * 1024,
        )
        .unwrap();
        let mut runtime =
            Runtime::new(harness.buyer.clone(), policy, harness.policy.clone(), 2).unwrap();
        if let Some(limits) = resolution_limits {
            runtime = runtime.with_profile_resolution_limits(limits).unwrap();
        }
        if let Some(config) = retail {
            runtime = runtime.with_retail_authorization(config).unwrap();
        }
        let runtime = Arc::new(runtime);
        let model = model(&harness);
        let token = |id: &str, key: &str| GatewayTokenRecord {
            name: id.into(),
            token_id: id.into(),
            token_hash: gateway_token_hash(key),
            created_at: 1,
            expires_at: None,
            budget_au: Some(1_000_000),
            budget_period: Some(GatewayTokenBudgetPeriod::Total),
            spent_total_au: 0,
            spent_period_au: 0,
            period_started_at: Some(1),
            max_rate_per_minute: None,
            models: vec![model.clone()],
            last_used_at: None,
            revoked_at: None,
        };
        let tokens = GatewayTokenStore {
            version: 1,
            tokens: vec![
                token("owner", owner_key),
                token("other", "other-fixture-key"),
            ],
        };
        let path = directory.path().join("tokens.json");
        std::fs::write(&path, serde_json::to_vec(&tokens).unwrap()).unwrap();
        let access = GatewayAccessControl::new(true, tokens.clone(), Some(path))
            .initialize_durable_key_budget(
                directory.path().join("budget.redb"),
                GatewayKeyBudgetLimits {
                    max_tokens: 4,
                    max_reservations: 32,
                },
            )
            .unwrap();
        let state = GatewayState::fixture()
            .with_receipt_user_seed(harness.buyer_seed)
            .with_job_store_dir(directory.path().join("jobs"))
            .unwrap()
            .with_access_control(access)
            .with_proxy_control(control.control.clone())
            .with_proxy_buyer(runtime.clone())
            .unwrap();
        let (stopped, rx) = watch::channel(false);
        let owned = runtime.clone();
        let running_state = state.clone();
        let task = tokio::spawn(async move { owned.run(running_state, rx).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !runtime.running.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let router = openai_router(state.clone());
        Self {
            harness,
            control,
            state,
            router,
            runtime,
            stopped,
            task,
            directory,
            tokens,
        }
    }
    fn body(&self) -> Value {
        let offer = &self.harness.template.terms.offer;
        let mut body: Value =
            serde_json::from_slice(&support::request_body(offer.endpoint)).unwrap();
        body["model"] = json!(model(&self.harness));
        body["proxy"] = json!({"prices":{"rates":offer.rates,"per_request_au":offer.per_request_au.to_string(),
            "min_session_au":offer.min_session_au.to_string(),"max_total_spend_au":self.harness.template.terms.max_total_spend_au.to_string()},
            "rail":self.harness.template.terms.rail,"settlement_policy_hash":self.harness.policy.digest().unwrap(),
            "output_units":(offer.endpoint!=ProxyEndpoint::Decisions).then_some(37),
            "minimum_context":null,"minimum_tokens_per_second":null,"require_verified_operator":false});
        body
    }
    async fn post(
        &self,
        key: &str,
        body: Value,
        token: &str,
        asynchronous: bool,
    ) -> (StatusCode, axum::http::HeaderMap, Value) {
        let endpoint = support::endpoint_path(self.harness.adapter.endpoint());
        let mut request = Request::builder()
            .method("POST")
            .uri(endpoint)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .header("idempotency-key", key);
        if asynchronous {
            request = request.header("prefer", "respond-async");
        }
        let response = self
            .router
            .clone()
            .oneshot(
                request
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, headers, serde_json::from_slice(&bytes).unwrap())
    }
    async fn get(&self, path: &str, token: &str) -> (StatusCode, Value) {
        let response = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    async fn stop(self) {
        self.stopped.send_replace(true);
        self.task.await.unwrap().unwrap();
        assert!(!self.runtime.running.load(Ordering::Acquire));
        self.control.stop().await;
        self.harness.stop().await;
    }
}
fn model(harness: &Harness) -> String {
    let offer = &harness.template.terms.offer;
    format!(
        "proxy/offer/{}/{}/{}",
        offer.market_id,
        offer.provider_pubkey,
        offer.slot_id().unwrap()
    )
}

#[tokio::test]
async fn proxy_http_all_endpoints_require_explicit_buyer_activation() {
    let router = openai_router(GatewayState::fixture());
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let mut body: Value = serde_json::from_slice(&support::request_body(endpoint)).unwrap();
        body["model"] = json!(format!(
            "proxy/offer/{}/{}/{}",
            "1".repeat(64),
            "2".repeat(64),
            "3".repeat(64)
        ));
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(support::endpoint_path(endpoint))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{endpoint:?}"
        );
    }
}

#[tokio::test]
async fn proxy_http_paid_json_replay_owner_isolation_and_changed_controls() {
    let mut f = Fixture::start().await;
    let native_before = f
        .state
        .models_snapshot()
        .iter()
        .map(|model| model.id.clone())
        .collect::<Vec<_>>();
    assert!(!native_before.is_empty());
    let (status, headers, response) = f
        .post("paid-original", f.body(), "owner-fixture-key", false)
        .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["choices"][0]["message"]["content"], "hello");
    assert!(response["id"].as_str().unwrap().starts_with("proxy_"));
    assert_ne!(response["id"], "private-upstream-id");
    let job = headers
        .get("x-mayhem-job-id")
        .expect("durable owner job")
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(f.harness.backend_calls(), 1);
    assert_eq!(f.harness.status().await["publications"], 2);
    assert!(f
        .state
        .access_control
        .pending_key_budgets(None, 64)
        .unwrap()
        .is_empty());
    let charged = f.state.access_control.summary()["tokens"][0]["spent_total_au"]
        .as_str()
        .unwrap()
        .parse::<u128>()
        .unwrap();
    assert!(charged > 0);
    f.tokens.tokens[0].budget_au = Some(charged);
    std::fs::write(
        f.directory.path().join("tokens.json"),
        serde_json::to_vec(&f.tokens).unwrap(),
    )
    .unwrap();
    let (status, replay_headers, replay) = f
        .post("paid-original", f.body(), "owner-fixture-key", false)
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "exhausted key can read its existing result: {replay}"
    );
    assert_eq!(replay, response);
    assert_eq!(replay_headers["x-mayhem-job-id"], job);
    assert_eq!(
        f.post("paid-original", f.body(), "invalid-fixture-key", false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    for change in ["body", "price"] {
        let mut body = f.body();
        if change == "body" {
            body["messages"][0]["content"] = json!("different");
        } else {
            body["proxy"]["prices"]["max_total_spend_au"] = json!("999999");
        }
        let (status, _, _) = f
            .post("paid-original", body, "owner-fixture-key", false)
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{change}");
    }
    let (status, _) = f.get(&format!("/v1/jobs/{job}"), "other-fixture-key").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(f.harness.backend_calls(), 1);
    assert_eq!(f.harness.status().await["publications"], 2);
    assert_eq!(
        f.state
            .models_snapshot()
            .iter()
            .map(|model| model.id.clone())
            .collect::<Vec<_>>(),
        native_before
    );
    let changed = proxy_request::Policy::new(
        support::digest(2),
        Digest::new(f.harness.policy.digest().unwrap()).unwrap(),
        f.runtime.policy.lifetimes(),
        512 * 1024,
    )
    .unwrap();
    let changed = Arc::new(
        Runtime::new(
            f.harness.buyer.clone(),
            changed,
            f.harness.policy.clone(),
            2,
        )
        .unwrap(),
    );
    let changed_router = openai_router(f.state.clone().with_proxy_buyer(changed).unwrap());
    let response = changed_router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("authorization", "Bearer owner-fixture-key")
                .header("idempotency-key", "paid-original")
                .body(Body::from(serde_json::to_vec(&f.body()).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::CONFLICT,
        "operator policy revision is part of exact replay"
    );
    // Relax only model scope to reach the native spend check with the already
    // exhausted account. The proxy replay exemption must not grant native spend.
    f.tokens.tokens[0].models.clear();
    std::fs::write(
        f.directory.path().join("tokens.json"),
        serde_json::to_vec(&f.tokens).unwrap(),
    )
    .unwrap();
    let native: Value = serde_json::from_slice(&support::chat()).unwrap();
    assert_eq!(
        f.post("native-over-cap", native, "owner-fixture-key", false)
            .await
            .0,
        StatusCode::PAYMENT_REQUIRED
    );
    f.tokens.tokens[0].revoked_at = Some(now_secs());
    std::fs::write(
        f.directory.path().join("tokens.json"),
        serde_json::to_vec(&f.tokens).unwrap(),
    )
    .unwrap();
    assert_eq!(
        f.post("paid-original", f.body(), "owner-fixture-key", false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(f.harness.backend_calls(), 1);
    f.stop().await;
}

#[tokio::test]
async fn proxy_http_new_endpoint_schemas_reject_invalid_input_before_authorization() {
    for endpoint in [ProxyEndpoint::Completions, ProxyEndpoint::Responses] {
        let mut f = Fixture::start_with(endpoint, ProxyRail::Tnk).await;
        let mut invalid = f.body();
        if endpoint == ProxyEndpoint::Completions {
            invalid["prompt"] = json!({"not":"a completion prompt"});
        } else {
            invalid["input"] = json!({"not":"a response input"});
        }
        let mut requests = vec![("invalid-schema", invalid)];
        let unsupported = if endpoint == ProxyEndpoint::Completions {
            vec![("suffix", json!("end"))]
        } else {
            vec![
                ("instructions", json!("reply briefly")),
                ("store", json!(true)),
            ]
        };
        for (field, value) in unsupported {
            let mut body = f.body();
            body[field] = value;
            requests.push((field, body));
        }
        if endpoint == ProxyEndpoint::Responses {
            let mut body = f.body();
            body["store"] = json!(false);
            requests.push(("store-false", body));
        }
        for (key, invalid) in requests {
            let (status, headers, response) =
                f.post(key, invalid, "owner-fixture-key", false).await;
            assert!(
                status.is_client_error() || status.is_server_error(),
                "{endpoint:?}/{key}: {status} {response}"
            );
            let job = f
                .state
                .jobs
                .lock()
                .unwrap()
                .get(headers["x-mayhem-job-id"].to_str().unwrap(), now_secs())
                .unwrap()
                .unwrap();
            assert!(job.proxy.as_ref().unwrap().terms().is_none());
            assert!(job.result.is_none());
        }
        assert!(f
            .state
            .access_control
            .pending_key_budgets(None, 64)
            .unwrap()
            .is_empty());
        assert_eq!(
            f.state.access_control.summary()["tokens"][0]["spent_total_au"],
            "0"
        );
        assert_eq!(f.harness.backend_calls(), 0);
        assert_eq!(f.harness.status().await["publications"], 0);
        f.stop().await;
    }
}

#[tokio::test]
async fn proxy_http_all_four_endpoints_pay_exact_original_once_on_all_three_rails() {
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
            let mut f = Fixture::start_with(endpoint, rail).await;
            if endpoint == ProxyEndpoint::Decisions {
                let mut stream = f.body();
                stream["stream"] = json!(true);
                let (status, _, _) = f
                    .post(
                        "decisions-stream-rejected",
                        stream,
                        "owner-fixture-key",
                        false,
                    )
                    .await;
                assert_eq!(status, StatusCode::BAD_REQUEST);
                assert_eq!(f.harness.status().await["publications"], 0);
            }

            let (status, headers, response) = f
                .post(
                    "all-endpoints-original",
                    f.body(),
                    "owner-fixture-key",
                    false,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{endpoint:?}/{rail:?}: {response}");
            match endpoint {
                ProxyEndpoint::Chat => {
                    assert_eq!(response["choices"][0]["message"]["content"], "hello")
                }
                ProxyEndpoint::Completions => assert_eq!(response["choices"][0]["text"], "hello"),
                ProxyEndpoint::Responses => {
                    assert_eq!(response["status"], "completed");
                    assert_eq!(response["output"][0]["content"][0]["text"], "hello");
                }
                ProxyEndpoint::Decisions => assert_eq!(response["answers"]["q"]["noul"], 0.7),
            }
            assert!(response["id"].as_str().unwrap().starts_with("proxy_"));
            let id = headers["x-mayhem-job-id"].to_str().unwrap();
            let job = f
                .state
                .jobs
                .lock()
                .unwrap()
                .get(id, now_secs())
                .unwrap()
                .unwrap();
            let proxy = job.proxy.as_ref().unwrap();
            let terms = proxy.terms().unwrap();
            assert_eq!(terms.rail, rail);
            assert_eq!(terms.offer.endpoint, endpoint);
            assert_eq!(terms.offer.rates, f.harness.template.terms.offer.rates);
            let paid_receipt = match &proxy.closure().unwrap().outcome {
                mayhem_proxy::financial::recovery::FinancialOutcome::Paid { receipt } => receipt,
                _ => panic!("canonical paid closure required"),
            };
            let paid = paid_receipt.body.au_owed_cum;
            assert_eq!(paid, terms.offer.cost(&paid_receipt.body.usage).unwrap());
            let evidence_path = format!("/v1/jobs/{id}/proxy-evidence");
            let (evidence_status, evidence) = f.get(&evidence_path, "owner-fixture-key").await;
            assert_eq!(evidence_status, StatusCode::OK);
            assert_eq!(evidence["object"], "mayhem.proxy.job_evidence");
            assert_eq!(evidence["id"], id);
            assert_eq!(evidence["model"], model(&f.harness));
            assert_eq!(
                evidence["endpoint_family"],
                serde_json::to_value(endpoint).unwrap()
            );
            assert_eq!(evidence["terms"], serde_json::to_value(terms).unwrap());
            assert_eq!(evidence["financial"]["kind"], "canonical");
            assert_eq!(evidence["financial"]["outcome"]["kind"], "paid");
            assert_eq!(
                evidence["financial"]["outcome"]["receipt"],
                serde_json::to_value(paid_receipt).unwrap()
            );
            assert_eq!(evidence["financial"]["budget_settled"], true);
            assert_eq!(evidence["terms_hash"], terms.digest().unwrap());
            assert!(evidence.get("result").is_none());
            assert_eq!(
                f.get(&evidence_path, "other-fixture-key").await.0,
                StatusCode::NOT_FOUND
            );
            // Explicit opt-in ABI fixtures for the retail decoder. These are
            // local synthetic purchases, not production accounts or prompts.
            if let Some(directory) = std::env::var_os("MAYHEM_TEST_PROXY_EVIDENCE_DIR") {
                let directory = std::path::PathBuf::from(directory);
                std::fs::create_dir_all(&directory).unwrap();
                let endpoint_name = serde_json::to_value(endpoint).unwrap();
                let rail_name = serde_json::to_value(rail).unwrap();
                std::fs::write(
                    directory.join(format!(
                        "{}_{}.json",
                        endpoint_name.as_str().unwrap(),
                        rail_name.as_str().unwrap()
                    )),
                    serde_json::to_vec_pretty(&evidence).unwrap(),
                )
                .unwrap();
            }
            if endpoint != ProxyEndpoint::Decisions {
                assert_eq!(terms.max_usage["output_token"], 37);
                // Visible "hello" is two normalized billing units, regardless of
                // the upstream fixture's claimed completion/output token count 1.
                assert_eq!(paid_receipt.body.usage["output_token"], 2);
            } else {
                assert_eq!(paid_receipt.body.usage["decision"], 1);
            }
            assert!(paid > 0);
            assert_eq!(
                f.state.access_control.summary()["tokens"][0]["spent_total_au"],
                paid.to_string()
            );
            // Every route must let its owner read an already paid result even
            // after the shared key cap is exhausted, without authorizing new work.
            f.tokens.tokens[0].budget_au = Some(paid);
            std::fs::write(
                f.directory.path().join("tokens.json"),
                serde_json::to_vec(&f.tokens).unwrap(),
            )
            .unwrap();
            let (status, replay_headers, replay) = f
                .post(
                    "all-endpoints-original",
                    f.body(),
                    "owner-fixture-key",
                    false,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{endpoint:?}/{rail:?}: {replay}");
            assert_eq!(response, replay);
            assert_eq!(replay_headers["x-mayhem-job-id"], id);
            assert_eq!(
                f.get(&evidence_path, "owner-fixture-key").await,
                (StatusCode::OK, evidence)
            );
            for change in ["body", "price"] {
                let mut changed = f.body();
                if change == "price" {
                    changed["proxy"]["prices"]["max_total_spend_au"] = json!("999999");
                } else {
                    match endpoint {
                        ProxyEndpoint::Chat => {
                            changed["messages"][0]["content"] = json!("different")
                        }
                        ProxyEndpoint::Completions => changed["prompt"] = json!("different"),
                        ProxyEndpoint::Responses => changed["input"] = json!("different"),
                        ProxyEndpoint::Decisions => changed["state"] = json!("different"),
                    }
                }
                assert_eq!(
                    f.post(
                        "all-endpoints-original",
                        changed,
                        "owner-fixture-key",
                        false
                    )
                    .await
                    .0,
                    StatusCode::CONFLICT,
                    "{endpoint:?}/{rail:?}/{change}"
                );
            }
            assert_eq!(
                f.get(&format!("/v1/jobs/{id}"), "other-fixture-key")
                    .await
                    .0,
                StatusCode::NOT_FOUND
            );
            f.tokens.tokens[0].models.clear();
            std::fs::write(
                f.directory.path().join("tokens.json"),
                serde_json::to_vec(&f.tokens).unwrap(),
            )
            .unwrap();
            let native = serde_json::from_slice(&support::request_body(endpoint)).unwrap();
            assert_eq!(
                f.post("native-over-cap", native, "owner-fixture-key", false)
                    .await
                    .0,
                StatusCode::PAYMENT_REQUIRED,
                "{endpoint:?}/{rail:?} native cap"
            );
            f.tokens.tokens[0].revoked_at = Some(now_secs());
            std::fs::write(
                f.directory.path().join("tokens.json"),
                serde_json::to_vec(&f.tokens).unwrap(),
            )
            .unwrap();
            assert_eq!(
                f.get(&evidence_path, "owner-fixture-key").await.0,
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                f.post(
                    "all-endpoints-original",
                    f.body(),
                    "owner-fixture-key",
                    false
                )
                .await
                .0,
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(f.harness.backend_calls(), 1);
            assert_eq!(f.harness.status().await["publications"], 2);
            f.stop().await;
        }
    }
}

#[tokio::test]
async fn proxy_http_async_owner_completes_without_http_wait_and_streaming_async_rejected() {
    let mut f = Fixture::start().await;
    let mut stream = f.body();
    stream["stream"] = json!(true);
    let (status, _, _) = f
        .post("stream-rejected", stream, "owner-fixture-key", true)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(f.harness.backend_calls(), 0);
    assert_eq!(f.harness.status().await["publications"], 0);
    let (status, headers, body) = f
        .post("async-original", f.body(), "owner-fixture-key", true)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let job = headers
        .get("x-mayhem-job-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (status, _, body) = f
                .post("async-original", f.body(), "owner-fixture-key", false)
                .await;
            if status == StatusCode::OK {
                assert_eq!(body["choices"][0]["message"]["content"], "hello");
                break;
            }
            assert_eq!(status, StatusCode::ACCEPTED, "{body}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.harness.backend_calls(), 1);
    assert_eq!(f.harness.status().await["publications"], 2);
    assert_eq!(
        f.get(&format!("/v1/jobs/{job}"), "other-fixture-key")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let disabled = openai_router(GatewayState::fixture());
    let response = disabled
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&f.body()).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(f.harness.backend_calls(), 1);
    f.stop().await;
}

#[tokio::test]
async fn proxy_http_pending_publication_recovers_original_hold_without_inventing_execute() {
    let mut f = Fixture::start().await;
    f.harness.command("publish_pending").await;
    let (status, headers, body) = f
        .post("pending-original", f.body(), "owner-fixture-key", true)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let id = headers["x-mayhem-job-id"].to_str().unwrap().to_owned();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if f.harness.status().await["submissions"].as_u64().unwrap() > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let job = f
        .state
        .jobs
        .lock()
        .unwrap()
        .get(&id, now_secs())
        .unwrap()
        .unwrap();
    let terms = job.proxy.as_ref().unwrap().terms().unwrap().clone();
    assert!(job.result.is_none());
    let (evidence_status, evidence) = f
        .get(
            &format!("/v1/jobs/{id}/proxy-evidence"),
            "owner-fixture-key",
        )
        .await;
    assert_eq!(evidence_status, StatusCode::OK);
    assert_eq!(evidence["financial"], json!({"kind":"pending"}));
    assert_eq!(evidence["terms_hash"], terms.digest().unwrap());
    assert_eq!(evidence["result_verified"], false);
    assert_eq!(f.harness.backend_calls(), 0);
    assert_eq!(f.harness.status().await["publications"], 0);
    assert_eq!(
        f.state
            .access_control
            .pending_key_budgets(None, 64)
            .unwrap()[0]
            .1
            .maximum,
        terms.max_spend_au
    );
    f.harness.command("flush_publication").await;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let job = f
                .state
                .jobs
                .lock()
                .unwrap()
                .get(&id, now_secs())
                .unwrap()
                .unwrap();
            if job.proxy.as_ref().unwrap().authorization().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let (status, _, replay) = f
        .post("pending-original", f.body(), "owner-fixture-key", false)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{replay}");
    let job = f
        .state
        .jobs
        .lock()
        .unwrap()
        .get(&id, now_secs())
        .unwrap()
        .unwrap();
    assert_eq!(job.proxy.as_ref().unwrap().terms(), Some(&terms));
    assert!(job.result.is_none());
    assert_eq!(
        f.harness.backend_calls(),
        0,
        "recovery never dispatches an uncertain original request"
    );
    assert_eq!(
        f.harness.status().await["publications"],
        1,
        "original reservation only"
    );
    assert_eq!(
        f.state
            .access_control
            .pending_key_budgets(None, 64)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.state.access_control.summary()["tokens"][0]["spent_total_au"],
        "0"
    );
    f.stop().await;
}
