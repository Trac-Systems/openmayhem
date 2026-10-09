use super::*;
use axum::{extract::State, routing::any};
use mayhem_proxy::registry::{
    publication::{Document, DocumentReference, Manifest, Release},
    Definition,
};
use std::collections::BTreeMap;

fn canonical_hash(domain: &str, value: &Value) -> String {
    let mut bytes = domain.as_bytes().to_vec();
    bytes.push(0);
    bytes.extend(mayhem_proto::stable_json_bytes(value).unwrap());
    blake3::hash(&bytes).to_hex().to_string()
}
fn temperature(revision: u32) -> Definition {
    serde_json::from_value(json!({"schema_version":1,"field_id":"fixture.temperature","schema_revision":revision,
        "labels":{"en":"Fixture temperature 🧠"},"help":{},"units":null,"group":"controls","order":0,
        "endpoints":["openai_chat_completions"],"value_schema":{"type":"decimal","minimum":"0","maximum":if revision==1{"1"}else{"0.1"}},
        "default":{"type":"decimal","value":"0.1"},"operators":["eq","gte","lte"],"minimum_assurance":"declared",
        "max_evidence_age_ms":null,"usage":{"kind":"request_control","request_path":"temperature"},"rules":[]})).unwrap()
}
fn publication(previous: Option<&Release>, definition: Definition) -> (Release, Document) {
    let version = definition.schema_revision;
    let document: Document = serde_json::from_value(
        json!({"field_id":definition.field_id,"schema_revision":version,
        "version":version,"definition_hash":definition.digest().unwrap(),"definition":definition}),
    )
    .unwrap();
    let manifest = Manifest {
        schema_version: 1,
        parent_release_id: previous.map(|p| p.release_id.clone()),
        parent_release_hash: previous.map(|p| p.release_hash.clone()),
        changes: vec![DocumentReference {
            field_id: document.field_id.clone(),
            schema_revision: version,
            version,
            definition_hash: document.definition_hash.clone(),
        }],
    };
    let release = Release {
        object: "proxy.registry_release".into(),
        release_id: format!("00000000-0000-4000-8000-{version:012x}"),
        revision: version.to_string(),
        release_hash: manifest.digest().unwrap(),
        manifest,
        published_at: "2026-10-09T15:47:47.722Z".into(),
        publication_state: "published".into(),
    };
    (release, document)
}
struct RegistryState {
    head: Release,
    releases: BTreeMap<String, (Release, Document)>,
    reads: usize,
    unavailable: bool,
}
struct Registry {
    origin: String,
    state: Arc<Mutex<RegistryState>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Registry {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Registry {
    async fn start() -> Self {
        let (release, document) = publication(None, temperature(1));
        let state = Arc::new(Mutex::new(RegistryState {
            head: release.clone(),
            releases: BTreeMap::from([(release.release_id.clone(), (release, document))]),
            reads: 0,
            unavailable: false,
        }));
        async fn respond(
            State(state): State<Arc<Mutex<RegistryState>>>,
            request: Request<Body>,
        ) -> Response {
            let path = request.uri().path().to_owned();
            let lookup = path.ends_with("/lookup");
            let body = to_bytes(request.into_body(), 32 * 1024).await.unwrap();
            let refs = if lookup {
                serde_json::from_slice::<Value>(&body).unwrap()["references"].clone()
            } else {
                Value::Null
            };
            let mut state = state.lock().unwrap();
            state.reads += 1;
            if state.unavailable {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            let id = path.strip_prefix("/v1/proxy/registry/releases/").unwrap();
            let release = if id == "current" {
                state.head.clone()
            } else {
                let Some((r, _)) = state.releases.get(id.strip_suffix("/lookup").unwrap_or(id))
                else {
                    return StatusCode::NOT_FOUND.into_response();
                };
                r.clone()
            };
            let value = if lookup {
                let mut docs = vec![];
                for reference in refs.as_array().unwrap() {
                    let found = state
                        .releases
                        .values()
                        .filter(|(_, d)| {
                            d.field_id == reference["field_id"].as_str().unwrap()
                                && d.schema_revision
                                    == reference["schema_revision"].as_u64().unwrap() as u32
                        })
                        .next();
                    let Some((_, document)) = found else {
                        return StatusCode::NOT_FOUND.into_response();
                    };
                    docs.push(json!(document));
                }
                json!({"object":"proxy.registry_lookup","release_id":release.release_id,"release_hash":release.release_hash,"definitions":docs})
            } else {
                json!(release)
            };
            let selector = if lookup {
                json!(refs
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| canonical_hash("mayhem/proxy/registry-reference/v1", r))
                    .collect::<Vec<_>>())
            } else {
                Value::Null
            };
            let etag = format!(
                "\"{}\"",
                canonical_hash(
                    "mayhem/proxy/registry-representation/v1",
                    &json!({"release_id":release.release_id,
                "release_hash":release.release_hash,"kind":if lookup{"lookup"}else{"release"},"selector":selector})
                )
            );
            let mut response = Json(value).into_response();
            response.headers_mut().insert("etag", etag.parse().unwrap());
            response
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let router = axum::Router::new()
            .route("/{*path}", any(respond))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            origin,
            state,
            task,
        }
    }
    fn config(&self) -> crate::openai::proxy_control::RegistryConfig {
        crate::openai::proxy_control::RegistryConfig {
            origin: self.origin.clone(),
            allow_loopback_http: true,
        }
    }
    fn advance(&self) {
        let mut state = self.state.lock().unwrap();
        let (release, document) = publication(Some(&state.head), temperature(2));
        state.head = release.clone();
        state
            .releases
            .insert(release.release_id.clone(), (release, document));
    }
}
fn body(f: &Fixture) -> Value {
    let mut body = f.body();
    let controls = body["proxy"].clone();
    body["messages"] = json!([{"role":"user","content":"Hello 🧠 𐀀"}]);
    body["proxy"]["profile"] = json!({"schema_version":1,"lane":"proxy","endpoint":ProxyEndpoint::Chat,
        "target":{"kind":"exact_offer","offer_id":model(&f.harness).strip_prefix("proxy/offer/").unwrap()},
        "providers":{"allow":null,"deny":[],"require_verified_operator":false},"allowed_rails":[controls["rail"]],
        "prices":controls["prices"],"max_retail_cost_micro":"1000000",
        "settlement_policies":[{"rail":controls["rail"],"settlement_policy_hash":controls["settlement_policy_hash"]}],
        "constraints":{"minimum_context":null,"minimum_tokens_per_second":null,"output_units":controls["output_units"],
            "capabilities":[],"request_controls":[{"field_id":"fixture.temperature","schema_revision":1,"value":{"type":"decimal","value":"0.2"}}],"data_handling":[]},
        "ranking":"lowest_estimated_cost","continuity":"retain_compatible"});
    body
}
async fn prepare(f: &Fixture, body: Value, token: &str) -> (StatusCode, Value) {
    let response=f.router.clone().oneshot(Request::builder().method("POST").uri("/v1/proxy/profile/prepare")
        .header("content-type","application/json").header("authorization",format!("Bearer {token}"))
        .body(Body::from(serde_json::to_vec(&json!({"schema_version":1,"endpoint":"openai_chat_completions","request":body})).unwrap())).unwrap()).await.unwrap();
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
async fn fixture(registry: &Registry) -> Fixture {
    let mut contract = mayhem_proto::endpoint_family_contract_template(
        mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
    )
    .unwrap();
    contract
        .request_attribute_specs
        .get_mut("temperature")
        .unwrap()
        .maximum = Some(0.5);
    Fixture::start_with_registry(
        ProxyEndpoint::Chat,
        ProxyRail::Fiat,
        None,
        "owner-fixture-key",
        Some(contract),
        Some(registry.config()),
    )
    .await
}

#[tokio::test]
async fn published_registry_preparation_estimate_execution_and_original_replay_are_bound() {
    let registry = Registry::start().await;
    let mut f = fixture(&registry).await;
    let original = body(&f);
    let (status, prepared) = prepare(&f, original.clone(), "owner-fixture-key").await;
    assert_eq!(status, StatusCode::OK, "{prepared}");
    assert_eq!(prepared["request"]["temperature"], json!(0.2));
    assert_eq!(prepared["request"]["messages"], original["messages"]);
    assert_eq!(
        prepared["registry_release"]["release_id"],
        registry.state.lock().unwrap().head.release_id
    );
    let ready = prepared["request"].clone();
    let (status, _, quote) = estimation::estimate(&f, ready.clone(), "owner-fixture-key").await;
    assert_eq!(status, StatusCode::OK, "{quote}");
    assert_eq!(
        quote["request_content_digest"],
        prepared["request_content_digest"]
    );
    assert_eq!(
        quote["controls"]["registry_release"],
        prepared["registry_release"]
    );
    estimation::assert_no_purchase(&mut f).await;
    if let Ok(path) = std::env::var("PROXY_PROFILE_PREPARATION_FIXTURE") {
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
        std::fs::write(path,serde_json::to_vec_pretty(&json!({"schema_version":1,"request":{"schema_version":1,"endpoint":"openai_chat_completions","request":original},"publication":publication,"prepared":prepared,"estimate":quote})).unwrap()).unwrap();
    }
    let (status, headers, result) = f
        .post(
            "registry-profile",
            ready.clone(),
            "owner-fixture-key",
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(f.harness.backend_calls(), 1);
    let job = f
        .state
        .jobs
        .lock()
        .unwrap()
        .get(headers["x-mayhem-job-id"].to_str().unwrap(), now_secs())
        .unwrap()
        .unwrap();
    assert_eq!(
        quote["request_hash"],
        job.proxy.as_ref().unwrap().terms().unwrap().request_hash
    );
    registry.advance();
    f.control
        .control
        .registry()
        .unwrap()
        .refresh_head()
        .await
        .unwrap();
    // A newer semantic revision cannot reinterpret the prepared old release.
    let (status, reprepared) = prepare(&f, ready.clone(), "owner-fixture-key").await;
    assert_eq!(status, StatusCode::OK, "{reprepared}");
    assert_eq!(reprepared["registry_release"], prepared["registry_release"]);
    registry.state.lock().unwrap().unavailable = true;
    let reads = registry.state.lock().unwrap().reads;
    let (status, replay_headers, replay) = f
        .post(
            "registry-profile",
            ready.clone(),
            "owner-fixture-key",
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay, result);
    assert_eq!(
        replay_headers["x-mayhem-job-id"],
        headers["x-mayhem-job-id"]
    );
    assert_eq!(f.harness.backend_calls(), 1);
    assert_eq!(registry.state.lock().unwrap().reads, reads);
    let mut changed = ready;
    changed["temperature"] = json!(0.3);
    assert_eq!(
        f.post("registry-profile", changed, "owner-fixture-key", false)
            .await
            .0,
        StatusCode::CONFLICT
    );
    f.stop().await;
}

#[tokio::test]
async fn preparation_and_new_work_reject_unbound_conflicting_unknown_or_unsupported_controls() {
    let registry = Registry::start().await;
    let mut f = fixture(&registry).await;
    let original = body(&f);
    assert_eq!(
        prepare(&f, original.clone(), "wrong-key").await.0,
        StatusCode::UNAUTHORIZED
    );
    let (_, prepared) = prepare(&f, original.clone(), "owner-fixture-key").await;
    let ready = prepared["request"].clone();
    for mutate in [
        |v: &mut Value| {
            v["temperature"] = json!(0.3);
        },
        |v: &mut Value| {
            v.as_object_mut().unwrap().remove("temperature");
        },
        |v: &mut Value| {
            v["proxy"]
                .as_object_mut()
                .unwrap()
                .remove("registry_release");
        },
        |v: &mut Value| {
            v["proxy"]["registry_release"]["release_hash"] = json!("f".repeat(64));
        },
    ] {
        let mut bad = ready.clone();
        mutate(&mut bad);
        let (status, _, error) = estimation::estimate(&f, bad.clone(), "owner-fixture-key").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(
            f.post("invalid-controls", bad, "owner-fixture-key", false)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut conflict = original.clone();
    conflict["temperature"] = json!(0.3);
    assert_eq!(
        prepare(&f, conflict, "owner-fixture-key").await.0,
        StatusCode::BAD_REQUEST
    );
    let mut unsupported = original.clone();
    unsupported["proxy"]["profile"]["constraints"]["request_controls"][0]["value"]["value"] =
        json!("0.7");
    // Registry permits 0.7; the actual custom endpoint contract does not.
    assert_eq!(
        prepare(&f, unsupported, "owner-fixture-key").await.0,
        StatusCode::BAD_REQUEST
    );
    let mut unknown = original.clone();
    unknown["proxy"]["profile"]["constraints"]["request_controls"][0]["schema_revision"] = json!(2);
    assert_eq!(
        prepare(&f, unknown, "owner-fixture-key").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let mut evidence = original;
    evidence["proxy"]["profile"]["providers"]["require_verified_operator"] = json!(true);
    assert_eq!(
        prepare(&f, evidence, "owner-fixture-key").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    estimation::assert_no_purchase(&mut f).await;
    f.stop().await;
    let mut disabled = Fixture::start_with(ProxyEndpoint::Chat, ProxyRail::Fiat).await;
    assert_eq!(
        prepare(&disabled, body(&disabled), "owner-fixture-key")
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    estimation::assert_no_purchase(&mut disabled).await;
    disabled.stop().await;
}
