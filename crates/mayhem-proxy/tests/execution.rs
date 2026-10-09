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
#[path = "support/responses.rs"]
mod response_fixture;

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
    backend_raw(
        status,
        serde_json::to_vec(&body).unwrap(),
        "application/json",
        delay,
    )
    .await
}
async fn backend_raw(
    status: u16,
    body: Vec<u8>,
    content_type: &'static str,
    delay: Duration,
) -> Backend {
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
            let header=format!("HTTP/1.1 {status} Fixture\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
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
        Self::with_payload_limit(base, endpoint, 128 * 1024 * 1024)
    }
    fn with_payload_limit(base: &str, endpoint: ProxyEndpoint, payload_limit: u64) -> Self {
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
                    max_payload_bytes: payload_limit,
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
        self.prepare_kind(n, body, rail, false)
    }
    fn prepare_kind(&self, n: u64, body: &[u8], rail: ProxyRail, stream: bool) -> attempts::Record {
        self.prepare_with_lease(n, body, rail, stream, d(11))
    }
    fn prepare_with_lease(
        &self,
        n: u64,
        body: &[u8],
        rail: ProxyRail,
        stream: bool,
        capacity_lease: Digest,
    ) -> attempts::Record {
        let request = if stream {
            self.adapter.prepare_stream(body).unwrap()
        } else {
            self.adapter.prepare_json(body).unwrap()
        };
        let policy = mayhem_proxy::metering::Policy::for_endpoint(request.endpoint());
        let offer = mayhem_proto::proxy::ProxyOffer {
            schema_version: 1,
            lane: mayhem_proto::proxy::ProxyLane::Proxy,
            market_id: d(6).as_str().into(),
            provider_pubkey: d(5).as_str().into(),
            membership_revision: 1,
            revision: 1,
            endpoint: request.endpoint(),
            ctx_bracket: "ctx_1024".into(),
            outcome_class: String::new(),
            metering_policy_hash: policy.hash().as_str().into(),
            rates: policy
                .contract()
                .units
                .into_iter()
                .map(|unit| mayhem_proto::proxy::ProxyRate {
                    unit,
                    per_unit_au: 1,
                    granularity: 1,
                })
                .collect(),
            per_request_au: 0,
            min_session_au: 0,
            accepted_rails: vec![rail],
        };
        let record = self
            .journal
            .prepare(
                d(n),
                Binding {
                    request_hash: request.request_hash().clone(),
                    endpoint: request.endpoint(),
                    contract_version: 30,
                    provider_pubkey: d(5),
                    market_id: d(6),
                    offer_digest: Digest::new(offer.digest().unwrap()).unwrap(),
                    endpoint_contract: self.adapter.contract_hash().clone(),
                    metering_policy: request.metering_policy_hash(),
                    accepted_terms: d(9),
                    reservation: d(10),
                    capacity_lease,
                    connection_digest: self.connection.fingerprint().clone(),
                    connection_revision: self.connection.revision(),
                    recipe_digest: self.adapter.recipe_hash().clone(),
                    rail,
                },
                1000,
            )
            .unwrap();
        self.journal
            .retain_acceptance(
                &record.invocation,
                record.attempt,
                &attempts::AcceptanceSnapshot {
                    adapter: self.adapter.snapshot(),
                    offer,
                },
            )
            .unwrap();
        record
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
    for n in 2..8 {
        let mut binding = base.binding.clone();
        match n {
            2 => binding.request_hash = d(90),
            3 => binding.endpoint_contract = d(90),
            4 => binding.recipe_digest = d(90),
            5 => binding.connection_digest = d(90),
            6 => binding.connection_revision = 2,
            _ => binding.metering_policy = d(90),
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
async fn owned_usage_prices_the_accepted_offer_once_with_rail_and_budget_binding() {
    use mayhem_proto::proxy::{ProxyLane, ProxyOffer, ProxyRate};
    use mayhem_proxy::metering::{self, Policy};
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let (input, output) = match endpoint {
            ProxyEndpoint::Chat => (serde_json::from_slice(&chat()).unwrap(), answer()),
            ProxyEndpoint::Completions => (
                json!({"model":"public","prompt":"hello"}),
                json!({"choices":[{"index":0,"text":"hello","finish_reason":"stop"}]}),
            ),
            ProxyEndpoint::Responses => (
                json!({"model":"public","input":"hello"}),
                json!({"status":"completed","output":[{"id":"m1","type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}]}),
            ),
            ProxyEndpoint::Decisions => (
                json!({"model":"public","state":"hello","questions":{"q":{"type":"noul","instructions":"hello?"}}}),
                json!({"answers":{"q":{"type":"noul","noul":0.9}}}),
            ),
        };
        let backend = backend(200, output, Duration::ZERO).await;
        let fixture = Fixture::new(&backend.base, endpoint);
        let body = serde_json::to_vec(&input).unwrap();
        for (i, rail) in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap]
            .into_iter()
            .enumerate()
        {
            let n = 100 + i as u64;
            let base = fixture.prepare(n, &body, rail);
            let b = &base.binding;
            let policy = Policy::for_endpoint(endpoint);
            let offer = ProxyOffer {
                schema_version: 1,
                lane: ProxyLane::Proxy,
                market_id: b.market_id.as_str().into(),
                provider_pubkey: b.provider_pubkey.as_str().into(),
                membership_revision: 1,
                revision: 7,
                endpoint,
                ctx_bracket: "ctx_1024".into(),
                outcome_class: String::new(),
                metering_policy_hash: policy.hash().as_str().into(),
                rates: policy
                    .contract()
                    .units
                    .into_iter()
                    .map(|unit| ProxyRate {
                        unit,
                        per_unit_au: 7,
                        granularity: 3,
                    })
                    .collect(),
                per_request_au: 5,
                min_session_au: 10,
                accepted_rails: vec![rail],
            };
            let mut binding = b.clone();
            binding.offer_digest = Digest::new(offer.digest().unwrap()).unwrap();
            // Wire terms are supplied by the parent; this test verifies the real
            // HTTP -> worker -> retained evidence -> unsigned receipt boundary.
            let wire: Value = serde_json::from_str(include_str!(
                "../../mayhem-proto/tests/fixtures/proxy-finance-v1.json"
            ))
            .unwrap();
            let financial_policy: mayhem_proto::proxy::finance::ProxySettlementPolicy =
                serde_json::from_value(wire["cases"][0]["policy"].clone()).unwrap();
            let mut terms: mayhem_proto::proxy::finance::ProxySpendTerms =
                serde_json::from_value(wire["cases"][0]["terms"].clone()).unwrap();
            terms.offer = offer.clone();
            terms.rail = rail;
            terms.request_hash = binding.request_hash.as_str().into();
            terms.reservation_id = binding.reservation.as_str().into();
            terms.capacity_lease = binding.capacity_lease.as_str().into();
            terms.endpoint_contract = binding.endpoint_contract.as_str().into();
            terms.recipe_hash = binding.recipe_digest.as_str().into();
            terms.connection_digest = binding.connection_digest.as_str().into();
            terms.connection_revision = binding.connection_revision;
            terms.max_usage = offer.rates.iter().map(|r| (r.unit.clone(), 1024)).collect();
            terms.max_spend_au = offer.cost(&terms.max_usage).unwrap();
            terms.max_total_spend_au =
                terms.prior_spend_au + terms.prior_reserved_au + terms.max_spend_au;
            binding.accepted_terms = Digest::new(terms.digest().unwrap()).unwrap();
            let r = fixture.journal.prepare(d(n + 100), binding, 1000).unwrap();
            fixture
                .journal
                .retain_acceptance(
                    &r.invocation,
                    r.attempt,
                    &attempts::AcceptanceSnapshot {
                        adapter: fixture.adapter.snapshot(),
                        offer: offer.clone(),
                    },
                )
                .unwrap();
            fixture
                .executor
                .execute_json(&r.invocation, &body, &Cancellation::default())
                .await
                .unwrap();
            let mut saved = fixture.journal.recover(&r.invocation, r.attempt).unwrap();
            let quoted = metering::price_completed(&saved, u128::MAX).unwrap();
            assert_eq!(quoted.binding.rail, rail);
            let expected = quoted
                .observation
                .units
                .values()
                .fold(5, |s, n| s + (u128::from(*n) * 7).div_ceil(3))
                .max(10);
            assert_eq!(quoted.subtotal_au, expected);
            let receipt =
                metering::draft_terminal_receipt(&saved, &terms, &financial_policy, 1, 2000, None)
                    .unwrap();
            assert_eq!(receipt.au_owed_cum, expected);
            assert_eq!(receipt.billing_au_owed_cum, expected + terms.prior_spend_au);
            assert_eq!(receipt.usage, quoted.observation.units);
            let original_digest = receipt.digest().unwrap();
            let reopened = fixture.journal.recover(&r.invocation, r.attempt).unwrap();
            assert_eq!(
                metering::draft_terminal_receipt(
                    &reopened,
                    &terms,
                    &financial_policy,
                    1,
                    2000,
                    Some(&receipt)
                )
                .unwrap()
                .digest()
                .unwrap(),
                original_digest
            );
            let mut repriced = terms.clone();
            repriced.offer.rates[0].per_unit_au += 1;
            assert!(metering::draft_terminal_receipt(
                &saved,
                &repriced,
                &financial_policy,
                1,
                2000,
                None
            )
            .is_err());
            assert!(metering::draft_terminal_receipt(
                &saved,
                &terms,
                &financial_policy,
                2,
                2001,
                Some(&receipt)
            )
            .is_err());
            assert_eq!(
                metering::price_completed(&saved, expected)
                    .unwrap()
                    .digest()
                    .unwrap(),
                quoted.digest().unwrap()
            );
            assert!(matches!(
                metering::price_completed(&saved, expected - 1),
                Err(metering::Error::Budget)
            ));
            for edit in 0..6 {
                let mut wrong = offer.clone();
                match edit {
                    0 => wrong.revision += 1,
                    1 => wrong.rates[0].per_unit_au += 1,
                    2 => wrong.provider_pubkey = d(99).as_str().into(),
                    3 => {
                        wrong.accepted_rails = vec![if rail == ProxyRail::Fiat {
                            ProxyRail::Tap
                        } else {
                            ProxyRail::Fiat
                        }]
                    }
                    4 => wrong.market_id = d(99).as_str().into(),
                    _ => wrong.metering_policy_hash = d(99).as_str().into(),
                }
                saved.acceptance.as_mut().unwrap().snapshot.offer = wrong;
                assert!(matches!(
                    metering::price_completed(&saved, u128::MAX),
                    Err(metering::Error::Offer)
                ));
            }
            saved = fixture.journal.recover(&r.invocation, r.attempt).unwrap();
            let observed = saved
                .result
                .as_mut()
                .unwrap()
                .reply
                .observed_usage
                .as_mut()
                .unwrap();
            *observed.units.values_mut().next().unwrap() += 1;
            assert!(matches!(
                metering::price_completed(&saved, u128::MAX),
                Err(metering::Error::Evidence)
            ));
            saved = fixture.journal.recover(&r.invocation, r.attempt).unwrap();
            saved.record.cancellation_requested = true;
            assert!(matches!(
                metering::price_completed(&saved, u128::MAX),
                Err(metering::Error::OutcomePolicyRequired)
            ));
            assert_eq!(
                fixture.journal.get(&r.invocation).unwrap().unwrap().phase,
                Phase::Dispatched
            );
            assert!(fixture
                .journal
                .get(&r.invocation)
                .unwrap()
                .unwrap()
                .resolution
                .is_none());
        }
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            3,
            "pricing/recovery must not call upstream again"
        );
    }
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

#[tokio::test]
async fn invalid_or_remote_schema_never_dispatches_or_changes_attempt_phase() {
    for schema in [
        json!({"type":"not-a-type"}),
        json!({"$ref":"http://127.0.0.1:18081/private"}),
        json!({"type":"array","misspelledUniqueItems":true}),
    ] {
        let backend = backend(200, answer(), Duration::ZERO).await;
        let fixture = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut body: Value = serde_json::from_slice(&chat()).unwrap();
        body["response_format"] = json!({"type":"json_schema","json_schema":{"name":"result","strict":true,"schema":schema}});
        let bytes = serde_json::to_vec(&body).unwrap();
        let record = fixture.prepare(1, &bytes, ProxyRail::Fiat);
        let err = fixture
            .executor
            .execute_json(&record.invocation, &bytes, &Cancellation::default())
            .await
            .unwrap_err();
        assert!(
            matches!(err,Error::Decoder(mayhem_proxy::worker::Error::Upstream(ref f)) if f.code==Code::InvalidSchema && f.execution==Execution::NotDispatched)
        );
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            fixture
                .journal
                .get(&record.invocation)
                .unwrap()
                .unwrap()
                .phase,
            Phase::Prepared
        );
    }
}

#[tokio::test]
async fn original_structured_schema_is_checked_in_real_worker_before_returning_result() {
    for (content, valid) in [
        (r#"{"ids":["E1","E2"]}"#, true),
        (r#"{"ids":["E1","E1"]}"#, false),
        (r#"{"ids":["E1",7]}"#, false),
    ] {
        let mut response = answer();
        response["choices"][0]["message"]["content"] = json!(content);
        let backend = backend(200, response, Duration::ZERO).await;
        let fixture = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut body: Value = serde_json::from_slice(&chat()).unwrap();
        body["response_format"] = json!({"type":"json_schema","json_schema":{"name":"result","strict":true,"schema":{"type":"object","required":["ids"],"properties":{"ids":{"type":"array","minItems":2,"uniqueItems":true,"items":{"type":"string"}}}}}});
        let bytes = serde_json::to_vec(&body).unwrap();
        let record = fixture.prepare(1, &bytes, ProxyRail::Tnk);
        let result = fixture
            .executor
            .execute_json(&record.invocation, &bytes, &Cancellation::default())
            .await;
        assert_eq!(result.is_ok(), valid, "{content}: {result:?}");
        if valid {
            assert_eq!(
                result.unwrap().reply.body["choices"][0]["message"]["content"],
                content
            );
        }
        let current = fixture.journal.get(&record.invocation).unwrap().unwrap();
        assert_eq!(current.phase, Phase::Dispatched);
        assert_eq!(current.last_failure.is_some(), !valid);
        assert!(current.closure.is_none());
    }
}

#[tokio::test]
async fn tool_argument_schema_violation_is_not_returned_as_an_executable_call() {
    for arguments in [r#"{"path":7}"#, r#"{}"#, r#"{"path":"src/game.js"}"#] {
        let mut response = answer();
        response["choices"][0]["finish_reason"] = json!("tool_calls");
        response["choices"][0]["message"]["tool_calls"] = json!([{"id":"call_1","type":"function","function":{"name":"read_file","arguments":arguments}}]);
        let backend = backend(200, response, Duration::ZERO).await;
        let fixture = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut body: Value = serde_json::from_slice(&chat()).unwrap();
        body["tools"] = json!([{"type":"function","function":{"name":"read_file","parameters":{"type":"object","required":["path"],"properties":{"path":{"type":"string"}},"additionalProperties":false}}}]);
        let bytes = serde_json::to_vec(&body).unwrap();
        let record = fixture.prepare(1, &bytes, ProxyRail::Tap);
        let result = fixture
            .executor
            .execute_json(&record.invocation, &bytes, &Cancellation::default())
            .await;
        assert_eq!(result.is_ok(), arguments.contains("src/game.js"));
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    }
}

fn sse(chunks: &[Value], done: bool) -> Vec<u8> {
    let mut text = String::new();
    for chunk in chunks {
        text.push_str("data: ");
        text.push_str(&chunk.to_string());
        text.push_str("\n\n");
    }
    if done {
        text.push_str("data: [DONE]\n\n");
    }
    text.into_bytes()
}
fn delta(content: &str, finish: Value) -> Value {
    json!({"id":"stream_1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":content},"finish_reason":finish}]})
}
fn stream_request() -> Vec<u8> {
    let mut body: Value = serde_json::from_slice(&chat()).unwrap();
    body["stream"] = json!(true);
    serde_json::to_vec(&body).unwrap()
}

#[tokio::test]
async fn responses_events_use_real_worker_schema_checks_and_never_emit_unverified_completion() {
    for (arguments, valid) in [
        (r#"{"city":"München"}"#, true),
        (r#"{"city":12}"#, false),
        ("{}", false),
    ] {
        let events = response_fixture::flow(arguments);
        let backend = backend_raw(
            200,
            sse(&events, false),
            "text/event-stream",
            Duration::ZERO,
        )
        .await;
        let fixture = Fixture::new(&backend.base, ProxyEndpoint::Responses);
        let bytes = response_fixture::body();
        let record = fixture.prepare_kind(1, &bytes, ProxyRail::Tap, true);
        let mut provisional = Vec::new();
        let result = fixture
            .executor
            .execute_stream(&record.invocation, &bytes, &Cancellation::default(), |v| {
                provisional.push(v);
                async { Ok(()) }
            })
            .await;
        assert_eq!(result.is_ok(), valid, "{arguments}: {result:?}");
        assert!(!provisional.is_empty());
        for value in provisional {
            assert!(!value["type"].as_str().unwrap().ends_with(".done"));
            assert_ne!(value["type"], "response.completed");
            assert!(!value.to_string().contains("private-upstream-settings"));
            assert!(value.get("usage").is_none());
        }
        if valid {
            let result = result.unwrap();
            assert_eq!(result.reply.body["output"][2]["arguments"], arguments);
            assert_eq!(result.reply.body["model"], "public-model");
            assert_eq!(result.reply.reported_usage.unwrap().output_tokens, 12);
            assert!(result.reply.body.get("usage").is_none());
        }
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        let current = fixture.journal.get(&record.invocation).unwrap().unwrap();
        assert_eq!(current.phase, Phase::Dispatched);
        assert_eq!(current.last_failure.is_some(), !valid);
        assert!(current.closure.is_none());
    }
}

#[tokio::test]
async fn responses_failed_envelope_retains_safe_cause_without_vendor_text_or_replay() {
    for (code, expected) in [
        ("invalid_api_key", Code::UpstreamAuthentication),
        ("insufficient_quota", Code::UpstreamPaymentRequired),
        ("server_error", Code::UpstreamUnavailable),
    ] {
        let mut events = response_fixture::flow(r#"{"city":"München"}"#);
        events.truncate(2);
        response_fixture::append(
            &mut events,
            "response.failed",
            json!({"response":{"id":"upstream-response","object":"response","status":"failed","error":{"code":code,"message":"private-credential-data"}}}),
        );
        let backend = backend_raw(
            200,
            sse(&events, false),
            "text/event-stream",
            Duration::ZERO,
        )
        .await;
        let fixture = Fixture::new(&backend.base, ProxyEndpoint::Responses);
        let bytes = response_fixture::body();
        let r = fixture.prepare_kind(1, &bytes, ProxyRail::Fiat, true);
        let result = fixture
            .executor
            .execute_stream(&r.invocation, &bytes, &Cancellation::default(), |_| async {
                Ok(())
            })
            .await;
        assert!(result.is_err());
        assert!(!format!("{result:?}").contains("private-credential-data"));
        let current = fixture.journal.get(&r.invocation).unwrap().unwrap();
        let failure = current.last_failure.unwrap();
        assert_eq!(failure.code, expected);
        assert_eq!(failure.execution, Execution::Unknown);
        assert!(current.closure.is_none());
        assert!(matches!(
            fixture
                .executor
                .execute_stream(&r.invocation, &bytes, &Cancellation::default(), |_| async {
                    Ok(())
                })
                .await,
            Err(Error::RecoveryRequired)
        ));
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn responses_structured_text_checks_original_schema_and_allows_explicit_refusal() {
    for (text, refusal, valid) in [
        (r#"{"ids":["a","b"]}"#, false, true),
        (r#"{"ids":["a","a"]}"#, false, false),
        ("Cannot answer", true, true),
    ] {
        let events = response_fixture::text_flow(text, refusal);
        let backend = backend_raw(
            200,
            sse(&events, false),
            "text/event-stream",
            Duration::ZERO,
        )
        .await;
        let fixture = Fixture::new(&backend.base, ProxyEndpoint::Responses);
        let bytes=serde_json::to_vec(&json!({"model":"public-model","input":"Give unique ids","stream":true,
            "text":{"format":{"type":"json_schema","name":"ids","strict":true,"schema":{"type":"object","required":["ids"],"properties":{"ids":{"type":"array","uniqueItems":true,"items":{"type":"string"},"minItems":2}}}}}})).unwrap();
        let r = fixture.prepare_kind(1, &bytes, ProxyRail::Tnk, true);
        let result = fixture
            .executor
            .execute_stream(&r.invocation, &bytes, &Cancellation::default(), |_| async {
                Ok(())
            })
            .await;
        assert_eq!(result.is_ok(), valid, "{text}: {result:?}");
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn storage_capacity_is_reserved_before_post_and_retained_result_recovers_without_network() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let blocked = Fixture::with_payload_limit(&backend.base, ProxyEndpoint::Chat, 64 * 1024);
    let body = chat();
    let r = blocked.prepare(1, &body, ProxyRail::Fiat);
    assert!(matches!(
        blocked
            .executor
            .execute_json(&r.invocation, &body, &Cancellation::default())
            .await,
        Err(Error::Journal(attempts::Error::Capacity))
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        blocked.journal.get(&r.invocation).unwrap().unwrap().phase,
        Phase::Prepared
    );

    let fixture = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let r = fixture.prepare(2, &body, ProxyRail::Tap);
    let result = fixture
        .executor
        .execute_json(&r.invocation, &body, &Cancellation::default())
        .await
        .unwrap();
    let expected = result.reply.body.clone();
    let commitment = result.result_digest.clone();
    assert!(fixture.journal.allocated_payload_bytes().unwrap() > 0);
    assert_eq!(fixture.journal.prune_closed(u64::MAX, 64).unwrap(), 0);
    let Fixture {
        _store: store,
        _work: work,
        journal,
        executor,
        adapter,
        connection,
    } = fixture;
    drop(executor);
    drop(journal);
    drop(adapter);
    drop(connection);
    let reopened = Arc::new(
        Journal::open(
            store.path().join("journal"),
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
                max_payload_bytes: 1,
            },
        )
        .unwrap(),
    );
    let storage = Storage::new(reopened.clone(), 2).unwrap();
    let recovered = storage.recover(&r.invocation, r.attempt).await.unwrap();
    let quote = mayhem_proxy::metering::price_completed(&recovered, u128::MAX).unwrap();
    let snapshot = recovered.acceptance.as_ref().unwrap();
    let restored = Adapter::restore(snapshot.snapshot.adapter.clone()).unwrap();
    assert_eq!(restored.recipe_hash(), &r.binding.recipe_digest);
    assert_eq!(
        restored.prepare_json(&body).unwrap().request_hash(),
        &r.binding.request_hash
    );
    assert_eq!(quote.binding.rail, ProxyRail::Tap);
    let mut changed = snapshot.snapshot.clone();
    changed.adapter.upstream_model = "new-private-model".into();
    assert!(matches!(
        reopened.retain_acceptance(&r.invocation, r.attempt, &changed),
        Err(attempts::Error::Conflict)
    ));
    changed = snapshot.snapshot.clone();
    changed.offer.revision += 1;
    changed.offer.rates[0].per_unit_au += 100;
    assert!(matches!(
        reopened.retain_acceptance(&r.invocation, r.attempt, &changed),
        Err(attempts::Error::Conflict)
    ));
    assert_eq!(
        reopened
            .retain_acceptance(&r.invocation, r.attempt, &snapshot.snapshot)
            .unwrap(),
        snapshot.digest
    );
    assert_eq!(recovered.request.unwrap().body, body);
    let retained = recovered.result.unwrap();
    assert_eq!(retained.reply.body, expected);
    assert_eq!(retained.digest, commitment);
    assert_eq!(recovered.record.binding.rail, ProxyRail::Tap);
    assert_eq!(recovered.record.phase, Phase::Dispatched);
    assert!(recovered.record.closure.is_none());
    assert!(reopened
        .begin_dispatch(
            &r.invocation,
            recovered.record.generation,
            recovered.record.updated_at_ms
        )
        .is_err());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert!(!format!("{retained:?}").contains("hello"));
    drop(storage);
    drop(reopened);
    drop(work);
    drop(store);
}

#[tokio::test]
async fn matching_but_unretained_acceptance_cannot_dispatch() {
    let backend = backend(200, answer(), Duration::ZERO).await;
    let fixture = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let body = chat();
    let valid = fixture.prepare(1, &body, ProxyRail::Fiat);
    let missing = fixture.journal.prepare(d(2), valid.binding, 1000).unwrap();
    assert!(matches!(
        fixture
            .executor
            .execute_json(&missing.invocation, &body, &Cancellation::default())
            .await,
        Err(Error::Binding)
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture
            .journal
            .get(&missing.invocation)
            .unwrap()
            .unwrap()
            .phase,
        Phase::Prepared
    );
}

#[tokio::test]
async fn streaming_delivers_only_provisional_chunks_and_preserves_final_shape() {
    let chunks = [
        delta("Hello ", Value::Null),
        delta("世界", Value::Null),
        delta("", json!("stop")),
        json!({"id":"stream_1","choices":[],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}}),
    ];
    let backend = backend_raw(200, sse(&chunks, true), "text/event-stream", Duration::ZERO).await;
    let fixture = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let body = stream_request();
    let r = fixture.prepare_kind(1, &body, ProxyRail::Tap, true);
    let mut received = Vec::new();
    let result = fixture
        .executor
        .execute_stream(&r.invocation, &body, &Cancellation::default(), |value| {
            received.push(value);
            async { Ok(()) }
        })
        .await
        .unwrap();
    assert_eq!(
        result.reply.body["choices"][0]["message"]["content"],
        "Hello 世界"
    );
    assert_eq!(result.reply.body["choices"][0]["finish_reason"], "stop");
    assert_eq!(result.reply.reported_usage.unwrap().output_tokens, 3);
    assert!(!received.is_empty());
    for chunk in received {
        assert_eq!(chunk["model"], "public-model");
        for choice in chunk["choices"].as_array().unwrap() {
            assert!(choice["finish_reason"].is_null());
        }
    }
    let current = fixture.journal.get(&r.invocation).unwrap().unwrap();
    assert!(current.output_may_have_been_delivered);
    assert!(current.closure.is_none());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn streamed_tool_arguments_are_reassembled_exactly_and_schema_checked() {
    for (args, valid) in [
        ("{\"path\":\"src/世界.js\"}", true),
        ("{\"path\":7}", false),
        ("{\"path\":", false),
    ] {
        let chunks = [
            json!({"id":"stream_1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","type":"function","function":{"name":"read_","arguments":""}}]},"finish_reason":null}]}),
            json!({"id":"stream_1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"file","arguments":args}}]},"finish_reason":"tool_calls"}]}),
        ];
        let backend =
            backend_raw(200, sse(&chunks, true), "text/event-stream", Duration::ZERO).await;
        let fixture = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let mut body: Value = serde_json::from_slice(&stream_request()).unwrap();
        body["tools"] = json!([{"type":"function","function":{"name":"read_file","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}}]);
        let body = serde_json::to_vec(&body).unwrap();
        let r = fixture.prepare_kind(1, &body, ProxyRail::Fiat, true);
        let result = fixture
            .executor
            .execute_stream(&r.invocation, &body, &Cancellation::default(), |_| async {
                Ok(())
            })
            .await;
        assert_eq!(result.is_ok(), valid, "{args}: {result:?}");
        if valid {
            assert_eq!(
                result.unwrap().reply.body["choices"][0]["message"]["tool_calls"][0]["function"]
                    ["arguments"],
                args
            );
        }
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn missing_terminal_changed_provider_and_midstream_error_are_not_success() {
    for case in 0..4 {
        let mut chunks = vec![delta("partial", Value::Null), delta("", json!("stop"))];
        let done = case != 0;
        match case {
            1 => chunks[1]["id"] = json!("another_provider_response"),
            2 => chunks[1]["choices"][0]["finish_reason"] = Value::Null,
            3 => {
                chunks[1] = json!({"error":{"code":"rate_limit_exceeded","message":"private-upstream-details"}})
            }
            _ => (),
        }
        let backend =
            backend_raw(200, sse(&chunks, done), "text/event-stream", Duration::ZERO).await;
        let fixture = Fixture::new(&backend.base, ProxyEndpoint::Chat);
        let body = stream_request();
        let r = fixture.prepare_kind(1, &body, ProxyRail::Tnk, true);
        let result = fixture
            .executor
            .execute_stream(&r.invocation, &body, &Cancellation::default(), |_| async {
                Ok(())
            })
            .await;
        assert!(result.is_err(), "case {case}");
        assert!(!format!("{result:?}").contains("private-upstream-details"));
        let current = fixture.journal.get(&r.invocation).unwrap().unwrap();
        assert_eq!(current.phase, Phase::Dispatched);
        assert!(current.closure.is_none());
        assert_eq!(
            current.last_failure.unwrap().code,
            if case == 3 {
                Code::UpstreamRateLimited
            } else {
                Code::UpstreamProtocol
            }
        );
        assert_eq!(current.remote_id.unwrap().as_str(), "stream_1");
        let retained = fixture.journal.recover(&r.invocation, r.attempt).unwrap();
        assert!(retained.request.is_some() && retained.result.is_none());
    }
}

#[tokio::test]
async fn client_stream_sink_failure_is_cancellation_not_provider_cooldown() {
    let backend = backend_raw(
        200,
        sse(
            &[delta("text", Value::Null), delta("", json!("stop"))],
            true,
        ),
        "text/event-stream",
        Duration::ZERO,
    )
    .await;
    let fixture = Fixture::new(&backend.base, ProxyEndpoint::Chat);
    let body = stream_request();
    let r = fixture.prepare_kind(1, &body, ProxyRail::Fiat, true);
    let result = fixture
        .executor
        .execute_stream(&r.invocation, &body, &Cancellation::default(), |_| async {
            Err(())
        })
        .await;
    assert!(matches!(result, Err(Error::Cancelled)));
    let current = fixture.journal.get(&r.invocation).unwrap().unwrap();
    assert!(current.cancellation_requested);
    assert!(current.last_failure.is_none());
    assert!(current.closure.is_none());
}

#[tokio::test]
async fn legacy_completion_stream_uses_text_shape_and_never_invents_chat_messages() {
    let chunks = [
        json!({"id":"c","object":"text_completion","choices":[{"index":0,"text":"hello","finish_reason":null}]}),
        json!({"id":"c","object":"text_completion","choices":[{"index":0,"text":" world","finish_reason":"length"}]}),
    ];
    let backend = backend_raw(200, sse(&chunks, true), "text/event-stream", Duration::ZERO).await;
    let fixture = Fixture::new(&backend.base, ProxyEndpoint::Completions);
    let body = serde_json::to_vec(&json!({"model":"m","prompt":"hi","stream":true})).unwrap();
    let r = fixture.prepare_kind(1, &body, ProxyRail::Fiat, true);
    let result = fixture
        .executor
        .execute_stream(&r.invocation, &body, &Cancellation::default(), |value| {
            assert!(value["choices"][0].get("text").is_some());
            async { Ok(()) }
        })
        .await
        .unwrap();
    assert_eq!(result.reply.body["choices"][0]["text"], "hello world");
    assert_eq!(result.reply.body["choices"][0]["finish_reason"], "length");
}

#[tokio::test]
async fn terminal_marker_finishes_even_when_upstream_keeps_sse_connection_open() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1/", listener.local_addr().unwrap());
    let body = sse(&[delta("ready", json!("stop"))], true);
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut buf = [0; 1024];
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            if n == 0 {
                return;
            }
            bytes.extend_from_slice(&buf[..n]);
            if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
        let frame = format!("{:x}\r\n", body.len());
        socket.write_all(frame.as_bytes()).await.unwrap();
        socket.write_all(&body).await.unwrap();
        socket.write_all(b"\r\n").await.unwrap();
        // Deliberately never send a zero chunk/EOF. Test timeout is not a model limit.
        std::future::pending::<()>().await;
    });
    let fixture = Fixture::new(&base, ProxyEndpoint::Chat);
    let bytes = stream_request();
    let r = fixture.prepare_kind(1, &bytes, ProxyRail::Fiat, true);
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        fixture.executor.execute_stream(
            &r.invocation,
            &bytes,
            &Cancellation::default(),
            |_| async { Ok(()) },
        ),
    )
    .await;
    task.abort();
    let _ = task.await;
    assert_eq!(
        result.unwrap().unwrap().reply.body["choices"][0]["message"]["content"],
        "ready"
    );
}

fn shared_capacity(f: &Fixture) -> Arc<mayhem_proxy::capacity::Authority> {
    use mayhem_proxy::capacity::*;
    let a = Arc::new(
        Authority::open(
            f._store.path().join("capacity"),
            Identity {
                network_id: "918".into(),
                msb_bootstrap: d(1),
                subnet_bootstrap: d(2),
                controller_pubkey: d(3),
            },
            Limits {
                max_groups: 4,
                max_routes: 8,
                max_leases: 20,
                max_evidence_age: Duration::from_secs(60),
            },
        )
        .unwrap(),
    );
    a.configure_group(d(200), 2).unwrap();
    a.configure_route(Route {
        id: d(201),
        group: d(200),
        lane: Lane::Proxy,
        max_concurrency: 2,
    })
    .unwrap();
    for scope in [Scope::Group(d(200)), Scope::Route(d(201))] {
        capacity_ready(&a, scope);
    }
    a
}
fn capacity_ready(a: &mayhem_proxy::capacity::Authority, scope: mayhem_proxy::capacity::Scope) {
    use mayhem_proxy::capacity::*;
    let ticket = a.begin_observation(scope).unwrap();
    a.observe(
        ticket,
        Evidence {
            state: Readiness::Ready,
            allowance: 2,
            age: Duration::ZERO,
            valid_for: Duration::from_secs(60),
        },
    )
    .unwrap();
}
fn reserve_execution(
    f: &Fixture,
    a: &mayhem_proxy::capacity::Authority,
    n: u64,
    bytes: &[u8],
    stream: bool,
) -> (attempts::Record, mayhem_proxy::capacity::Reservation) {
    let request = if stream {
        f.adapter.prepare_stream(bytes).unwrap()
    } else {
        f.adapter.prepare_json(bytes).unwrap()
    };
    let r = a
        .reserve(
            &d(201),
            mayhem_proxy::capacity::Work {
                invocation: d(n),
                request_hash: request.request_hash().clone(),
            },
        )
        .unwrap();
    let record = f.prepare_with_lease(n, bytes, ProxyRail::Tnk, stream, r.lease().id.clone());
    (record, r)
}

#[tokio::test]
async fn shared_capacity_guards_real_json_and_stream_dispatch_and_never_auto_releases_on_return() {
    use mayhem_proxy::capacity;
    for streaming in [false, true] {
        let b = if streaming {
            backend_raw(
                200,
                sse(
                    &[delta("hello", json!(null)), delta("", json!("stop"))],
                    true,
                ),
                "text/event-stream",
                Duration::ZERO,
            )
            .await
        } else {
            backend(200, answer(), Duration::ZERO).await
        };
        let mut f = Fixture::new(&b.base, ProxyEndpoint::Chat);
        let a = shared_capacity(&f);
        let bytes = if streaming { stream_request() } else { chat() };
        let (record, reservation) = reserve_execution(&f, &a, 1, &bytes, streaming);
        f.executor = f.executor.with_capacity(a.clone(), d(201));
        let cancel = Cancellation::default();
        let reply = if streaming {
            f.executor
                .execute_stream(&record.invocation, &bytes, &cancel, |_| async { Ok(()) })
                .await
                .unwrap()
        } else {
            f.executor
                .execute_json(&record.invocation, &bytes, &cancel)
                .await
                .unwrap()
        };
        assert_eq!(b.calls.load(Ordering::SeqCst), 1);
        let lease = a.lease(&reservation.lease().id).unwrap().unwrap();
        assert_eq!(lease.phase, capacity::Phase::Dispatched);
        assert_eq!(a.status(&d(201)).unwrap().group_occupied, 1);
        assert!(matches!(
            f.executor
                .execute_json(&record.invocation, &chat(), &cancel)
                .await,
            Err(Error::RecoveryRequired) | Err(Error::Binding)
        ));
        assert_eq!(b.calls.load(Ordering::SeqCst), 1);
        // Only the parent's explicitly retained and validated outcome closes capacity.
        assert!(a
            .complete(capacity::VerifiedCompletion {
                lease,
                evidence: reply.result_digest
            })
            .unwrap());
        assert_eq!(a.status(&d(201)).unwrap().available, 2);
    }
}

#[tokio::test]
async fn busy_after_reservation_stops_the_http_post_and_recovery_can_use_same_unsent_attempt() {
    use mayhem_proxy::capacity::*;
    let b = backend(200, answer(), Duration::ZERO).await;
    let mut f = Fixture::new(&b.base, ProxyEndpoint::Chat);
    let a = shared_capacity(&f);
    let bytes = chat();
    let (record, reservation) = reserve_execution(&f, &a, 1, &bytes, false);
    f.executor = f.executor.with_capacity(a.clone(), d(201));
    let ticket = a.begin_observation(Scope::Group(d(200))).unwrap();
    a.observe(
        ticket,
        Evidence {
            state: Readiness::Busy,
            allowance: 0,
            age: Duration::ZERO,
            valid_for: Duration::from_secs(60),
        },
    )
    .unwrap();
    assert!(matches!(
        f.executor
            .execute_json(&record.invocation, &bytes, &Cancellation::default())
            .await,
        Err(mayhem_proxy::execution::Error::Capacity(
            mayhem_proxy::capacity::Error::Busy
        ))
    ));
    assert_eq!(b.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.journal.get(&record.invocation).unwrap().unwrap().phase,
        attempts::Phase::Prepared
    );
    capacity_ready(&a, Scope::Group(d(200)));
    f.executor
        .execute_json(&record.invocation, &bytes, &Cancellation::default())
        .await
        .unwrap();
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        a.lease(&reservation.lease().id).unwrap().unwrap().phase,
        mayhem_proxy::capacity::Phase::Dispatched
    );
}

#[tokio::test]
async fn missing_or_mismatched_capacity_never_reaches_real_upstream() {
    for wrong in ["id", "route", "invocation", "body"] {
        let b = backend(200, answer(), Duration::ZERO).await;
        let mut f = Fixture::new(&b.base, ProxyEndpoint::Chat);
        let a = shared_capacity(&f);
        let bytes = chat();
        let mut work = mayhem_proxy::capacity::Work {
            invocation: d(1),
            request_hash: f
                .adapter
                .prepare_json(&bytes)
                .unwrap()
                .request_hash()
                .clone(),
        };
        if wrong == "invocation" {
            work.invocation = d(999);
        }
        if wrong == "body" {
            work.request_hash = d(999);
        }
        let reservation = a.reserve(&d(201), work).unwrap();
        let lease = if wrong == "id" {
            d(999)
        } else {
            reservation.lease().id.clone()
        };
        let record = f.prepare_with_lease(1, &bytes, ProxyRail::Tap, false, lease);
        f.executor = f
            .executor
            .with_capacity(a.clone(), if wrong == "route" { d(999) } else { d(201) });
        assert!(
            matches!(
                f.executor
                    .execute_json(&record.invocation, &bytes, &Cancellation::default())
                    .await,
                Err(Error::Capacity(_))
            ),
            "{wrong}"
        );
        assert_eq!(b.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            f.journal.get(&record.invocation).unwrap().unwrap().phase,
            Phase::Prepared
        );
        a.cancel_reserved(reservation).unwrap();
    }
}

#[tokio::test]
async fn dropped_guarded_transport_keeps_capacity_occupied_and_cannot_redispatch() {
    let b = backend(200, answer(), Duration::from_millis(250)).await;
    let mut f = Fixture::new(&b.base, ProxyEndpoint::Chat);
    let a = shared_capacity(&f);
    let bytes = chat();
    let (record, reservation) = reserve_execution(&f, &a, 1, &bytes, false);
    f.executor = f.executor.with_capacity(a.clone(), d(201));
    let cancel = Cancellation::default();
    {
        let future = f.executor.execute_json(&record.invocation, &bytes, &cancel);
        tokio::pin!(future);
        tokio::select! { result=&mut future=>panic!("unexpected early result {result:?}"),_=async { while b.calls.load(Ordering::SeqCst) == 0 { tokio::time::sleep(Duration::from_millis(2)).await; } }=>() }
    }
    assert_eq!(a.status(&d(201)).unwrap().group_occupied, 1);
    assert_eq!(
        a.lease(&reservation.lease().id).unwrap().unwrap().phase,
        mayhem_proxy::capacity::Phase::Dispatched
    );
    assert!(matches!(
        f.executor
            .execute_json(&record.invocation, &bytes, &cancel)
            .await,
        Err(Error::RecoveryRequired)
    ));
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn guarded_concurrent_replay_has_one_post_and_two_store_gap_requires_reconciliation() {
    for gap in [false, true] {
        let b = backend(200, answer(), Duration::from_millis(30)).await;
        let mut f = Fixture::new(&b.base, ProxyEndpoint::Chat);
        let a = shared_capacity(&f);
        let bytes = chat();
        let (record, reservation) = reserve_execution(&f, &a, 1, &bytes, false);
        if gap {
            drop(a.dispatch(&reservation).unwrap());
        }
        f.executor = f.executor.with_capacity(a.clone(), d(201));
        let cancel = Cancellation::default();
        let (one, two) = tokio::join!(
            f.executor.execute_json(&record.invocation, &bytes, &cancel),
            f.executor.execute_json(&record.invocation, &bytes, &cancel)
        );
        if gap {
            assert!(one.is_err() && two.is_err());
            assert_eq!(b.calls.load(Ordering::SeqCst), 0);
            assert_eq!(
                f.journal.get(&record.invocation).unwrap().unwrap().phase,
                Phase::Prepared
            );
        } else {
            assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
            assert_eq!(b.calls.load(Ordering::SeqCst), 1);
        }
        assert_eq!(a.status(&d(201)).unwrap().group_occupied, 1);
        assert_eq!(
            a.lease(&reservation.lease().id).unwrap().unwrap().phase,
            mayhem_proxy::capacity::Phase::Dispatched
        );
    }
}

#[path = "support/paid_execution.rs"]
mod paid_acceptance;

#[path = "support/health_execution.rs"]
mod health_execution;
#[path = "support/probe_execution.rs"]
mod probe_execution;
