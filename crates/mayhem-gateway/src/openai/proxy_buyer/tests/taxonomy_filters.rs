//! Byte-identical responses exported from actual SITE admin publication/HTTP/PG.
use super::*;
use axum::{extract::State, routing::any};
use std::sync::atomic::{AtomicUsize, Ordering};
fn fixture() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../mayhem-proxy/tests/fixtures/taxonomy-routing-v1.json"
    )))
    .unwrap()
}
fn hash(domain: &str, value: &Value) -> String {
    let mut bytes = domain.as_bytes().to_vec();
    bytes.push(0);
    bytes.extend(mayhem_proto::stable_json_bytes(value).unwrap());
    blake3::hash(&bytes).to_hex().to_string()
}
struct Metadata {
    origin: String,
    mode: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Metadata {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Metadata {
    async fn start() -> Self {
        let mode = Arc::new(AtomicUsize::new(0));
        async fn serve(State(mode): State<Arc<AtomicUsize>>, request: Request<Body>) -> Response {
            if mode.load(Ordering::SeqCst) == 1 {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            let path = request.uri().path().to_owned();
            let data = fixture();
            let id = path.split('/').nth(5).unwrap();
            let (release, answer) = if id == data["original"]["release_id"].as_str().unwrap() {
                (&data["original"], &data["selected"])
            } else if id == data["updated"]["release_id"].as_str().unwrap() {
                (&data["updated"], &data["excluded"])
            } else {
                return StatusCode::NOT_FOUND.into_response();
            };
            let (mut result, kind, selector) = if path.ends_with("/selection") {
                let bytes = to_bytes(request.into_body(), 65536).await.unwrap();
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                let mut expected = data["request"].clone();
                expected["release_hash"] = release["release_hash"].clone();
                assert_eq!(body, expected);
                (
                    answer.clone(),
                    "selection",
                    json!({"variants":body["variants"],"tags":body["tags"],
                    "models":body["models"].as_array().unwrap().iter().map(|m|hash("mayhem/proxy/taxonomy-match-model/v1",m)).collect::<Vec<_>>()}),
                )
            } else {
                (release.clone(), "release", Value::Null)
            };
            if mode.load(Ordering::SeqCst) == 2 && kind == "selection" {
                result["matches"][0]["model"]["model_id"] = json!("substituted");
            }
            let tag = format!(
                "\"{}\"",
                hash(
                    "mayhem/proxy/taxonomy-representation/v1",
                    &json!({"release_id":release["release_id"],"release_hash":release["release_hash"],"kind":kind,"selector":selector})
                )
            );
            let mut response = Json(result).into_response();
            response.headers_mut().insert("etag", tag.parse().unwrap());
            response
        }
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", socket.local_addr().unwrap());
        let router = axum::Router::new()
            .route("/{*path}", any(serve))
            .with_state(mode.clone());
        let task = tokio::spawn(async move { axum::serve(socket, router).await.unwrap() });
        Self { origin, mode, task }
    }
    fn config(&self) -> crate::openai::proxy_control::RegistryConfig {
        crate::openai::proxy_control::RegistryConfig {
            origin: self.origin.clone(),
            allow_loopback_http: true,
        }
    }
}
fn wanted(f: &Fixture) -> Value {
    let mut input = resolver::start(f);
    input["request"]["proxy"]["profile"]["taxonomy_filters"] = fixture()["filters"].clone();
    input["retail_ranking"] = json!({"margin_bps":0,"max_cost_micro":"1000000"});
    input
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
async fn versioned_taxonomy_filters_bind_four_endpoint_quotes_holds_and_original_replay() {
    let _case = estimation::ESTIMATE_CASE.lock().await;
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let metadata = Metadata::start().await;
        let mut f = Fixture::start_configured(
            endpoint,
            ProxyRail::Fiat,
            None,
            "owner-fixture-key",
            None,
            Some(metadata.config()),
            None,
        )
        .await;
        let input = wanted(&f);
        metadata.mode.store(1, Ordering::SeqCst);
        let (_, unknown) =
            resolver::send(f.router.clone(), input.clone(), "owner-fixture-key").await;
        assert_eq!(unknown["status"], "incomplete", "{unknown}");
        metadata.mode.store(0, Ordering::SeqCst);
        let (status, selected) =
            resolver::send(f.router.clone(), input.clone(), "owner-fixture-key").await;
        assert_eq!(status, StatusCode::OK, "{selected}");
        assert_eq!(selected["status"], "selected", "{selected}");
        let body = selected["selection"]["request"].clone();
        assert_eq!(
            body["proxy"]["profile"]["taxonomy_filters"],
            fixture()["filters"]
        );
        let (status, _, quote) = estimation::estimate(&f, body.clone(), "owner-fixture-key").await;
        assert_eq!(status, StatusCode::OK, "{quote}");
        let mut revised = input.clone();
        revised["request"]["proxy"]["profile"]["taxonomy_filters"]["release_id"] =
            fixture()["updated"]["release_id"].clone();
        revised["request"]["proxy"]["profile"]["taxonomy_filters"]["release_hash"] =
            fixture()["updated"]["release_hash"].clone();
        let (_, excluded) = resolver::send(f.router.clone(), revised, "owner-fixture-key").await;
        assert_eq!(excluded["status"], "no_match", "{excluded}");
        let mut legacy = input.clone();
        legacy["request"]["proxy"]["profile"]["target"]["tags"] = json!(["gateway_filter_tag"]);
        let (legacy_code, legacy_answer) =
            resolver::send(f.router.clone(), legacy, "owner-fixture-key").await;
        assert!(
            legacy_code != StatusCode::OK || legacy_answer["status"] != "selected",
            "{legacy_answer}"
        );
        metadata.mode.store(2, Ordering::SeqCst);
        let (_, forged) =
            resolver::send(f.router.clone(), input.clone(), "owner-fixture-key").await;
        assert_eq!(forged["status"], "incomplete", "{forged}");
        metadata.mode.store(0, Ordering::SeqCst);
        estimation::assert_no_purchase(&mut f).await;
        if endpoint == ProxyEndpoint::Chat {
            let request = Arc::new(
                proxy_request::Request::parse(endpoint, body.clone(), &f.runtime.policy)
                    .unwrap()
                    .unwrap(),
            );
            let owner = Arc::new(Owner::default());
            let gate = super::super::taxonomy_filters::gate(
                f.control.control.clone(),
                request,
                owner.clone(),
            )
            .await
            .unwrap();
            let purchase = f.harness.prepare().await;
            gate.authorize(&purchase).await.unwrap();
            assert_eq!(owner.0.load(Ordering::SeqCst), 1);
            metadata.mode.store(1, Ordering::SeqCst);
            assert!(gate.authorize(&purchase).await.is_err());
            assert_eq!(owner.0.load(Ordering::SeqCst), 1);
            metadata.mode.store(0, Ordering::SeqCst);
            if let Ok(path) = std::env::var("MAYHEM_TEST_TAXONOMY_FILTER_FIXTURE") {
                let publication = f
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
                std::fs::write(path,serde_json::to_vec_pretty(&json!({"test_only":true,"input":input,"selected":selected,"quote":quote,"publication":publication})).unwrap()).unwrap();
            }
        }
        let (status, id, result) = f
            .post(
                "taxonomy-filter-original",
                body.clone(),
                "owner-fixture-key",
                false,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(f.harness.backend_calls(), 1);
        metadata.mode.store(1, Ordering::SeqCst);
        let before = f.harness.status().await;
        let (status, replay_id, replay) = f
            .post(
                "taxonomy-filter-original",
                body.clone(),
                "owner-fixture-key",
                false,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(id, replay_id);
        assert_eq!(result, replay);
        let (status, _, _) = f
            .post("taxonomy-filter-new", body, "owner-fixture-key", false)
            .await;
        assert_ne!(status, StatusCode::OK);
        assert_eq!(f.harness.backend_calls(), 1);
        assert_eq!(
            before["publications"],
            f.harness.status().await["publications"]
        );
        f.stop().await;
    }
}
