//! Actual running Core HTTP + canonical fixture/worker; metadata responses were
//! exported from real SITE Nest/PostgreSQL publication, never production reads.
use super::*;
use axum::{extract::State, routing::any};
use std::collections::BTreeMap;
fn fixture() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../mayhem-proxy/tests/fixtures/taxonomy-propagation-v1.json"
    )))
    .unwrap()
}
fn hash(domain: &str, v: &Value) -> String {
    let mut b = domain.as_bytes().to_vec();
    b.push(0);
    b.extend(mayhem_proto::stable_json_bytes(v).unwrap());
    blake3::hash(&b).to_hex().to_string()
}
#[derive(Default)]
struct MetadataState {
    published: bool,
    unavailable: bool,
    deny: bool,
    large: bool,
    reads: BTreeMap<String, usize>,
}
struct Metadata {
    origin: String,
    state: Arc<Mutex<MetadataState>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Metadata {
    fn drop(&mut self) {
        self.task.abort()
    }
}
impl Metadata {
    async fn start() -> Self {
        let state = Arc::new(Mutex::new(MetadataState::default()));
        async fn respond(
            State(state): State<Arc<Mutex<MetadataState>>>,
            r: Request<Body>,
        ) -> Response {
            let path = r.uri().path().to_owned();
            let url = url::Url::parse(&format!("http://127.0.0.1{}", r.uri())).unwrap();
            let q: BTreeMap<String, String> = url.query_pairs().into_owned().collect();
            let bytes = to_bytes(r.into_body(), 32 * 1024).await.unwrap();
            let body = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice::<Value>(&bytes).unwrap()
            };
            let mut s = state.lock().unwrap();
            *s.reads.entry(path.clone()).or_default() += 1;
            if !s.published || s.unavailable {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            let f = fixture();
            let taxonomy = path.starts_with("/v1/proxy/taxonomy/");
            let id = path.split('/').nth(5).unwrap();
            let updated = id == f["updated"]["release_id"].as_str().unwrap();
            let release = if taxonomy {
                if updated {
                    &f["updated"]
                } else {
                    &f["taxonomy"]
                }
            } else {
                &f["registry"]
            };
            if id != "current" && id != release["release_id"].as_str().unwrap() {
                return StatusCode::NOT_FOUND.into_response();
            }
            let (mut out, kind, selector) = if path.ends_with("/members") {
                let mut page = if updated {
                    f["updated_members"].clone()
                } else {
                    f["members"].clone()
                };
                if s.large {
                    let position = q.get("cursor").map(|v| v.parse::<usize>().unwrap());
                    if position.is_none() {
                        page["scopes"] = json!([]);
                        page["scanned_entries"] = json!(256);
                        page["next_cursor"] = json!("0");
                        page["exhausted"] = json!(false);
                    } else {
                        let start = position.unwrap();
                        let end = (start + q["limit"].parse::<usize>().unwrap()).min(132);
                        let mut scopes = vec![];
                        for n in start..end {
                            if n == 0 {
                                scopes.push(f["members"]["scopes"][0].clone())
                            } else {
                                scopes.push(json!({"source":{"entry_id":format!("empty{n}"),"schema_revision":1,"version":1,"document_hash":"e".repeat(64)},"kind":"model","family_id":"empty","model_id":format!("missing{n}"),"revision":"","quantization":""}));
                            }
                        }
                        page["scopes"] = json!(scopes);
                        page["scanned_entries"] = json!(end - start);
                        page["next_cursor"] = if end < 132 {
                            json!(end.to_string())
                        } else {
                            Value::Null
                        };
                        page["exhausted"] = json!(end == 132);
                    }
                }
                (
                    page,
                    "members",
                    json!({"entry_id":q["entry_id"],"schema_revision":q["schema_revision"].parse::<u32>().unwrap(),"limit":q["limit"].parse::<usize>().unwrap(),"cursor":q.get("cursor")}),
                )
            } else if path.ends_with("/match") {
                assert_eq!(body, f["match_request"]);
                let mut answer = if updated {
                    f["updated_matched"].clone()
                } else {
                    f["matched"].clone()
                };
                if s.deny {
                    answer["matches"][0]["source"] = Value::Null;
                    answer["matches"][0]["scope"] = Value::Null;
                }
                (
                    answer,
                    "match",
                    json!({"entry_id":body["entry_id"],"schema_revision":body["schema_revision"],"models":body["models"].as_array().unwrap().iter().map(|v|hash("mayhem/proxy/taxonomy-match-model/v1",v)).collect::<Vec<_>>()}),
                )
            } else if path.ends_with("/lookup") {
                (
                    f["registry_lookup"].clone(),
                    "lookup",
                    json!(body["references"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| hash("mayhem/proxy/registry-reference/v1", v))
                        .collect::<Vec<_>>()),
                )
            } else {
                (release.clone(), "release", Value::Null)
            };
            if out.is_null() {
                return StatusCode::NOT_FOUND.into_response();
            }
            let domain = if taxonomy {
                "mayhem/proxy/taxonomy-representation/v1"
            } else {
                "mayhem/proxy/registry-representation/v1"
            };
            let tag = format!(
                "\"{}\"",
                hash(
                    domain,
                    &json!({"release_id":release["release_id"],"release_hash":release["release_hash"],"kind":kind,"selector":selector})
                )
            );
            let mut response = Json(out.take()).into_response();
            response.headers_mut().insert("etag", tag.parse().unwrap());
            response
        }
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", l.local_addr().unwrap());
        let app = axum::Router::new()
            .route("/{*path}", any(respond))
            .with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
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
    fn member_reads(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .reads
            .iter()
            .filter(|(p, _)| p.ends_with("/members"))
            .map(|(_, n)| n)
            .sum()
    }
}
struct Http {
    origin: String,
    client: reqwest::Client,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Http {
    fn drop(&mut self) {
        self.task.abort()
    }
}
impl Http {
    async fn start(router: axum::Router) -> Self {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", l.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(l, router).await.unwrap() });
        Self {
            origin,
            client: reqwest::Client::new(),
            task,
        }
    }
    async fn post(&self, path: &str, body: &Value, key: Option<&str>) -> (u16, Value) {
        let mut req = self
            .client
            .post(format!("{}{path}", self.origin))
            .bearer_auth("owner-fixture-key")
            .json(body);
        if let Some(k) = key {
            req = req.header("idempotency-key", k)
        }
        let r = req.send().await.unwrap();
        let status = r.status().as_u16();
        let body = r.json().await.unwrap();
        (status, body)
    }
    async fn resolve(&self, input: Value) -> Value {
        let (mut code, mut v) = self.post("/v1/proxy/profile/resolve", &input, None).await;
        assert_eq!(code, 200, "{v}");
        let mut steps = 0;
        while v["status"] == "pending" {
            assert_eq!(v["ranking_claim"], "none");
            assert!(v["selection"].is_null());
            steps += 1;
            assert!(steps < 400, "{v}");
            (code, v) = self
                .post("/v1/proxy/profile/resolve", &v["continuation"], None)
                .await;
            assert_eq!(code, 200, "{v}");
        }
        v
    }
}
fn input(f: &Fixture) -> Value {
    let data = fixture();
    let mut v = super::resolver::start(f);
    v["request"]["proxy"]["profile"]["target"] = json!({"kind":"taxonomy_category","taxonomy":{"release_id":data["taxonomy"]["release_id"],"release_hash":data["taxonomy"]["release_hash"],"entry_id":"runtime_new_category","schema_revision":1},"variants":[],"tags":[],"market_allowlist":null});
    v["request"]["proxy"]["profile"]["constraints"]["request_controls"] =
        data["request_controls"].clone();
    v
}
#[tokio::test]
async fn taxonomy_publication_reaches_same_running_resolver_estimate_execution_and_replay() {
    let _case = estimation::ESTIMATE_CASE.lock().await;
    let data = fixture();
    let metadata = Metadata::start().await;
    let mut f = Fixture::start_with_registry(
        ProxyEndpoint::Chat,
        ProxyRail::Fiat,
        None,
        "owner-fixture-key",
        None,
        Some(metadata.config()),
    )
    .await;
    let http = Http::start(f.router.clone()).await;
    let pid = std::process::id();
    let original = input(&f);
    assert_eq!(
        http.post("/v1/proxy/profile/resolve", &original, None)
            .await
            .0,
        503
    );
    metadata.state.lock().unwrap().published = true;
    let selected = http.resolve(original.clone()).await;
    assert_eq!(selected["status"], "selected", "{selected}");
    assert_eq!(selected["selection"]["request"]["temperature"], json!(0.2));
    assert_eq!(selected["scope_exhausted"], true);
    assert_eq!(
        selected["selection"]["request"]["proxy"]["registry_release"],
        data["registry_release"]
    );
    let ready = selected["selection"]["request"].clone();
    let before = metadata.member_reads();
    let (code, estimate) = http
        .post(
            "/v1/proxy/estimate",
            &json!({"schema_version":1,"endpoint":ProxyEndpoint::Chat,"request":ready}),
            None,
        )
        .await;
    assert_eq!(code, 200, "{estimate}");
    assert_eq!(
        estimate["request_hash"],
        selected["selection"]["estimate"]["request_hash"]
    );
    assert_eq!(
        metadata.member_reads(),
        before,
        "exact quote may not traverse category"
    );
    estimation::assert_no_purchase(&mut f).await;
    metadata.state.lock().unwrap().deny = true;
    assert_ne!(
        http.post("/v1/chat/completions", &ready, Some("taxonomy-denied"))
            .await
            .0,
        200
    );
    assert_eq!(f.harness.backend_calls(), 0);
    metadata.state.lock().unwrap().deny = false;
    let (code, result) = http
        .post("/v1/chat/completions", &ready, Some("taxonomy-original"))
        .await;
    assert_eq!(code, 200, "{result}");
    assert_eq!(f.harness.backend_calls(), 1);
    assert_eq!(
        metadata.member_reads(),
        before,
        "admission may not traverse category"
    );
    metadata.state.lock().unwrap().large = true;
    let large = http.resolve(original.clone()).await;
    assert_eq!(large["status"], "selected", "{large}");
    assert_eq!(large["considered_candidates"], 1);
    assert_eq!(large["scope_exhausted"], true);
    assert!(metadata.member_reads() > 8);
    metadata.state.lock().unwrap().large = false;
    let mut updated = original.clone();
    updated["request"]["proxy"]["profile"]["target"]["taxonomy"]["release_id"] =
        data["updated"]["release_id"].clone();
    updated["request"]["proxy"]["profile"]["target"]["taxonomy"]["release_hash"] =
        data["updated"]["release_hash"].clone();
    let version = http.resolve(updated).await;
    assert_eq!(version["status"], "selected", "{version}");
    metadata.state.lock().unwrap().unavailable = true;
    let (code, replay) = http
        .post("/v1/chat/completions", &ready, Some("taxonomy-original"))
        .await;
    assert_eq!(code, 200, "{replay}");
    assert_eq!(replay, result);
    assert_eq!(f.harness.backend_calls(), 1);
    assert_ne!(
        http.post("/v1/chat/completions", &ready, Some("taxonomy-new-offline"))
            .await
            .0,
        200
    );
    assert_eq!(f.harness.backend_calls(), 1);
    assert_eq!(pid, std::process::id());
    if let Some(path) = std::env::var_os("MAYHEM_TEST_TAXONOMY_RESOLVER_FIXTURE") {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        out.write_all(&serde_json::to_vec_pretty(&json!({"test_only":true,"scope":"actual Core HTTP resolver, canonical local fixture, worker; actual SITE metadata replayed by loopback HTTP","process_id_before":pid,"process_id_after":std::process::id(),"request":original,"selected":selected,"estimate":estimate,"large_scope":large,"updated_release":version,"result":result,"replay":replay,"backend_calls":f.harness.backend_calls(),"metadata_member_reads":metadata.member_reads()})).unwrap()).unwrap();
        out.sync_all().unwrap();
    }
    drop(http);
    f.stop().await;
}
