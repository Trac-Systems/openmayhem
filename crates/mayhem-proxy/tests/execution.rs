#![cfg(unix)]

use mayhem_proto::{
    endpoint_family_contract_template,
    proxy::{ProxyEndpoint, ProxyRail},
};
use mayhem_proxy::{
    attempts::{self, Binding, Digest, Identity, Journal, Phase},
    connector::{
        config::ConnectionConfig,
        failure::{Code, Execution},
        http::HttpConnection,
    },
    endpoint::{Adapter, Limits},
    execution::{Cancellation, Error, Executor, Storage},
    worker::host::{Pool, PoolLimits},
};
use serde_json::{json, Value};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

fn d(n: u64) -> Digest {
    Digest::new(format!("{n:064x}")).unwrap()
}
fn dir() -> tempfile::TempDir {
    let p = tempfile::tempdir().unwrap();
    std::fs::set_permissions(p.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    p
}
struct Backend {
    base: String,
    calls: Arc<AtomicUsize>,
    bodies: Arc<Mutex<Vec<Value>>>,
    handle: JoinHandle<()>,
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
async fn backend(status: u16, body: Value, delay: Duration) -> Backend {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1/", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let received = bodies.clone();
    let handle = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0u8; 1024];
            let boundary = loop {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    break None;
                }
                bytes.extend_from_slice(&buf[..n]);
                assert!(bytes.len() < 2 * 1024 * 1024);
                if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(i + 4);
                }
            };
            let Some(boundary) = boundary else { continue };
            let headers = String::from_utf8_lossy(&bytes[..boundary]);
            let length = headers
                .lines()
                .find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|s| s.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            while bytes.len() < boundary + length {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
            }
            count.fetch_add(1, Ordering::SeqCst);
            received
                .lock()
                .unwrap()
                .push(serde_json::from_slice(&bytes[boundary..boundary + length]).unwrap());
            tokio::time::sleep(delay).await;
            let body = serde_json::to_vec(&body).unwrap();
            let header=format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
            if socket.write_all(header.as_bytes()).await.is_err() {
                continue;
            }
            for chunk in body.chunks(7) {
                if socket.write_all(chunk).await.is_err() {
                    break;
                }
            }
        }
    });
    Backend {
        base,
        calls,
        bodies,
        handle,
    }
}
struct Fixture {
    _store: tempfile::TempDir,
    _work: tempfile::TempDir,
    journal: Arc<Journal>,
    executor: Executor,
    adapter: Arc<Adapter>,
    connection: Arc<HttpConnection>,
}
impl Fixture {
    fn new(base: &str, endpoint: ProxyEndpoint) -> Self {
        let store = dir();
        let work = dir();
        let journal = Arc::new(
            Journal::open(
                &store.path().join("journal"),
                Identity {
                    network_id: "918".into(),
                    msb_bootstrap: d(1),
                    subnet_bootstrap: d(2),
                    controller_pubkey: d(3),
                },
                attempts::Limits {
                    max_records: 50,
                    max_unfinished: 50,
                    closed_retention_ms: 1000,
                },
            )
            .unwrap(),
        );
        let connection=Arc::new(HttpConnection::new(serde_json::from_value::<ConnectionConfig>(json!({"schema_version":1,"id":"fixture","revision":1,"base_url":base,"network":{"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true},"paths":{"chat_completions":"chat/completions","completions":"completions","responses":"responses","decisions":"decisions"},"error_profile":"open_ai"})).unwrap()).unwrap());
        let family = match endpoint {
            ProxyEndpoint::Chat => mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
            ProxyEndpoint::Completions => mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS,
            ProxyEndpoint::Responses => mayhem_proto::ENDPOINT_OPENAI_RESPONSES,
            ProxyEndpoint::Decisions => mayhem_proto::ENDPOINT_MAYHEM_DECISIONS,
        };
        let adapter = Arc::new(
            Adapter::new(
                endpoint,
                endpoint_family_contract_template(family).unwrap(),
                "upstream-model".into(),
                Limits {
                    request_bytes: 1024 * 1024,
                    response_bytes: 1024 * 1024,
                    choices: 8,
                    tools: 16,
                    questions: 16,
                    decision_options: 32,
                },
            )
            .unwrap(),
        );
        let pool = Arc::new(
            Pool::new(
                env!("CARGO_BIN_EXE_mayhem-proxy-worker"),
                work.path(),
                PoolLimits {
                    max_children: 2,
                    max_buffer_bytes: 64 * 1024 * 1024,
                    startup_timeout: Duration::from_secs(5),
                    processing_timeout: Duration::from_secs(3),
                },
            )
            .unwrap(),
        );
        let storage = Arc::new(Storage::new(journal.clone(), 8).unwrap());
        let executor = Executor::new(connection.clone(), adapter.clone(), pool, storage).unwrap();
        Self {
            _store: store,
            _work: work,
            journal,
            executor,
            adapter,
            connection,
        }
    }
    fn prepare(&self, n: u64, body: &[u8], rail: ProxyRail) -> attempts::Record {
        let request = self.adapter.prepare_json(body).unwrap();
        self.journal
            .prepare(
                d(n),
                Binding {
                    request_hash: request.request_hash().clone(),
                    endpoint: request.endpoint(),
                    contract_version: 30,
                    provider_pubkey: d(5),
                    market_id: d(6),
                    offer_digest: d(7),
                    endpoint_contract: self.adapter.contract_hash().clone(),
                    metering_policy: d(8),
                    accepted_terms: d(9),
                    reservation: d(10),
                    capacity_lease: d(11),
                    connection_digest: self.connection.fingerprint().clone(),
                    connection_revision: self.connection.revision(),
                    recipe_digest: self.adapter.recipe_hash().clone(),
                    rail,
                },
                1000,
            )
            .unwrap()
    }
}
fn chat() -> Vec<u8> {
    serde_json::to_vec(
        &json!({"model":"public-model","messages":[{"role":"user","content":"hello"}]}),
    )
    .unwrap()
}
fn answer() -> Value {
    json!({"id":"upstream_id","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3},"mayhem":{"receipt":{"paid":true}}})
}

#[tokio::test]
async fn all_four_endpoints_execute_through_real_http_worker_and_journal_without_settlement() {
    for (i, endpoint, request, response) in [
        (
            0,
            ProxyEndpoint::Chat,
            json!({"model":"public-model","messages":[{"role":"user","content":"hi"}]}),
            answer(),
        ),
        (
            1,
            ProxyEndpoint::Completions,
            json!({"model":"public-model","prompt":"hi"}),
            json!({"id":"u","choices":[{"index":0,"text":"hello","finish_reason":"stop"}]}),
        ),
        (
            2,
            ProxyEndpoint::Responses,
            json!({"model":"public-model","input":"hi"}),
            json!({"id":"u","status":"completed","output":[{"id":"i","type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}]}),
        ),
        (
            3,
            ProxyEndpoint::Decisions,
            json!({"model":"public-model","state":"hi","questions":{"q":{"type":"noul","instructions":"hello?"}}}),
            json!({"id":"u","answers":{"q":{"type":"noul","noul":0.7}}}),
        ),
    ] {
        let backend = backend(200, response, Duration::ZERO).await;
        let fixture = Fixture::new(&backend.base, endpoint);
        let bytes = serde_json::to_vec(&request).unwrap();
        let rail = [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap][i % 3];
        let record = fixture.prepare(100 + i as u64, &bytes, rail);
        let result = fixture
            .executor
            .execute_json(&record.invocation, &bytes, &Cancellation::default())
            .await
            .unwrap();
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.bodies.lock().unwrap()[0]["model"], "upstream-model");
        if endpoint == ProxyEndpoint::Responses {
            assert_eq!(backend.bodies.lock().unwrap()[0]["store"], false);
        }
        assert_eq!(result.reply.body["model"], "public-model");
        assert_eq!(
            result.reply.body["id"],
            format!("proxy_{}", record.invocation.as_str())
        );
        assert_eq!(result.attempt.binding.rail, rail);
        assert!(result.reply.body.get("mayhem").is_none());
        let current = fixture.journal.get(&record.invocation).unwrap().unwrap();
        assert_eq!(current.phase, Phase::Dispatched);
        assert!(current.resolution.is_none() && current.closure.is_none());
        assert!(!current.output_may_have_been_delivered);
        assert!(current.remote_id.is_some());
        assert!(matches!(
            fixture
                .executor
                .execute_json(&record.invocation, &bytes, &Cancellation::default())
                .await,
            Err(Error::RecoveryRequired)
        ));
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            1,
            "no implicit replay POST"
        );
    }
}

#[tokio::test]
async fn successful_late_body_cannot_override_a_durably_cancelled_invocation() {
    let backend = backend(200, answer(), Duration::from_millis(100)).await;
    let fixture = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let bytes = chat();
    let r = fixture.prepare(1, &bytes, ProxyRail::Fiat);
    let cancel = Cancellation::default();
    let execution = fixture
        .executor
        .execute_json(&r.invocation, &bytes, &cancel);
    tokio::pin!(execution);
    tokio::select! {
        result=&mut execution=>panic!("unexpected early result {result:?}"),
        _=async {while backend.calls.load(Ordering::SeqCst)==0{tokio::time::sleep(Duration::from_millis(2)).await;}}=>()
    }
    let current = fixture.journal.get(&r.invocation).unwrap().unwrap();
    fixture
        .journal
        .advance(
            &r.invocation,
            current.generation,
            attempts::Event::CancelRequested,
            current.updated_at_ms + 1,
        )
        .unwrap();
    assert!(matches!(execution.await, Err(Error::Cancelled)));
    let current = fixture.journal.get(&r.invocation).unwrap().unwrap();
    assert!(current.cancellation_requested && current.remote_id.is_some());
    assert!(current.closure.is_none() && !current.output_may_have_been_delivered);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn wrong_request_contract_recipe_connection_or_revision_never_reaches_upstream() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let fixture = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let bytes = chat();
    let base = fixture.prepare(1, &bytes, ProxyRail::Fiat);
    for n in 2..7 {
        let mut binding = base.binding.clone();
        match n {
            2 => binding.request_hash = d(90),
            3 => binding.endpoint_contract = d(90),
            4 => binding.recipe_digest = d(90),
            5 => binding.connection_digest = d(90),
            _ => binding.connection_revision = 2,
        }
        fixture.journal.prepare(d(n), binding, 1000).unwrap();
        assert!(matches!(
            fixture
                .executor
                .execute_json(&d(n), &bytes, &Cancellation::default())
                .await,
            Err(Error::Binding)
        ));
        assert_eq!(
            fixture.journal.get(&d(n)).unwrap().unwrap().phase,
            Phase::Prepared
        );
    }
    let bad =
        br#"{"model":"public-model","messages":[{"role":"user","content":"hi"}],"stream":true}"#;
    assert!(matches!(
        fixture
            .executor
            .execute_json(&d(1), bad, &Cancellation::default())
            .await,
        Err(Error::Endpoint(_))
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_same_invocation_submits_once_and_does_not_mint_a_second_attempt() {
    let backend = backend(200, answer(), Duration::from_millis(50)).await;
    let fixture = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let bytes = chat();
    let record = fixture.prepare(1, &bytes, ProxyRail::Fiat);
    let cancel = Cancellation::default();
    let (a, b) = tokio::join!(
        fixture
            .executor
            .execute_json(&record.invocation, &bytes, &cancel),
        fixture
            .executor
            .execute_json(&record.invocation, &bytes, &cancel)
    );
    assert_eq!(a.is_ok() as u8 + b.is_ok() as u8, 1);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture
            .journal
            .get(&record.invocation)
            .unwrap()
            .unwrap()
            .attempt,
        record.attempt
    );
}

#[tokio::test]
async fn upstream_auth_rate_limit_and_protocol_failures_are_saved_without_false_refunds() {
    for (status, response, code) in [
        (
            401,
            json!({"error":{"code":"invalid_api_key","message":"private-secret"}}),
            Code::UpstreamAuthentication,
        ),
        (
            429,
            json!({"error":{"code":"rate_limit_exceeded"}}),
            Code::UpstreamRateLimited,
        ),
        (
            200,
            json!({"error":{"code":"insufficient_quota"}}),
            Code::UpstreamPaymentRequired,
        ),
        (
            200,
            json!({"choices":[{"index":0,"message":{"role":"assistant","content":"cut off"},"finish_reason":null}]}),
            Code::UpstreamProtocol,
        ),
        (202, json!({"id":"queued-job"}), Code::UpstreamProtocol),
    ] {
        let backend = backend(status, response, Duration::ZERO).await;
        let fixture = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let bytes = chat();
        let r = fixture.prepare(1, &bytes, ProxyRail::Tap);
        let error = fixture
            .executor
            .execute_json(&r.invocation, &bytes, &Cancellation::default())
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("private-secret"));
        let current = fixture.journal.get(&r.invocation).unwrap().unwrap();
        assert_eq!(current.phase, Phase::Dispatched);
        let failure = current.last_failure.unwrap();
        assert_eq!(failure.code, code);
        assert_eq!(failure.execution, Execution::Unknown);
        assert!(current.resolution.is_none() && current.closure.is_none());
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn cancellation_before_dispatch_does_not_call_backend_and_after_dispatch_retains_uncertainty()
{
    let backend = backend(200, answer(), Duration::from_millis(250)).await;
    let fixture = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let bytes = chat();
    let a = fixture.prepare(1, &bytes, ProxyRail::Fiat);
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(matches!(
        fixture
            .executor
            .execute_json(&a.invocation, &bytes, &cancel)
            .await,
        Err(Error::Cancelled)
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture.journal.get(&a.invocation).unwrap().unwrap().phase,
        Phase::Resolved
    );
    let b = fixture.prepare(2, &bytes, ProxyRail::Tnk);
    let cancel = Cancellation::default();
    let execution = fixture
        .executor
        .execute_json(&b.invocation, &bytes, &cancel);
    tokio::pin!(execution);
    tokio::select! {result=&mut execution=>panic!("returned before cancellation {result:?}"),_=async {while backend.calls.load(Ordering::SeqCst)==0{tokio::time::sleep(Duration::from_millis(2)).await;}cancel.cancel();}=>()}
    assert!(matches!(execution.await, Err(Error::Cancelled)));
    let current = fixture.journal.get(&b.invocation).unwrap().unwrap();
    assert_eq!(current.phase, Phase::Dispatched);
    assert!(current.cancellation_requested);
    assert!(current.closure.is_none());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn dropped_caller_future_leaves_a_recoverable_attempt_without_redispatch() {
    let backend = backend(200, answer(), Duration::from_millis(250)).await;
    let fixture = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let bytes = chat();
    let r = fixture.prepare(1, &bytes, ProxyRail::Fiat);
    let cancel = Cancellation::default();
    {
        let execution = fixture
            .executor
            .execute_json(&r.invocation, &bytes, &cancel);
        tokio::pin!(execution);
        tokio::select! {result=&mut execution=>panic!("unexpected completion {result:?}"),_=async {while backend.calls.load(Ordering::SeqCst)==0{tokio::time::sleep(Duration::from_millis(2)).await;}}=>()}
    }
    assert_eq!(
        fixture.journal.get(&r.invocation).unwrap().unwrap().phase,
        Phase::Dispatched
    );
    assert!(matches!(
        fixture
            .executor
            .execute_json(&r.invocation, &bytes, &cancel)
            .await,
        Err(Error::RecoveryRequired)
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
}
