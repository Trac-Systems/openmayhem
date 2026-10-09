use mayhem_proto::{
    endpoint_family_contract_template, proxy::ProxyEndpoint, stable_json_bytes,
    EndpointAttributeSpec,
};
use mayhem_proxy::registry::{publication::*, *};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Notify,
};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/registry-publication-v1.json")).unwrap()
}
fn initial() -> Release {
    serde_json::from_value(fixture()["release"].clone()).unwrap()
}
fn original_document() -> Document {
    serde_json::from_value(fixture()["lookup"]["definitions"][0].clone()).unwrap()
}
fn hash(domain: &str, value: &Value) -> String {
    let mut bytes = domain.as_bytes().to_vec();
    bytes.push(0);
    bytes.extend(stable_json_bytes(value).unwrap());
    blake3::hash(&bytes).to_hex().to_string()
}
fn response_etag(release: &Value, refs: Option<&Value>) -> String {
    let selector = refs
        .map(|refs| {
            json!(refs
                .as_array()
                .unwrap()
                .iter()
                .map(|r| hash("mayhem/proxy/registry-reference/v1", r))
                .collect::<Vec<_>>())
        })
        .unwrap_or(Value::Null);
    format!(
        "\"{}\"",
        hash(
            "mayhem/proxy/registry-representation/v1",
            &json!({"release_id":release["release_id"],
        "release_hash":release["release_hash"],"kind":if refs.is_some(){"lookup"}else{"release"},"selector":selector})
        )
    )
}
fn definition(id: &str) -> Document {
    let mut document = original_document();
    document.field_id = id.into();
    document.definition.field_id = id.into();
    document.definition_hash = document.definition.digest().unwrap();
    document
}
fn release(parent: Option<&Release>, sequence: u64, docs: &[Document]) -> Release {
    let mut changes = docs
        .iter()
        .map(|d| DocumentReference {
            field_id: d.field_id.clone(),
            schema_revision: d.schema_revision,
            version: d.version,
            definition_hash: d.definition_hash.clone(),
        })
        .collect::<Vec<_>>();
    changes.sort_by(|a, b| a.field_id.cmp(&b.field_id));
    let manifest = Manifest {
        schema_version: 1,
        parent_release_id: parent.map(|r| r.release_id.clone()),
        parent_release_hash: parent.map(|r| r.release_hash.clone()),
        changes,
    };
    Release {
        object: "proxy.registry_release".into(),
        release_id: format!("00000000-0000-4000-8000-{sequence:012x}"),
        revision: sequence.to_string(),
        release_hash: manifest.digest().unwrap(),
        manifest,
        published_at: "2026-10-09T15:47:47.722Z".into(),
        publication_state: "published".into(),
    }
}
#[derive(Clone)]
struct Call {
    method: String,
    path: String,
    body: Value,
}
struct State {
    head: Value,
    releases: BTreeMap<String, Value>,
    documents: BTreeMap<(String, String, u32), Value>,
    calls: Vec<Call>,
    mutate: Option<fn(&mut Value)>,
    status: u16,
    delay: Duration,
    etag_override: Option<String>,
    length_override: Option<usize>,
    chunked: bool,
}
impl State {
    fn add(&mut self, release: &Release, documents: &[Document]) {
        self.releases
            .insert(release.release_id.clone(), json!(release));
        for d in documents {
            self.documents.insert(
                (
                    release.release_id.clone(),
                    d.field_id.clone(),
                    d.schema_revision,
                ),
                json!(d),
            );
        }
    }
}
struct Mock {
    origin: String,
    state: Arc<Mutex<State>>,
    notified: Arc<Notify>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Mock {
    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mut initial_state = State {
            head: json!(initial()),
            releases: BTreeMap::new(),
            documents: BTreeMap::new(),
            calls: vec![],
            mutate: None,
            status: 200,
            delay: Duration::ZERO,
            etag_override: None,
            length_override: None,
            chunked: false,
        };
        initial_state.add(&initial(), &[original_document()]);
        let state = Arc::new(Mutex::new(initial_state));
        let task_state = state.clone();
        let notified = Arc::new(Notify::new());
        let task_notify = notified.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let state = task_state.clone();
                let notify = task_notify.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let mut buffer = [0; 4096];
                    let (header_end, length) = loop {
                        let n = socket.read(&mut buffer).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buffer[..n]);
                        assert!(bytes.len() <= 64 * 1024);
                        if let Some(end) = bytes.windows(4).position(|p| p == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&bytes[..end]);
                            let length = headers
                                .lines()
                                .find_map(|h| {
                                    h.to_lowercase()
                                        .strip_prefix("content-length: ")
                                        .map(str::to_owned)
                                })
                                .map(|s| s.parse::<usize>().unwrap())
                                .unwrap_or(0);
                            if bytes.len() >= end + 4 + length {
                                break (end, length);
                            }
                        }
                    };
                    let headers = String::from_utf8_lossy(&bytes[..header_end]);
                    assert!(!headers.to_lowercase().contains("authorization:"));
                    let first = headers
                        .lines()
                        .next()
                        .unwrap()
                        .split_whitespace()
                        .collect::<Vec<_>>();
                    let call = Call {
                        method: first[0].into(),
                        path: first[1].into(),
                        body: if length > 0 {
                            serde_json::from_slice(&bytes[header_end + 4..header_end + 4 + length])
                                .unwrap()
                        } else {
                            Value::Null
                        },
                    };
                    let (status, body, etag, delay, declared, chunked) = {
                        let mut s = state.lock().unwrap();
                        s.calls.push(call.clone());
                        let suffix = call
                            .path
                            .strip_prefix("/v1/proxy/registry/releases/")
                            .unwrap();
                        let mut status = s.status;
                        let (mut body, default_etag) = if call.method == "GET" {
                            let body = if suffix == "current" {
                                s.head.clone()
                            } else {
                                s.releases.get(suffix).cloned().unwrap_or(Value::Null)
                            };
                            let tag = response_etag(&body, None);
                            (body, tag)
                        } else {
                            assert_eq!(call.method, "POST");
                            let id = suffix.strip_suffix("/lookup").unwrap();
                            let release = s.releases.get(id).cloned().unwrap_or(Value::Null);
                            let definitions = call.body["references"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|r| {
                                    s.documents
                                        .get(&(
                                            id.into(),
                                            r["field_id"].as_str().unwrap().into(),
                                            r["schema_revision"].as_u64().unwrap() as u32,
                                        ))
                                        .cloned()
                                })
                                .collect::<Option<Vec<_>>>();
                            if definitions.is_none() {
                                status = 404;
                            }
                            (
                                json!({"object":"proxy.registry_lookup","release_id":id,"release_hash":release["release_hash"],"definitions":definitions.unwrap_or_default()}),
                                response_etag(&release, Some(&call.body["references"])),
                            )
                        };
                        if status == 404 {
                            body = json!({"error":{"statusCode":404,"message":"Not published.","code":
                                if suffix == "current" { "proxy_registry_unpublished" }
                                else if call.method == "POST" { "proxy_registry_reference_unpublished" }
                                else { "proxy_registry_release_not_found" }}});
                        }
                        if let Some(mutate) = s.mutate {
                            mutate(&mut body);
                        }
                        (
                            status,
                            serde_json::to_vec(&body).unwrap(),
                            s.etag_override.clone().unwrap_or(default_etag),
                            s.delay,
                            s.length_override,
                            s.chunked,
                        )
                    };
                    notify.notify_one();
                    tokio::time::sleep(delay).await;
                    let headers = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nETag: {etag}\r\nConnection: close\r\n{}{}\r\n",
                        if status == 302 { "Location: http://127.0.0.1:1/forbidden\r\n" } else { "" },
                        if chunked { "Transfer-Encoding: chunked\r\n".into() } else { format!("Content-Length: {}\r\n", declared.unwrap_or(body.len())) });
                    if socket.write_all(headers.as_bytes()).await.is_err() {
                        return;
                    }
                    if chunked {
                        for chunk in body.chunks(4096) {
                            if socket
                                .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                                .await
                                .is_err()
                            {
                                return;
                            }
                            if socket.write_all(chunk).await.is_err() {
                                return;
                            }
                            if socket.write_all(b"\r\n").await.is_err() {
                                return;
                            }
                        }
                        let _ = socket.write_all(b"0\r\n\r\n").await;
                    } else {
                        let _ = socket.write_all(&body).await;
                    }
                });
            }
        });
        Self {
            origin,
            state,
            notified,
            task,
        }
    }
    fn reader(&self, limits: Limits) -> Reader {
        Reader::new(
            TrustedOrigin::local_loopback_http(&self.origin).unwrap(),
            limits,
        )
        .unwrap()
    }
    fn calls(&self) -> Vec<Call> {
        self.state.lock().unwrap().calls.clone()
    }
}

#[test]
fn actual_site_fixture_and_required_nullable_release_and_definition_wire() {
    let release = initial();
    release.validate().unwrap();
    original_document().validate().unwrap();
    assert_eq!(
        release.release_hash,
        "bff5338947a4ce8a3323a7146e7f40353623f1db222cf45cfaa7719f46647e98"
    );
    for field in ["parent_release_id", "parent_release_hash"] {
        let mut raw = fixture()["release"].clone();
        raw["manifest"].as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<Release>(raw).is_err());
    }
    for field in ["units", "default", "max_evidence_age_ms"] {
        let mut raw = fixture()["lookup"]["definitions"][0].clone();
        raw["definition"].as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<Document>(raw).is_err());
    }
    for field in ["release", "document"] {
        let mut raw = if field == "release" {
            fixture()["release"].clone()
        } else {
            fixture()["lookup"]["definitions"][0].clone()
        };
        raw["ignored"] = json!(true);
        assert!(if field == "release" {
            serde_json::from_value::<Release>(raw).is_err()
        } else {
            serde_json::from_value::<Document>(raw).is_err()
        });
    }
}

#[test]
fn trusted_origin_and_explicit_loopback_configuration_have_no_fallback() {
    assert!(TrustedOrigin::https("https://registry.example").is_ok());
    assert!(TrustedOrigin::local_loopback_http("http://127.0.0.1:3024").is_ok());
    assert!(TrustedOrigin::local_loopback_http("http://[::1]:3024").is_ok());
    for origin in [
        "http://127.0.0.1",
        "http://registry.example",
        "https://user:pass@registry.example",
        "https://@registry.example",
        "https://registry.example/path",
        "https://registry.example?x=1",
        "https://registry.example#x",
        "file:///tmp/a",
        " https://registry.example",
    ] {
        assert!(TrustedOrigin::https(origin).is_err(), "{origin}");
    }
    for origin in [
        "http://localhost:3024",
        "http://192.168.1.1",
        "https://registry.example",
    ] {
        assert!(TrustedOrigin::local_loopback_http(origin).is_err());
    }
    for limits in [
        Limits {
            concurrent_operations: 0,
            ..Limits::default()
        },
        Limits {
            cache_bytes: 1,
            ..Limits::default()
        },
        Limits {
            cache_entries: 4097,
            ..Limits::default()
        },
        Limits {
            head_ttl: Duration::from_secs(301),
            ..Limits::default()
        },
        Limits {
            operation_timeout: Duration::from_secs(31),
            ..Limits::default()
        },
    ] {
        assert!(Reader::new(
            TrustedOrigin::https("https://registry.example").unwrap(),
            limits
        )
        .is_err());
    }
}

#[tokio::test]
async fn actual_wire_head_exact_lookup_cache_and_unknown_evidence() {
    let mock = Mock::new().await;
    let reader = mock.reader(Limits::default());
    let pin = reader.current().await.unwrap();
    assert_eq!(pin.metadata(), &initial());
    assert_eq!(reader.current().await.unwrap().metadata(), pin.metadata());
    assert_eq!(
        reader
            .pin_release(&pin.metadata().release_id)
            .await
            .unwrap()
            .metadata(),
        pin.metadata()
    );
    let reference = original_document().reference();
    let snapshot = reader
        .lookup_exact(&pin, &[reference.clone()])
        .await
        .unwrap();
    assert_eq!(snapshot.documents().next().unwrap(), &original_document());
    reader
        .lookup_exact(&pin, &[reference.clone()])
        .await
        .unwrap();
    assert_eq!(mock.calls().len(), 2);
    let predicate = Predicate {
        field_id: reference.field_id.clone(),
        schema_revision: 1,
        operator: Operator::Eq,
        value: TypedValue::Boolean(true),
        evidence: Assurance::Verified,
        max_age_ms: None,
    };
    assert_eq!(
        evaluate(
            snapshot.get(&reference.field_id, 1).unwrap(),
            &predicate,
            None,
            Assurance::Verified,
            ProxyEndpoint::Chat,
            1
        )
        .unwrap(),
        Match::Unknown
    );
    let other = Mock::new().await;
    assert!(other
        .reader(Limits::default())
        .lookup_exact(&pin, &[reference])
        .await
        .is_err());
    assert!(other.calls().is_empty());
}

#[tokio::test]
async fn strict_lookup_rejects_wrong_release_hash_order_missing_extra_and_unknown_fields() {
    let mutations: &[fn(&mut Value)] = &[
        |v| v["release_id"] = json!("00000000-0000-4000-8000-000000000099"),
        |v| v["release_hash"] = json!("a".repeat(64)),
        |v| v["definitions"].as_array_mut().unwrap().clear(),
        |v| {
            let d = v["definitions"][0].clone();
            v["definitions"].as_array_mut().unwrap().push(d);
        },
        |v| v["definitions"][0]["schema_revision"] = json!(2),
        |v| v["definitions"][0]["definition_hash"] = json!("0".repeat(64)),
        |v| v["definitions"][0]["version"] = json!(2),
        |v| v["definitions"][0]["definition"]["labels"]["en"] = json!("Changed"),
        |v| v["definitions"][0]["definition"]["extra"] = json!(true),
        |v| v["definitions"][0]["extra"] = json!(true),
        |v| v["extra"] = json!(true),
    ];
    for mutation in mutations {
        let mock = Mock::new().await;
        let reader = mock.reader(Limits::default());
        let pin = reader.current().await.unwrap();
        mock.state.lock().unwrap().mutate = Some(*mutation);
        assert!(reader
            .lookup_exact(&pin, &[original_document().reference()])
            .await
            .is_err());
        mock.state.lock().unwrap().mutate = None;
        assert!(reader
            .lookup_exact(&pin, &[original_document().reference()])
            .await
            .is_ok()); // rejected batch was never cached
        assert_eq!(mock.calls().len(), 3);
    }
    let mock = Mock::new().await;
    let docs = [definition("fixture.a"), definition("fixture.b")];
    let rel = release(None, 1, &docs);
    {
        let mut s = mock.state.lock().unwrap();
        s.head = json!(rel);
        s.add(&rel, &docs);
    }
    let reader = mock.reader(Limits::default());
    let pin = reader.current().await.unwrap();
    mock.state.lock().unwrap().mutate =
        Some(|v| v["definitions"].as_array_mut().unwrap().reverse());
    assert!(reader
        .lookup_exact(
            &pin,
            &docs.iter().map(Document::reference).collect::<Vec<_>>()
        )
        .await
        .is_err());
}

#[tokio::test]
async fn release_schema_digest_identity_etag_and_status_fail_closed() {
    let mutations: &[fn(&mut Value)] = &[
        |v| v["object"] = json!("other"),
        |v| v["publication_state"] = json!("draft"),
        |v| v["revision"] = json!("01"),
        |v| v["revision"] = json!("9223372036854775808"),
        |v| v["release_hash"] = json!("0".repeat(64)),
        |v| v["manifest"]["changes"][0]["version"] = json!(2),
        |v| v["published_at"] = json!("2026-02-30T00:00:00.000Z"),
        |v| {
            v["manifest"]
                .as_object_mut()
                .unwrap()
                .remove("parent_release_id")
                .map(|_| ())
                .unwrap()
        },
        |v| v["extra"] = json!(true),
    ];
    for mutation in mutations {
        let mock = Mock::new().await;
        mock.state.lock().unwrap().mutate = Some(*mutation);
        assert!(mock.reader(Limits::default()).current().await.is_err());
    }
    for status in [302, 304, 401, 404, 429, 500] {
        let mock = Mock::new().await;
        mock.state.lock().unwrap().status = status;
        assert!(mock.reader(Limits::default()).current().await.is_err());
        assert_eq!(mock.calls().len(), 1);
    }
    let mock = Mock::new().await;
    mock.state.lock().unwrap().etag_override = Some(format!("\"{}\"", "0".repeat(64)));
    assert!(mock.reader(Limits::default()).current().await.is_err());
    let mock = Mock::new().await;
    let reader = mock.reader(Limits::default());
    let wrong = "00000000-0000-4000-8000-000000000099";
    mock.state
        .lock()
        .unwrap()
        .releases
        .insert(wrong.into(), json!(initial()));
    assert!(reader.pin_release(wrong).await.is_err());
}

#[tokio::test]
async fn pinned_history_survives_refresh_and_head_never_rolls_back_or_equivocates() {
    let mock = Mock::new().await;
    let reader = mock.reader(Limits::default());
    let first = reader.current().await.unwrap();
    let snapshot = reader
        .lookup_exact(&first, &[original_document().reference()])
        .await
        .unwrap();
    let mut updated = original_document();
    updated.version = 2;
    updated.schema_revision = 2;
    updated.definition.schema_revision = 2;
    updated.definition.minimum_assurance = Assurance::Verified;
    updated.definition_hash = updated.definition.digest().unwrap();
    let second = release(Some(first.metadata()), 2, &[updated.clone()]);
    {
        let mut s = mock.state.lock().unwrap();
        s.head = json!(second);
        s.add(&second, &[updated, original_document()]);
    }
    let pin = reader.refresh_head().await.unwrap();
    assert_eq!(pin.metadata(), &second);
    assert_eq!(
        reader
            .lookup_exact(&pin, &[original_document().reference()])
            .await
            .unwrap()
            .documents()
            .next()
            .unwrap(),
        &original_document()
    );
    assert_eq!(snapshot.documents().next().unwrap(), &original_document());
    mock.state.lock().unwrap().head = json!(initial());
    assert!(matches!(reader.refresh_head().await, Err(Error::Rollback)));
    let mut fork = second.clone();
    fork.release_id = "00000000-0000-4000-8000-0000000000ff".into();
    mock.state.lock().unwrap().head = json!(fork);
    assert!(matches!(
        reader.refresh_head().await,
        Err(Error::Equivocation)
    ));
    assert_eq!(
        reader
            .pin_release(&first.metadata().release_id)
            .await
            .unwrap()
            .metadata(),
        first.metadata()
    );
    let third = release(Some(&second), 3, &[definition("fixture.third")]);
    mock.state
        .lock()
        .unwrap()
        .add(&third, &[definition("fixture.third")]);
    reader.pin_release(&third.release_id).await.unwrap();
    mock.state.lock().unwrap().head = json!(second);
    assert!(matches!(reader.current().await, Err(Error::Rollback))); // a known newer exact release invalidates cached head freshness
}

#[tokio::test]
async fn ttl_expiry_deadline_and_concurrency_never_return_stale_current_head() {
    let mock = Mock::new().await;
    let reader = mock.reader(Limits {
        head_ttl: Duration::from_secs(1),
        operation_timeout: Duration::from_millis(80),
        concurrent_operations: 1,
        ..Limits::default()
    });
    reader.current().await.unwrap();
    mock.state.lock().unwrap().status = 503;
    assert!(reader.current().await.is_ok());
    assert_eq!(mock.calls().len(), 1);
    tokio::time::sleep(Duration::from_millis(1010)).await;
    assert!(matches!(reader.current().await, Err(Error::Http(503))));
    {
        let mut s = mock.state.lock().unwrap();
        s.status = 200;
        s.delay = Duration::from_millis(300);
    }
    // Drain already delivered notifications before the in-flight assertion.
    while tokio::time::timeout(Duration::from_millis(1), mock.notified.notified())
        .await
        .is_ok()
    {}
    let first = reader.refresh_head();
    tokio::pin!(first);
    tokio::select! { _ = mock.notified.notified() => {}, result = &mut first => panic!("unexpected early finish: {result:?}") }
    assert!(matches!(reader.refresh_head().await, Err(Error::Busy)));
    assert!(matches!(first.await, Err(Error::Deadline)));
}

#[tokio::test]
async fn batch_bounds_large_valid_bodies_and_count_and_byte_bounded_cache_eviction() {
    let mock = Mock::new().await;
    let reader = mock.reader(Limits {
        cache_entries: 2,
        cache_bytes: 8 * 1024 * 1024,
        ..Limits::default()
    });
    let mut docs = (0..96)
        .map(|i| definition(&format!("fixture.f{i:03}")))
        .collect::<Vec<_>>();
    for doc in &mut docs {
        for i in 0..8 {
            doc.definition
                .help
                .insert(format!("l{i}"), "x".repeat(1024));
        }
        doc.definition_hash = doc.definition.digest().unwrap();
    }
    let first = release(None, 1, &docs[..32]);
    let second = release(Some(&first), 2, &docs[32..64]);
    let rel = release(Some(&second), 3, &docs[64..]);
    {
        let mut s = mock.state.lock().unwrap();
        s.head = json!(rel);
        s.add(&rel, &docs);
    }
    let pin = reader.current().await.unwrap();
    let refs = docs.iter().map(Document::reference).collect::<Vec<_>>();
    let snapshot = reader.lookup_exact(&pin, &refs).await.unwrap();
    assert_eq!(snapshot.len(), 96);
    assert!(snapshot.encoded_bytes() > 256 * 1024);
    assert!(snapshot.encoded_bytes() <= MAX_SNAPSHOT_BYTES);
    let before = mock.calls().len();
    assert!(reader.lookup_exact(&pin, &[]).await.is_err());
    assert!(reader
        .lookup_exact(&pin, &[refs[0].clone(), refs[0].clone()])
        .await
        .is_err());
    let mut too_many = refs.clone();
    too_many.push(definition("fixture.extra").reference());
    assert!(reader.lookup_exact(&pin, &too_many).await.is_err());
    assert_eq!(mock.calls().len(), before);
    reader.lookup_exact(&pin, &[refs[0].clone()]).await.unwrap();
    assert_eq!(mock.calls().len(), before + 1); // evicted, not a permanent cardinality cap
    assert_eq!(
        snapshot.get(&refs[0].field_id, 1).unwrap(),
        &docs[0].definition
    ); // ownership survives eviction
    {
        let mut s = mock.state.lock().unwrap();
        s.length_override = Some(usize::MAX / 2);
    }
    assert!(matches!(
        reader.lookup_exact(&pin, &[refs[1].clone()]).await,
        Err(Error::Invalid("registry response exceeds byte bound"))
    ));
    {
        let mut s = mock.state.lock().unwrap();
        s.length_override = None;
        s.chunked = true;
        s.mutate = Some(|v| v["oversize"] = json!("x".repeat(128 * 1024)));
    }
    assert!(matches!(
        reader.lookup_exact(&pin, &[refs[1].clone()]).await,
        Err(Error::Invalid("registry response exceeds byte bound"))
    ));
}

#[tokio::test]
async fn cache_byte_budget_evicts_independently_of_entry_limit() {
    let mock = Mock::new().await;
    let mut docs = (0..4)
        .map(|i| definition(&format!("fixture.bytes{i}")))
        .collect::<Vec<_>>();
    for doc in &mut docs {
        for i in 0..8 {
            doc.definition
                .help
                .insert(format!("l{i}"), "x".repeat(1024));
        }
        doc.definition_hash = doc.definition.digest().unwrap();
    }
    let rel = release(None, 1, &docs);
    {
        let mut s = mock.state.lock().unwrap();
        s.head = json!(rel);
        s.add(&rel, &docs);
    }
    let reader = mock.reader(Limits {
        cache_entries: 512,
        cache_bytes: 32 * 1024,
        ..Limits::default()
    });
    let pin = reader.current().await.unwrap();
    let snapshot = reader
        .lookup_exact(
            &pin,
            &docs.iter().map(Document::reference).collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    assert!(snapshot.encoded_bytes() > 32 * 1024);
    assert_eq!(mock.calls().len(), 2);
    reader
        .lookup_exact(&pin, &[docs[0].reference()])
        .await
        .unwrap();
    assert_eq!(mock.calls().len(), 3);
    assert_eq!(snapshot.len(), 4);
}

#[tokio::test]
async fn definition_byte_bound_and_genuine_unpublished_errors_are_distinct() {
    let mock = Mock::new().await;
    let reader = mock.reader(Limits::default());
    let pin = reader.current().await.unwrap();
    mock.state.lock().unwrap().mutate = Some(|v| {
        let definition = &mut v["definitions"][0]["definition"];
        definition["help"] = json!((0..18)
            .map(|i| (format!("l{i}"), "x".repeat(1024)))
            .collect::<BTreeMap<_, _>>());
        let digest = hash("mayhem/proxy/registry-definition/v1", definition);
        v["definitions"][0]["definition_hash"] = json!(digest);
    });
    assert!(matches!(
        reader
            .lookup_exact(&pin, &[original_document().reference()])
            .await,
        Err(Error::Invalid("invalid registry definition"))
    ));
    {
        let mut s = mock.state.lock().unwrap();
        s.mutate = None;
        s.status = 404;
    }
    assert!(matches!(
        reader.refresh_head().await,
        Err(Error::NotPublished)
    ));
    mock.state.lock().unwrap().mutate = Some(|v| v["error"]["code"] = json!("not_found"));
    assert!(matches!(reader.refresh_head().await, Err(Error::Http(404))));
}

fn control_document(id: &str, next: Option<&str>) -> Document {
    let mut document = definition(id);
    document.definition.usage = Usage::RequestControl {
        request_path: id
            .strip_prefix("fixture.")
            .unwrap_or("reasoning.enabled")
            .into(),
    };
    if let Some(next) = next {
        let condition = |id: &str| Condition {
            field_id: id.into(),
            schema_revision: 1,
            operator: Operator::Eq,
            value: TypedValue::Boolean(true),
        };
        document.definition.rules = vec![Rule {
            when: vec![condition(id)],
            require: vec![condition(next)],
            forbid: vec![],
        }];
    }
    document.definition_hash = document.definition.digest().unwrap();
    document
}

#[tokio::test]
async fn exact_bounded_cyclic_closure_feeds_existing_controls_without_evidence_or_defaults() {
    let mock = Mock::new().await;
    let docs = [
        control_document("fixture.reasoning.enabled", Some("fixture.reasoning.check")),
        control_document("fixture.reasoning.check", Some("fixture.reasoning.enabled")),
    ];
    let rel = release(None, 1, &docs);
    {
        let mut s = mock.state.lock().unwrap();
        s.head = json!(rel);
        s.add(&rel, &docs);
    }
    let reader = mock.reader(Limits::default());
    let pin = reader.current().await.unwrap();
    let snapshot = reader
        .resolve_closure(&pin, &[docs[0].reference()])
        .await
        .unwrap();
    assert_eq!(snapshot.len(), 2);
    assert_eq!(
        mock.calls().iter().filter(|c| c.method == "POST").count(),
        2
    );
    let mut contract = endpoint_family_contract_template("openai_chat_completions").unwrap();
    for path in ["reasoning.enabled", "reasoning.check"] {
        contract.request_attributes.push(path.into());
        contract.request_attribute_specs.insert(
            path.into(),
            serde_json::from_value::<EndpointAttributeSpec>(json!({"value_types":["boolean"]}))
                .unwrap(),
        );
    }
    let request = json!({"model":"example","messages":[{"role":"user","content":"hello"}],"reasoning":{"check":true}});
    let controls = [Control {
        field_id: docs[0].field_id.clone(),
        schema_revision: 1,
        value: TypedValue::Boolean(true),
    }];
    let result = apply_controls(
        &request,
        ProxyEndpoint::Chat,
        &contract,
        &controls,
        |id, revision| snapshot.get(id, revision),
    )
    .unwrap();
    assert_eq!(result["reasoning"]["enabled"], true);
    assert!(request["reasoning"].get("enabled").is_none());
    let missing = json!({"model":"example","messages":[{"role":"user","content":"hello"}]});
    assert!(apply_controls(
        &missing,
        ProxyEndpoint::Chat,
        &contract,
        &controls,
        |id, revision| snapshot.get(id, revision)
    )
    .is_err());
}

#[tokio::test]
async fn exact_historical_roots_may_differ_but_one_graph_cannot_mix_semantics() {
    let mut a1 = control_document("fixture.reasoning.enabled", None);
    let mut a2 = a1.clone();
    a2.version = 2;
    a2.schema_revision = 2;
    a2.definition.schema_revision = 2;
    a2.definition_hash = a2.definition.digest().unwrap();
    let first = release(None, 1, &[a1.clone()]);
    let second = release(Some(&first), 2, &[a2.clone()]);
    let mock = Mock::new().await;
    {
        let mut s = mock.state.lock().unwrap();
        s.head = json!(second);
        s.add(&second, &[a1.clone(), a2.clone()]);
    }
    let reader = mock.reader(Limits::default());
    let pin = reader.current().await.unwrap();
    assert_eq!(
        reader
            .resolve_closure(&pin, &[a1.reference(), a2.reference()])
            .await
            .unwrap()
            .len(),
        2
    );

    // An inconsistent trusted response still cannot smuggle mixed semantics
    // into one closure just because each individual definition hashes correctly.
    a1.definition.rules = vec![Rule {
        when: vec![Condition {
            field_id: a1.field_id.clone(),
            schema_revision: 1,
            operator: Operator::Eq,
            value: TypedValue::Boolean(true),
        }],
        require: vec![Condition {
            field_id: a1.field_id.clone(),
            schema_revision: 2,
            operator: Operator::Eq,
            value: TypedValue::Boolean(true),
        }],
        forbid: vec![],
    }];
    a1.definition_hash = a1.definition.digest().unwrap();
    let bad = Mock::new().await;
    {
        let mut s = bad.state.lock().unwrap();
        s.head = json!(second);
        s.add(&second, &[a1.clone(), a2]);
    }
    let reader = bad.reader(Limits::default());
    let pin = reader.current().await.unwrap();
    assert!(matches!(
        reader.resolve_closure(&pin, &[a1.reference()]).await,
        Err(Error::Invalid(
            "registry conditional graph mixes semantic revisions"
        ))
    ));
}

#[tokio::test]
async fn invalid_or_oversized_reference_closure_is_never_widened_or_partially_returned() {
    for fault in [
        "missing",
        "filter",
        "endpoint",
        "type",
        "mixed",
        "filter-rules",
    ] {
        let mock = Mock::new().await;
        let mut a = control_document("fixture.reasoning.enabled", Some("fixture.reasoning.check"));
        let mut b = control_document("fixture.reasoning.check", None);
        match fault {
            "filter" => b.definition.usage = Usage::FilterOnly,
            "endpoint" => b.definition.endpoints = vec![ProxyEndpoint::Responses],
            "type" => {
                b.definition.value_schema = ValueSchema::Enum {
                    values: vec!["yes".into()],
                }
            }
            "mixed" => a.definition.rules[0].require.push(Condition {
                field_id: a.field_id.clone(),
                schema_revision: 2,
                operator: Operator::Eq,
                value: TypedValue::Boolean(true),
            }),
            "filter-rules" => a.definition.usage = Usage::FilterOnly,
            _ => {}
        }
        a.definition_hash = a.definition.digest().unwrap();
        b.definition_hash = b.definition.digest().unwrap();
        let docs = if fault == "missing" {
            vec![a.clone()]
        } else {
            vec![a.clone(), b]
        };
        let rel = release(None, 1, &docs);
        {
            let mut s = mock.state.lock().unwrap();
            s.head = json!(rel);
            s.add(&rel, &docs);
        }
        let reader = mock.reader(Limits::default());
        let pin = reader.current().await.unwrap();
        assert!(
            reader
                .resolve_closure(&pin, &[a.reference()])
                .await
                .is_err(),
            "{fault}"
        );
    }
    let mock = Mock::new().await;
    let docs = (0..97)
        .map(|i| {
            control_document(
                &format!("fixture.f{i}"),
                (i < 96).then(|| format!("fixture.f{}", i + 1)).as_deref(),
            )
        })
        .collect::<Vec<_>>();
    let first = release(None, 1, &docs[..32]);
    let second = release(Some(&first), 2, &docs[32..64]);
    let third = release(Some(&second), 3, &docs[64..96]);
    let rel = release(Some(&third), 4, &docs[96..]);
    {
        let mut s = mock.state.lock().unwrap();
        s.head = json!(rel);
        s.add(&rel, &docs);
    }
    let reader = mock.reader(Limits::default());
    let pin = reader.current().await.unwrap();
    assert!(reader
        .resolve_closure(&pin, &[docs[0].reference()])
        .await
        .is_err());
    assert_eq!(
        mock.calls().iter().filter(|c| c.method == "POST").count(),
        96
    );
    assert!(!mock
        .calls()
        .iter()
        .any(|c| c.body.to_string().contains("fixture.f96")));
}
